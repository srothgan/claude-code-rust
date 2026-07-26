// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use crate::agent::bridge::BridgeLauncher;
use crate::agent::wire::{BridgeCommand, CommandEnvelope, EventEnvelope, SessionLaunchSettings};
use crate::error::AppError;
use anyhow::Context as _;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt as _, AsyncWrite, AsyncWriteExt as _, BufReader, BufWriter};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{Instrument as _, info_span};

pub struct BridgeClient {
    child: Child,
    stdout: tokio::io::Lines<BufReader<ChildStdout>>,
    command_tx: CommandSender,
    writer_task: Option<JoinHandle<anyhow::Result<()>>>,
}

#[derive(Debug)]
pub enum BridgeShutdownOutcome {
    Graceful(std::process::ExitStatus),
    Forced(std::process::ExitStatus),
}

impl BridgeClient {
    pub fn spawn(launcher: &BridgeLauncher) -> anyhow::Result<Self> {
        let bridge_diagnostics_enabled = crate::logging::bridge_diagnostics_enabled();
        let spawn_span = info_span!(
            target: crate::logging::targets::BRIDGE_LIFECYCLE,
            "bridge_spawn",
            runtime_path = %launcher.runtime_path.display(),
            script_path = %launcher.script_path.display(),
        );
        let _entered = spawn_span.enter();
        tracing::info!(
            target: crate::logging::targets::BRIDGE_LIFECYCLE,
            event_name = "bridge_spawn_started",
            message = "spawning bridge process",
            outcome = "start",
            runtime_path = %launcher.runtime_path.display(),
            script_path = %launcher.script_path.display(),
        );
        let mut child = launcher
            .command(bridge_diagnostics_enabled)
            .spawn()
            .map_err(|_| anyhow::Error::new(AppError::BridgeSpawnFailed))
            .with_context(|| format!("failed to spawn bridge process: {}", launcher.describe()))?;

        tracing::info!(
            target: crate::logging::targets::BRIDGE_LIFECYCLE,
            event_name = "bridge_spawn_completed",
            message = "bridge process spawned",
            outcome = "success",
            bridge_pid = child.id().unwrap_or_default(),
            runtime_path = %launcher.runtime_path.display(),
            script_path = %launcher.script_path.display(),
        );

        let stdin = child.stdin.take().context("bridge stdin not available")?;
        let stdout = child.stdout.take().context("bridge stdout not available")?;
        if bridge_diagnostics_enabled {
            let stderr = child.stderr.take().context("bridge stderr not available")?;
            Self::spawn_stderr_logger(stderr);
        }

        let (connection, command_rx) = command_channel();
        let command_tx = connection.command_tx;
        let writer_span = info_span!(
            target: crate::logging::targets::BRIDGE_PROTOCOL,
            "bridge_stdin_writer",
        );
        let writer_task = tokio::spawn(
            run_command_writer(BufWriter::new(stdin), command_rx).instrument(writer_span),
        );

        Ok(Self {
            child,
            stdout: BufReader::new(stdout).lines(),
            command_tx,
            writer_task: Some(writer_task),
        })
    }

    fn spawn_stderr_logger(stderr: ChildStderr) {
        tokio::task::spawn(
            async move {
                let mut lines = BufReader::new(stderr).lines();
                loop {
                    match lines.next_line().await {
                        Ok(Some(line)) => crate::logging::emit_bridge_stderr_line(&line),
                        Ok(None) => break,
                        Err(err) => {
                            tracing::error!(
                                target: crate::logging::targets::BRIDGE_SDK,
                                event_name = "bridge_stderr_read_failed",
                                message = "failed to read bridge stderr",
                                error = %err,
                            );
                            break;
                        }
                    }
                }
            }
            .instrument(tracing::Span::current()),
        );
    }

    #[must_use]
    pub fn connection(&self) -> AgentConnection {
        AgentConnection { command_tx: self.command_tx.clone() }
    }

    /// Enqueue a lifecycle command and wait until the stdin writer has flushed
    /// its complete protocol line.
    pub async fn send(&self, envelope: CommandEnvelope) -> anyhow::Result<()> {
        let (write_ack_tx, write_ack_rx) = oneshot::channel();
        self.command_tx.send_reliable(envelope, Some(write_ack_tx)).await?;
        write_ack_rx
            .await
            .context("bridge stdin writer stopped before acknowledging command")?
            .map_err(anyhow::Error::msg)
    }

    pub async fn recv(&mut self) -> anyhow::Result<Option<EventEnvelope>> {
        enum ReceiveOutcome {
            Stdout(std::io::Result<Option<String>>),
            Writer(Result<anyhow::Result<()>, tokio::task::JoinError>),
        }

        let outcome = if let Some(writer_task) = self.writer_task.as_mut() {
            tokio::select! {
                line = self.stdout.next_line() => ReceiveOutcome::Stdout(line),
                writer = writer_task => ReceiveOutcome::Writer(writer),
            }
        } else {
            ReceiveOutcome::Stdout(self.stdout.next_line().await)
        };

        let line = match outcome {
            ReceiveOutcome::Stdout(line) => line,
            ReceiveOutcome::Writer(writer) => {
                self.writer_task.take();
                return match writer {
                    Ok(Ok(())) => Err(anyhow::anyhow!("bridge stdin writer stopped unexpectedly")),
                    Ok(Err(err)) => Err(err.context("bridge stdin writer failed")),
                    Err(err) => {
                        Err(anyhow::Error::new(err).context("bridge stdin writer panicked"))
                    }
                };
            }
        };
        let Some(line) = line.map_err(|err| {
            tracing::error!(
                target: crate::logging::targets::BRIDGE_PROTOCOL,
                event_name = "bridge_stdout_read_failed",
                message = "failed to read bridge stdout",
                outcome = "failure",
                error = %err,
            );
            anyhow::Error::new(err).context("failed to read bridge stdout")
        })?
        else {
            return Ok(None);
        };
        let size_bytes = line.len() + 1;
        let event: EventEnvelope = serde_json::from_str(&line).map_err(|err| {
            let preview = line.chars().take(240).collect::<String>();
            tracing::error!(
                target: crate::logging::targets::BRIDGE_PROTOCOL,
                event_name = "bridge_event_decode_failed",
                message = "failed to decode bridge event json",
                outcome = "failure",
                size_bytes,
                preview = %preview,
                preview_chars = preview.chars().count(),
                error = %err,
            );
            anyhow::Error::new(err).context("failed to decode bridge event json")
        })?;
        log_bridge_event_received(&event, size_bytes);
        Ok(Some(event))
    }

    pub async fn shutdown(&mut self) -> anyhow::Result<()> {
        tracing::info!(
            target: crate::logging::targets::BRIDGE_LIFECYCLE,
            event_name = "bridge_shutdown_requested",
            message = "requesting bridge shutdown",
            outcome = "start",
        );
        self.send(CommandEnvelope { request_id: None, command: BridgeCommand::Shutdown }).await
    }

    pub async fn shutdown_and_wait(
        mut self,
        graceful_timeout: Duration,
    ) -> anyhow::Result<BridgeShutdownOutcome> {
        let bridge_pid = self.child.id().unwrap_or_default();
        let graceful_deadline = tokio::time::Instant::now() + graceful_timeout;
        let shutdown_result = tokio::time::timeout_at(graceful_deadline, self.shutdown()).await;
        if let Err(err) = shutdown_result
            .map_err(|_| anyhow::anyhow!("timed out while queueing bridge shutdown command"))
            .and_then(std::convert::identity)
        {
            tracing::warn!(
                target: crate::logging::targets::BRIDGE_LIFECYCLE,
                event_name = "bridge_shutdown_request_failed",
                message = "failed to request graceful bridge shutdown",
                outcome = "degraded",
                bridge_pid,
                error = %err,
            );
        }

        if let Ok(status) = tokio::time::timeout_at(graceful_deadline, self.child.wait()).await {
            let status = status.context("failed to wait for bridge process")?;
            self.stop_writer().await;
            tracing::info!(
                target: crate::logging::targets::BRIDGE_LIFECYCLE,
                event_name = "bridge_shutdown_completed",
                message = "bridge process exited after graceful shutdown request",
                outcome = "success",
                bridge_pid,
                exit_status = %status,
                forced = false,
            );
            Ok(BridgeShutdownOutcome::Graceful(status))
        } else {
            let timeout_ms = u64::try_from(graceful_timeout.as_millis()).unwrap_or(u64::MAX);
            tracing::warn!(
                target: crate::logging::targets::BRIDGE_LIFECYCLE,
                event_name = "bridge_shutdown_timed_out",
                message = "bridge process did not exit before the shutdown deadline",
                outcome = "timeout",
                bridge_pid,
                timeout_ms,
            );
            if let Err(err) = self.child.start_kill() {
                tracing::error!(
                    target: crate::logging::targets::BRIDGE_LIFECYCLE,
                    event_name = "bridge_force_kill_failed",
                    message = "failed to force-kill bridge process",
                    outcome = "failure",
                    bridge_pid,
                    error = %err,
                );
            }
            let status = tokio::time::timeout(graceful_timeout, self.child.wait())
                .await
                .context("timed out while reaping bridge process after kill")?
                .context("failed to reap bridge process after kill")?;
            self.stop_writer().await;
            tracing::info!(
                target: crate::logging::targets::BRIDGE_LIFECYCLE,
                event_name = "bridge_shutdown_completed",
                message = "bridge process was force-killed and reaped",
                outcome = "success",
                bridge_pid,
                exit_status = %status,
                forced = true,
            );
            Ok(BridgeShutdownOutcome::Forced(status))
        }
    }

    pub async fn wait(mut self) -> anyhow::Result<std::process::ExitStatus> {
        let status = self.child.wait().await.context("failed to wait for bridge process");
        self.stop_writer().await;
        status
    }

    async fn stop_writer(&mut self) {
        if let Some(writer_task) = self.writer_task.take() {
            writer_task.abort();
            let _ = writer_task.await;
        }
    }
}

impl Drop for BridgeClient {
    fn drop(&mut self) {
        if let Some(writer_task) = self.writer_task.take() {
            writer_task.abort();
        }
    }
}

async fn run_command_writer(
    mut stdin: BufWriter<ChildStdin>,
    mut command_rx: CommandReceiver,
) -> anyhow::Result<()> {
    while let Some(command) = command_rx.recv().await {
        let QueuedCommand { envelope, write_ack, _lane_permit } = command;
        match write_command_envelope(&mut stdin, &envelope).await {
            Ok(()) => {
                if let Some(write_ack) = write_ack {
                    let _ = write_ack.send(Ok(()));
                }
            }
            Err(err) => {
                if let Some(write_ack) = write_ack {
                    let _ = write_ack.send(Err(err.to_string()));
                }
                return Err(err);
            }
        }
    }
    stdin.shutdown().await.context("failed to close bridge stdin after command channel closed")
}

async fn write_command_envelope<W>(writer: &mut W, envelope: &CommandEnvelope) -> anyhow::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let request_id = envelope.request_id.as_deref().unwrap_or("");
    let bridge_command = envelope.command.command_name();
    let session_id = envelope.command.session_id().unwrap_or("");
    let tool_call_id = envelope.command.tool_call_id().unwrap_or("");
    let line = serde_json::to_string(envelope).map_err(|err| {
        tracing::error!(
            target: crate::logging::targets::BRIDGE_PROTOCOL,
            event_name = "bridge_command_send_failed",
            message = "failed to serialize bridge command",
            outcome = "failure",
            request_id,
            bridge_command,
            session_id,
            tool_call_id,
            stage = "serialize",
            error = %err,
        );
        anyhow::Error::new(err).context("failed to serialize bridge command")
    })?;
    let size_bytes = line.len() + 1;
    write_command_line(writer, &line).await.map_err(|err| {
        tracing::error!(
            target: crate::logging::targets::BRIDGE_PROTOCOL,
            event_name = "bridge_command_send_failed",
            message = "failed to write bridge command line",
            outcome = "failure",
            request_id,
            bridge_command,
            session_id,
            tool_call_id,
            size_bytes,
            stage = err.stage,
            error = %err,
        );
        anyhow::Error::new(err).context("failed to write bridge command line")
    })?;
    log_bridge_command_sent(bridge_command, request_id, session_id, tool_call_id, size_bytes);
    Ok(())
}

async fn write_command_line<W>(writer: &mut W, line: &str) -> Result<(), BridgeCommandWriteError>
where
    W: AsyncWrite + Unpin,
{
    writer
        .write_all(line.as_bytes())
        .await
        .map_err(|source| BridgeCommandWriteError { stage: "write_payload", source })?;
    writer
        .write_all(b"\n")
        .await
        .map_err(|source| BridgeCommandWriteError { stage: "write_newline", source })?;
    writer.flush().await.map_err(|source| BridgeCommandWriteError { stage: "flush", source })
}

#[derive(Debug)]
struct BridgeCommandWriteError {
    stage: &'static str,
    source: std::io::Error,
}

impl fmt::Display for BridgeCommandWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.stage, self.source)
    }
}

impl std::error::Error for BridgeCommandWriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

fn log_bridge_command_sent(
    bridge_command: &str,
    request_id: &str,
    session_id: &str,
    tool_call_id: &str,
    size_bytes: usize,
) {
    match bridge_command {
        "initialize" | "create_session" | "resume_session" | "new_session" | "shutdown" => {
            tracing::info!(
                target: crate::logging::targets::BRIDGE_PROTOCOL,
                event_name = "bridge_command_sent",
                message = "bridge command sent",
                outcome = "success",
                bridge_command,
                request_id,
                session_id,
                tool_call_id,
                size_bytes,
            );
        }
        _ => {
            tracing::debug!(
                target: crate::logging::targets::BRIDGE_PROTOCOL,
                event_name = "bridge_command_sent",
                message = "bridge command sent",
                outcome = "success",
                bridge_command,
                request_id,
                session_id,
                tool_call_id,
                size_bytes,
            );
        }
    }
}

fn log_bridge_event_received(envelope: &EventEnvelope, size_bytes: usize) {
    let bridge_event = envelope.event.event_name();
    let request_id = envelope.request_id.as_deref().unwrap_or("");
    let session_id = envelope.event.session_id().unwrap_or("");
    let tool_call_id = envelope.event.tool_call_id().unwrap_or("");

    match bridge_event {
        "initialized" | "connected" | "session_replaced" => tracing::info!(
            target: crate::logging::targets::BRIDGE_PROTOCOL,
            event_name = "bridge_event_received",
            message = "bridge event received",
            outcome = "success",
            bridge_event,
            request_id,
            session_id,
            tool_call_id,
            size_bytes,
        ),
        "connection_failed" => tracing::error!(
            target: crate::logging::targets::BRIDGE_PROTOCOL,
            event_name = "bridge_event_received",
            message = "bridge event received",
            outcome = "failure",
            bridge_event,
            request_id,
            session_id,
            tool_call_id,
            size_bytes,
        ),
        "auth_required" | "turn_error" | "slash_error" | "mcp_operation_error" => tracing::warn!(
            target: crate::logging::targets::BRIDGE_PROTOCOL,
            event_name = "bridge_event_received",
            message = "bridge event received",
            outcome = "degraded",
            bridge_event,
            request_id,
            session_id,
            tool_call_id,
            size_bytes,
        ),
        "session_update"
        | "permission_request"
        | "question_request"
        | "elicitation_request"
        | "elicitation_complete"
        | "mcp_auth_redirect"
        | "turn_complete" => tracing::trace!(
            target: crate::logging::targets::BRIDGE_PROTOCOL,
            event_name = "bridge_event_received",
            message = "bridge event received",
            outcome = "success",
            bridge_event,
            request_id,
            session_id,
            tool_call_id,
            size_bytes,
        ),
        _ => tracing::debug!(
            target: crate::logging::targets::BRIDGE_PROTOCOL,
            event_name = "bridge_event_received",
            message = "bridge event received",
            outcome = "success",
            bridge_event,
            request_id,
            session_id,
            tool_call_id,
            size_bytes,
        ),
    }
}

#[derive(Clone)]
pub struct AgentConnection {
    command_tx: CommandSender,
}

/// Accepted regular commands retain FIFO position while consuming only the
/// regular admission budget.
const REGULAR_COMMAND_QUEUE_CAPACITY: usize = 64;
/// Control commands have independent admission capacity but share the same FIFO
/// so they cannot overtake commands that establish the state they control.
const CONTROL_COMMAND_QUEUE_CAPACITY: usize = 16;
const COMMAND_QUEUE_CAPACITY: usize =
    REGULAR_COMMAND_QUEUE_CAPACITY + CONTROL_COMMAND_QUEUE_CAPACITY;

#[derive(Clone)]
struct CommandSender {
    sender: mpsc::Sender<QueuedCommand>,
    regular_slots: Arc<Semaphore>,
    control_slots: Arc<Semaphore>,
}

pub(crate) struct CommandReceiver {
    receiver: mpsc::Receiver<QueuedCommand>,
}

struct QueuedCommand {
    envelope: CommandEnvelope,
    write_ack: Option<oneshot::Sender<Result<(), String>>>,
    _lane_permit: OwnedSemaphorePermit,
}

#[must_use]
pub(crate) fn command_channel() -> (AgentConnection, CommandReceiver) {
    let (sender, receiver) = mpsc::channel(COMMAND_QUEUE_CAPACITY);
    let command_tx = CommandSender {
        sender,
        regular_slots: Arc::new(Semaphore::new(REGULAR_COMMAND_QUEUE_CAPACITY)),
        control_slots: Arc::new(Semaphore::new(CONTROL_COMMAND_QUEUE_CAPACITY)),
    };
    (AgentConnection { command_tx }, CommandReceiver { receiver })
}

impl CommandReceiver {
    async fn recv(&mut self) -> Option<QueuedCommand> {
        self.receiver.recv().await
    }

    #[cfg(test)]
    pub(crate) fn try_recv(&mut self) -> Result<CommandEnvelope, mpsc::error::TryRecvError> {
        self.receiver.try_recv().map(|queued| queued.envelope)
    }

    #[cfg(test)]
    pub(crate) async fn recv_envelope(&mut self) -> Option<CommandEnvelope> {
        self.recv().await.map(|queued| queued.envelope)
    }
}

#[derive(Debug, Clone)]
pub struct PromptResponse {
    pub stop_reason: String,
}

impl AgentConnection {
    #[cfg(test)]
    #[must_use]
    pub(crate) fn test_channel() -> (Self, CommandReceiver) {
        command_channel()
    }

    /// Convenience wrapper for text-only prompts. Prefer `prompt_with_images`
    /// for new call sites that may need image support.
    pub fn prompt_text(&self, session_id: String, text: String) -> anyhow::Result<PromptResponse> {
        self.prompt_with_images(session_id, text, Vec::new())
    }

    pub fn prompt_with_images(
        &self,
        session_id: String,
        text: String,
        images: Vec<crate::app::clipboard_image::ImageAttachment>,
    ) -> anyhow::Result<PromptResponse> {
        let mut chunks = Vec::with_capacity(1 + images.len());

        // Add image chunks first (convention: images before text).
        for img in images {
            if let Err(reason) =
                crate::app::clipboard_image::validate_image(&img.data, &img.mime_type)
            {
                tracing::warn!("prompt_with_images: skipping invalid image: {reason}");
                continue;
            }
            chunks.push(crate::agent::types::PromptChunk {
                kind: "image".to_owned(),
                value: serde_json::json!({
                    "data": img.data,
                    "mime_type": img.mime_type,
                }),
            });
        }

        // Add text chunk.
        chunks.push(crate::agent::types::PromptChunk {
            kind: "text".to_owned(),
            value: serde_json::Value::String(text),
        });

        self.send(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::Prompt { session_id, chunks },
        })?;
        Ok(PromptResponse { stop_reason: "end_turn".to_owned() })
    }

    pub fn cancel(&self, session_id: String) -> anyhow::Result<()> {
        self.send(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::CancelTurn { session_id },
        })
    }

    pub fn set_mode(&self, session_id: String, mode: String) -> anyhow::Result<()> {
        self.send(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::SetMode { session_id, mode },
        })
    }

    pub fn set_effort(&self, session_id: String, effort: String) -> anyhow::Result<()> {
        self.send(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::SetEffort { session_id, effort },
        })
    }

    pub fn set_agent(&self, session_id: String, agent: Option<String>) -> anyhow::Result<()> {
        self.send(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::SetAgent { session_id, agent },
        })
    }

    pub fn set_fast_mode(&self, session_id: String, enabled: bool) -> anyhow::Result<()> {
        self.send(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::SetFastMode { session_id, enabled },
        })
    }

    pub fn generate_session_title(
        &self,
        session_id: String,
        description: String,
    ) -> anyhow::Result<()> {
        self.send(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::GenerateSessionTitle { session_id, description },
        })
    }

    pub fn rename_session(&self, session_id: String, title: String) -> anyhow::Result<()> {
        self.send(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::RenameSession { session_id, title },
        })
    }

    pub fn set_model(&self, session_id: String, model: String) -> anyhow::Result<()> {
        self.send(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::SetModel { session_id, model },
        })
    }

    pub fn get_status_snapshot(&self, session_id: String) -> anyhow::Result<()> {
        self.send(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::GetStatusSnapshot { session_id },
        })
    }

    pub fn get_context_usage(&self, session_id: String) -> anyhow::Result<()> {
        self.send(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::GetContextUsage { session_id },
        })
    }

    pub fn get_rewind_targets(&self, session_id: String) -> anyhow::Result<()> {
        self.send(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::GetRewindTargets { session_id },
        })
    }

    pub fn rewind(
        &self,
        session_id: String,
        target_user_message_id: String,
        restore_mode: crate::agent::types::RewindRestoreMode,
        launch_settings: SessionLaunchSettings,
    ) -> anyhow::Result<()> {
        self.send(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::Rewind {
                session_id,
                target_user_message_id,
                restore_mode,
                launch_settings,
            },
        })
    }

    pub fn reload_plugins(&self, session_id: String) -> anyhow::Result<()> {
        self.send(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::ReloadPlugins { session_id },
        })
    }

    pub fn get_mcp_snapshot(&self, session_id: String) -> anyhow::Result<()> {
        self.send(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::GetMcpSnapshot { session_id },
        })
    }

    pub async fn respond_to_elicitation(
        &self,
        session_id: String,
        elicitation_request_id: String,
        action: crate::agent::types::ElicitationAction,
        content: Option<serde_json::Value>,
    ) -> anyhow::Result<()> {
        self.send_reliable_control(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::ElicitationResponse {
                session_id,
                elicitation_request_id,
                action,
                content,
            },
        })
        .await
    }

    pub(crate) async fn respond_to_permission(
        &self,
        session_id: String,
        tool_call_id: String,
        outcome: crate::agent::types::PermissionOutcome,
    ) -> anyhow::Result<()> {
        self.send_reliable_control(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::PermissionResponse { session_id, tool_call_id, outcome },
        })
        .await
    }

    pub(crate) async fn respond_to_question(
        &self,
        session_id: String,
        tool_call_id: String,
        outcome: crate::agent::types::QuestionOutcome,
    ) -> anyhow::Result<()> {
        self.send_reliable_control(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::QuestionResponse { session_id, tool_call_id, outcome },
        })
        .await
    }

    pub(crate) async fn respond_to_user_dialog(
        &self,
        session_id: String,
        request_id: String,
        outcome: crate::agent::types::UserDialogOutcome,
    ) -> anyhow::Result<()> {
        self.send_reliable_control(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::UserDialogResponse { session_id, request_id, outcome },
        })
        .await
    }

    pub fn reconnect_mcp_server(
        &self,
        session_id: String,
        server_name: String,
    ) -> anyhow::Result<()> {
        self.send(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::McpReconnect { session_id, server_name },
        })
    }

    pub fn toggle_mcp_server(
        &self,
        session_id: String,
        server_name: String,
        enabled: bool,
    ) -> anyhow::Result<()> {
        self.send(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::McpToggle { session_id, server_name, enabled },
        })
    }

    pub fn set_mcp_servers(
        &self,
        session_id: String,
        servers: std::collections::BTreeMap<String, crate::agent::types::McpServerConfig>,
    ) -> anyhow::Result<()> {
        self.send(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::McpSetServers { session_id, servers },
        })
    }

    pub fn authenticate_mcp_server(
        &self,
        session_id: String,
        server_name: String,
    ) -> anyhow::Result<()> {
        self.send(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::McpAuthenticate { session_id, server_name },
        })
    }

    pub fn clear_mcp_auth(&self, session_id: String, server_name: String) -> anyhow::Result<()> {
        self.send(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::McpClearAuth { session_id, server_name },
        })
    }

    pub fn submit_mcp_oauth_callback_url(
        &self,
        session_id: String,
        server_name: String,
        callback_url: String,
    ) -> anyhow::Result<()> {
        self.send(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::McpOauthCallbackUrl { session_id, server_name, callback_url },
        })
    }

    pub fn new_session(
        &self,
        cwd: String,
        launch_settings: SessionLaunchSettings,
    ) -> anyhow::Result<()> {
        self.send(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::NewSession { cwd, launch_settings },
        })
    }

    pub fn resume_session(
        &self,
        session_id: String,
        launch_settings: SessionLaunchSettings,
    ) -> anyhow::Result<()> {
        self.send(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::ResumeSession {
                session_id,
                launch_settings,
                metadata: std::collections::BTreeMap::new(),
            },
        })
    }

    fn send(&self, envelope: CommandEnvelope) -> anyhow::Result<()> {
        self.command_tx.try_send(envelope, None)
    }

    async fn send_reliable_control(&self, envelope: CommandEnvelope) -> anyhow::Result<()> {
        debug_assert!(is_control_command(&envelope.command));
        self.command_tx.send_reliable(envelope, None).await
    }
}

impl CommandSender {
    fn lane(&self, command: &BridgeCommand) -> (&Arc<Semaphore>, &'static str, usize) {
        if is_control_command(command) {
            (&self.control_slots, "control", CONTROL_COMMAND_QUEUE_CAPACITY)
        } else {
            (&self.regular_slots, "regular", REGULAR_COMMAND_QUEUE_CAPACITY)
        }
    }

    fn try_send(
        &self,
        envelope: CommandEnvelope,
        write_ack: Option<oneshot::Sender<Result<(), String>>>,
    ) -> anyhow::Result<()> {
        let (slots, lane, capacity) = self.lane(&envelope.command);
        let permit = Arc::clone(slots).try_acquire_owned().map_err(|err| {
            self.log_saturation(&envelope, lane, capacity, &err.to_string());
            anyhow::anyhow!("bridge {lane} command queue is full (capacity {capacity})")
        })?;
        match self.sender.try_send(QueuedCommand { envelope, write_ack, _lane_permit: permit }) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(queued)) => {
                self.log_saturation(
                    &queued.envelope,
                    lane,
                    capacity,
                    "combined command queue unexpectedly full",
                );
                Err(anyhow::anyhow!("bridge command queue is full"))
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                Err(anyhow::anyhow!("bridge command channel closed"))
            }
        }
    }

    async fn send_reliable(
        &self,
        envelope: CommandEnvelope,
        write_ack: Option<oneshot::Sender<Result<(), String>>>,
    ) -> anyhow::Result<()> {
        let (slots, lane, capacity) = self.lane(&envelope.command);
        let permit = Arc::clone(slots)
            .acquire_owned()
            .await
            .map_err(|_| anyhow::anyhow!("bridge {lane} command admission closed"))?;
        self.sender.send(QueuedCommand { envelope, write_ack, _lane_permit: permit }).await.map_err(
            |_| anyhow::anyhow!("bridge command channel closed (lane {lane}, capacity {capacity})"),
        )
    }

    fn log_saturation(
        &self,
        envelope: &CommandEnvelope,
        lane: &str,
        capacity: usize,
        reason: &str,
    ) {
        tracing::warn!(
            target: crate::logging::targets::BRIDGE_PROTOCOL,
            event_name = "bridge_command_queue_saturated",
            message = "bridge command rejected because its bounded admission lane is full",
            outcome = "rejected",
            bridge_command = envelope.command.command_name(),
            queue_lane = lane,
            queue_capacity = capacity,
            combined_available_capacity = self.sender.capacity(),
            reason,
        );
    }
}

fn is_control_command(command: &BridgeCommand) -> bool {
    matches!(
        command,
        BridgeCommand::CancelTurn { .. }
            | BridgeCommand::PermissionResponse { .. }
            | BridgeCommand::QuestionResponse { .. }
            | BridgeCommand::UserDialogResponse { .. }
            | BridgeCommand::ElicitationResponse { .. }
            | BridgeCommand::Shutdown
    )
}

#[cfg(test)]
mod tests {
    use super::{
        AgentConnection, CONTROL_COMMAND_QUEUE_CAPACITY, REGULAR_COMMAND_QUEUE_CAPACITY,
        command_channel, write_command_line,
    };
    use crate::agent::types::ElicitationAction;
    use crate::agent::wire::BridgeCommand;
    use std::collections::BTreeMap;
    use std::time::Duration;
    use tokio::io::AsyncReadExt as _;

    #[tokio::test]
    async fn control_commands_keep_reserved_capacity_when_regular_lane_is_full() {
        let (connection, mut receiver) = command_channel();
        for index in 0..REGULAR_COMMAND_QUEUE_CAPACITY {
            connection
                .set_model("session-1".to_owned(), format!("model-{index}"))
                .expect("regular command should fit");
        }
        let error = connection
            .set_model("session-1".to_owned(), "overflow".to_owned())
            .expect_err("regular command lane should be bounded");
        assert!(error.to_string().contains("regular command queue is full"));

        connection.cancel("session-1".to_owned()).expect("control lane should remain available");
        for index in 0..REGULAR_COMMAND_QUEUE_CAPACITY {
            let command = receiver.recv_envelope().await.expect("regular command");
            assert!(matches!(
                command.command,
                BridgeCommand::SetModel { model, .. } if model == format!("model-{index}")
            ));
        }
        let command = receiver.recv_envelope().await.expect("control command");
        assert!(matches!(command.command, BridgeCommand::CancelTurn { .. }));
    }

    #[tokio::test]
    async fn cancel_does_not_overtake_the_prompt_it_targets() {
        let (connection, mut receiver) = command_channel();
        connection.prompt_text("session-1".to_owned(), "hello".to_owned()).expect("prompt");
        connection.cancel("session-1".to_owned()).expect("cancel");

        let prompt = receiver.recv_envelope().await.expect("prompt command");
        let cancel = receiver.recv_envelope().await.expect("cancel command");

        assert!(matches!(prompt.command, BridgeCommand::Prompt { .. }));
        assert!(matches!(cancel.command, BridgeCommand::CancelTurn { .. }));
    }

    #[test]
    fn control_command_lane_is_also_bounded() {
        let (connection, _receiver) = command_channel();
        for _ in 0..CONTROL_COMMAND_QUEUE_CAPACITY {
            connection.cancel("session-1".to_owned()).expect("control command should fit");
        }
        let error = connection
            .cancel("session-1".to_owned())
            .expect_err("control command lane should be bounded");
        assert!(error.to_string().contains("control command queue is full"));
    }

    #[tokio::test]
    async fn interactive_response_waits_for_control_capacity_instead_of_being_lost() {
        let (connection, mut receiver) = command_channel();
        for index in 0..CONTROL_COMMAND_QUEUE_CAPACITY {
            connection.cancel(format!("session-{index}")).expect("control command should fit");
        }

        let response = connection.respond_to_elicitation(
            "session-response".to_owned(),
            "elicitation-1".to_owned(),
            ElicitationAction::Decline,
            None,
        );
        tokio::pin!(response);
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut response).await.is_err(),
            "response must wait while the reserved control budget is exhausted"
        );

        let first = receiver.recv_envelope().await.expect("first cancel");
        assert!(matches!(
            first.command,
            BridgeCommand::CancelTurn { session_id } if session_id == "session-0"
        ));
        response.await.expect("response should enter the queue after capacity is released");

        for index in 1..CONTROL_COMMAND_QUEUE_CAPACITY {
            let queued = receiver.recv_envelope().await.expect("queued cancel");
            assert!(matches!(
                queued.command,
                BridgeCommand::CancelTurn { session_id }
                    if session_id == format!("session-{index}")
            ));
        }
        let queued = receiver.recv_envelope().await.expect("elicitation response");
        assert!(matches!(
            queued.command,
            BridgeCommand::ElicitationResponse {
                session_id,
                elicitation_request_id,
                action: ElicitationAction::Decline,
                content: None,
            } if session_id == "session-response" && elicitation_request_id == "elicitation-1"
        ));
    }

    #[test]
    fn generate_session_title_sends_bridge_command() {
        let (conn, mut rx) = AgentConnection::test_channel();

        conn.generate_session_title("session-1".to_owned(), "Summarize work".to_owned())
            .expect("generate");

        let envelope = rx.try_recv().expect("command");
        assert_eq!(
            envelope.command,
            BridgeCommand::GenerateSessionTitle {
                session_id: "session-1".to_owned(),
                description: "Summarize work".to_owned(),
            }
        );
    }

    #[tokio::test]
    async fn write_command_line_appends_exactly_one_newline() {
        let (mut writer, mut reader) = tokio::io::duplex(128);

        write_command_line(&mut writer, r#"{"command":"cancel_turn","session_id":"s1"}"#)
            .await
            .expect("write command");
        drop(writer);

        let mut output = Vec::new();
        reader.read_to_end(&mut output).await.expect("read command");

        assert_eq!(
            String::from_utf8(output).expect("utf8"),
            "{\"command\":\"cancel_turn\",\"session_id\":\"s1\"}\n"
        );
    }

    #[test]
    fn rename_session_sends_bridge_command() {
        let (conn, mut rx) = AgentConnection::test_channel();

        conn.rename_session("session-1".to_owned(), "Renamed".to_owned()).expect("rename");

        let envelope = rx.try_recv().expect("command");
        assert_eq!(
            envelope.command,
            BridgeCommand::RenameSession {
                session_id: "session-1".to_owned(),
                title: "Renamed".to_owned(),
            }
        );
    }

    #[test]
    fn set_effort_sends_bridge_command() {
        let (conn, mut rx) = AgentConnection::test_channel();

        conn.set_effort("session-1".to_owned(), "max".to_owned()).expect("set effort");

        let envelope = rx.try_recv().expect("command");
        assert_eq!(
            envelope.command,
            BridgeCommand::SetEffort {
                session_id: "session-1".to_owned(),
                effort: "max".to_owned(),
            }
        );
    }

    #[test]
    fn set_agent_sends_bridge_command() {
        let (conn, mut rx) = AgentConnection::test_channel();

        conn.set_agent("session-1".to_owned(), Some("reviewer".to_owned())).expect("set agent");

        let envelope = rx.try_recv().expect("command");
        assert_eq!(
            envelope.command,
            BridgeCommand::SetAgent {
                session_id: "session-1".to_owned(),
                agent: Some("reviewer".to_owned()),
            }
        );
    }

    #[test]
    fn set_agent_reset_sends_bridge_command() {
        let (conn, mut rx) = AgentConnection::test_channel();

        conn.set_agent("session-1".to_owned(), None).expect("reset agent");

        let envelope = rx.try_recv().expect("command");
        assert_eq!(
            envelope.command,
            BridgeCommand::SetAgent { session_id: "session-1".to_owned(), agent: None }
        );
    }

    #[test]
    fn set_fast_mode_sends_bridge_command() {
        let (conn, mut rx) = AgentConnection::test_channel();

        conn.set_fast_mode("session-1".to_owned(), true).expect("set fast mode");

        let envelope = rx.try_recv().expect("command");
        assert_eq!(
            envelope.command,
            BridgeCommand::SetFastMode { session_id: "session-1".to_owned(), enabled: true }
        );
    }

    #[test]
    fn get_mcp_snapshot_sends_bridge_command() {
        let (conn, mut rx) = AgentConnection::test_channel();

        conn.get_mcp_snapshot("session-1".to_owned()).expect("mcp snapshot");

        let envelope = rx.try_recv().expect("command");
        assert_eq!(
            envelope.command,
            BridgeCommand::GetMcpSnapshot { session_id: "session-1".to_owned() }
        );
    }

    #[test]
    fn get_context_usage_sends_bridge_command() {
        let (conn, mut rx) = AgentConnection::test_channel();

        conn.get_context_usage("session-1".to_owned()).expect("context usage");

        let envelope = rx.try_recv().expect("command");
        assert_eq!(
            envelope.command,
            BridgeCommand::GetContextUsage { session_id: "session-1".to_owned() }
        );
    }

    #[test]
    fn reload_plugins_sends_bridge_command() {
        let (conn, mut rx) = AgentConnection::test_channel();

        conn.reload_plugins("session-1".to_owned()).expect("reload plugins");

        let envelope = rx.try_recv().expect("command");
        assert_eq!(
            envelope.command,
            BridgeCommand::ReloadPlugins { session_id: "session-1".to_owned() }
        );
    }

    #[test]
    fn get_rewind_targets_sends_bridge_command() {
        let (conn, mut rx) = AgentConnection::test_channel();

        conn.get_rewind_targets("session-1".to_owned()).expect("rewind targets");

        let envelope = rx.try_recv().expect("command");
        assert_eq!(
            envelope.command,
            BridgeCommand::GetRewindTargets { session_id: "session-1".to_owned() }
        );
    }

    #[test]
    fn rewind_sends_bridge_command() {
        let (conn, mut rx) = AgentConnection::test_channel();

        conn.rewind(
            "session-1".to_owned(),
            "user-1".to_owned(),
            crate::agent::types::RewindRestoreMode::Code,
            crate::agent::wire::SessionLaunchSettings::default(),
        )
        .expect("rewind");

        let envelope = rx.try_recv().expect("command");
        assert_eq!(
            envelope.command,
            BridgeCommand::Rewind {
                session_id: "session-1".to_owned(),
                target_user_message_id: "user-1".to_owned(),
                restore_mode: crate::agent::types::RewindRestoreMode::Code,
                launch_settings: crate::agent::wire::SessionLaunchSettings::default(),
            }
        );
    }

    #[test]
    fn reconnect_mcp_server_sends_bridge_command() {
        let (conn, mut rx) = AgentConnection::test_channel();

        conn.reconnect_mcp_server("session-1".to_owned(), "notion".to_owned())
            .expect("mcp reconnect");

        let envelope = rx.try_recv().expect("command");
        assert_eq!(
            envelope.command,
            BridgeCommand::McpReconnect {
                session_id: "session-1".to_owned(),
                server_name: "notion".to_owned(),
            }
        );
    }

    #[test]
    fn toggle_mcp_server_sends_bridge_command() {
        let (conn, mut rx) = AgentConnection::test_channel();

        conn.toggle_mcp_server("session-1".to_owned(), "notion".to_owned(), false)
            .expect("mcp toggle");

        let envelope = rx.try_recv().expect("command");
        assert_eq!(
            envelope.command,
            BridgeCommand::McpToggle {
                session_id: "session-1".to_owned(),
                server_name: "notion".to_owned(),
                enabled: false,
            }
        );
    }

    #[test]
    fn set_mcp_servers_sends_bridge_command() {
        let (conn, mut rx) = AgentConnection::test_channel();
        let servers = BTreeMap::from([(
            "notion".to_owned(),
            crate::agent::types::McpServerConfig::Http {
                url: "https://mcp.notion.com/mcp".to_owned(),
                headers: BTreeMap::new(),
                tools: Vec::new(),
                timeout: Some(5000),
                request_timeout_ms: Some(30000),
                always_load: Some(true),
            },
        )]);

        conn.set_mcp_servers("session-1".to_owned(), servers.clone()).expect("mcp set servers");

        let envelope = rx.try_recv().expect("command");
        assert_eq!(
            envelope.command,
            BridgeCommand::McpSetServers { session_id: "session-1".to_owned(), servers }
        );
    }

    #[tokio::test]
    async fn respond_to_elicitation_sends_bridge_command() {
        let (conn, mut rx) = AgentConnection::test_channel();

        conn.respond_to_elicitation(
            "session-1".to_owned(),
            "elicitation-1".to_owned(),
            ElicitationAction::Accept,
            None,
        )
        .await
        .expect("elicitation response");

        let envelope = rx.try_recv().expect("command");
        assert_eq!(
            envelope.command,
            BridgeCommand::ElicitationResponse {
                session_id: "session-1".to_owned(),
                elicitation_request_id: "elicitation-1".to_owned(),
                action: ElicitationAction::Accept,
                content: None,
            }
        );
    }
}
