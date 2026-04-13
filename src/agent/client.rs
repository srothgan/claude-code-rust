// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::agent::bridge::BridgeLauncher;
use crate::agent::wire::{BridgeCommand, CommandEnvelope, EventEnvelope, SessionLaunchSettings};
use crate::error::AppError;
use anyhow::Context as _;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, BufWriter};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::mpsc;
use tracing::{Instrument as _, info_span};

pub struct BridgeClient {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: tokio::io::Lines<BufReader<ChildStdout>>,
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
            .map_err(|_| anyhow::Error::new(AppError::AdapterCrashed))
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

        Ok(Self { child, stdin: BufWriter::new(stdin), stdout: BufReader::new(stdout).lines() })
    }

    fn spawn_stderr_logger(stderr: ChildStderr) {
        tokio::task::spawn_local(
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

    pub async fn send(&mut self, envelope: CommandEnvelope) -> anyhow::Result<()> {
        let request_id = envelope.request_id.as_deref().unwrap_or("");
        let bridge_command = envelope.command.command_name();
        let session_id = envelope.command.session_id().unwrap_or("");
        let tool_call_id = envelope.command.tool_call_id().unwrap_or("");

        let line = serde_json::to_string(&envelope).map_err(|err| {
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
        self.stdin.write_all(line.as_bytes()).await.map_err(|err| {
            tracing::error!(
                target: crate::logging::targets::BRIDGE_PROTOCOL,
                event_name = "bridge_command_send_failed",
                message = "failed to write bridge command",
                outcome = "failure",
                request_id,
                bridge_command,
                session_id,
                tool_call_id,
                size_bytes,
                stage = "write",
                error = %err,
            );
            anyhow::Error::new(err).context("failed to write bridge command")
        })?;
        self.stdin.write_all(b"\n").await.map_err(|err| {
            tracing::error!(
                target: crate::logging::targets::BRIDGE_PROTOCOL,
                event_name = "bridge_command_send_failed",
                message = "failed to write bridge newline",
                outcome = "failure",
                request_id,
                bridge_command,
                session_id,
                tool_call_id,
                size_bytes,
                stage = "write_newline",
                error = %err,
            );
            anyhow::Error::new(err).context("failed to write bridge newline")
        })?;
        self.stdin.flush().await.map_err(|err| {
            tracing::error!(
                target: crate::logging::targets::BRIDGE_PROTOCOL,
                event_name = "bridge_command_send_failed",
                message = "failed to flush bridge stdin",
                outcome = "failure",
                request_id,
                bridge_command,
                session_id,
                tool_call_id,
                size_bytes,
                stage = "flush",
                error = %err,
            );
            anyhow::Error::new(err).context("failed to flush bridge stdin")
        })?;
        log_bridge_command_sent(bridge_command, request_id, session_id, tool_call_id, size_bytes);
        Ok(())
    }

    pub async fn recv(&mut self) -> anyhow::Result<Option<EventEnvelope>> {
        let Some(line) = self.stdout.next_line().await.map_err(|err| {
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
        self.send(CommandEnvelope { request_id: None, command: BridgeCommand::Shutdown }).await?;
        Ok(())
    }

    pub async fn wait(mut self) -> anyhow::Result<std::process::ExitStatus> {
        self.child.wait().await.context("failed to wait for bridge process")
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
    command_tx: mpsc::UnboundedSender<CommandEnvelope>,
}

#[derive(Debug, Clone)]
pub struct PromptResponse {
    pub stop_reason: String,
}

impl AgentConnection {
    #[must_use]
    pub fn new(command_tx: mpsc::UnboundedSender<CommandEnvelope>) -> Self {
        Self { command_tx }
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

    pub fn respond_to_elicitation(
        &self,
        session_id: String,
        elicitation_request_id: String,
        action: crate::agent::types::ElicitationAction,
        content: Option<serde_json::Value>,
    ) -> anyhow::Result<()> {
        self.send(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::ElicitationResponse {
                session_id,
                elicitation_request_id,
                action,
                content,
            },
        })
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
        self.command_tx.send(envelope).map_err(|_| anyhow::anyhow!("bridge command channel closed"))
    }
}

#[cfg(test)]
mod tests {
    use super::AgentConnection;
    use crate::agent::types::ElicitationAction;
    use crate::agent::wire::BridgeCommand;
    use std::collections::BTreeMap;

    #[test]
    fn generate_session_title_sends_bridge_command() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = AgentConnection::new(tx);

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

    #[test]
    fn rename_session_sends_bridge_command() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = AgentConnection::new(tx);

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
    fn get_mcp_snapshot_sends_bridge_command() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = AgentConnection::new(tx);

        conn.get_mcp_snapshot("session-1".to_owned()).expect("mcp snapshot");

        let envelope = rx.try_recv().expect("command");
        assert_eq!(
            envelope.command,
            BridgeCommand::GetMcpSnapshot { session_id: "session-1".to_owned() }
        );
    }

    #[test]
    fn get_context_usage_sends_bridge_command() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = AgentConnection::new(tx);

        conn.get_context_usage("session-1".to_owned()).expect("context usage");

        let envelope = rx.try_recv().expect("command");
        assert_eq!(
            envelope.command,
            BridgeCommand::GetContextUsage { session_id: "session-1".to_owned() }
        );
    }

    #[test]
    fn reload_plugins_sends_bridge_command() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = AgentConnection::new(tx);

        conn.reload_plugins("session-1".to_owned()).expect("reload plugins");

        let envelope = rx.try_recv().expect("command");
        assert_eq!(
            envelope.command,
            BridgeCommand::ReloadPlugins { session_id: "session-1".to_owned() }
        );
    }

    #[test]
    fn reconnect_mcp_server_sends_bridge_command() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = AgentConnection::new(tx);

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
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = AgentConnection::new(tx);

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
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = AgentConnection::new(tx);
        let servers = BTreeMap::from([(
            "notion".to_owned(),
            crate::agent::types::McpServerConfig::Http {
                url: "https://mcp.notion.com/mcp".to_owned(),
                headers: BTreeMap::new(),
            },
        )]);

        conn.set_mcp_servers("session-1".to_owned(), servers.clone()).expect("mcp set servers");

        let envelope = rx.try_recv().expect("command");
        assert_eq!(
            envelope.command,
            BridgeCommand::McpSetServers { session_id: "session-1".to_owned(), servers }
        );
    }

    #[test]
    fn respond_to_elicitation_sends_bridge_command() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = AgentConnection::new(tx);

        conn.respond_to_elicitation(
            "session-1".to_owned(),
            "elicitation-1".to_owned(),
            ElicitationAction::Accept,
            None,
        )
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
