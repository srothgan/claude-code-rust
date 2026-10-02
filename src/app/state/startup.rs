// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use crate::StartupLaunch;
use std::path::PathBuf;

/// Bootstrap state resolved from CLI flags and consumed while the app
/// transitions from launch to a connected session.
///
/// CLI launch intent is immutable after construction. Runtime sequencing moves
/// through [`StartupPhase`], so callers cannot represent conflicting states
/// such as "connection started but not requested".
#[derive(Debug, Default)]
pub struct StartupState {
    /// Explicit bridge script path from `--bridge-script`.
    bridge_script: Option<PathBuf>,
    launch: StartupLaunch,
    phase: StartupPhase,
    session_options: Option<crate::SessionOptions>,
    initial_prompt: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StartupPhase {
    /// Waiting for trust resolution before a bridge connection may be started.
    #[default]
    AwaitingConnection,
    /// Trust is resolved and the bridge connection may be spawned.
    ConnectionRequested,
    /// The bridge task has been spawned. If startup requested the picker, the
    /// picker sub-phase tracks list loading and resolution.
    ConnectionStarted(StartupConnectionPhase),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupConnectionPhase {
    Running,
    PickerPending { recent_sessions_loaded: bool },
    PickerResolved,
}

impl StartupState {
    #[must_use]
    pub fn new(bridge_script: Option<PathBuf>, launch: StartupLaunch) -> Self {
        Self { bridge_script, launch, ..Self::default() }
    }

    #[must_use]
    pub fn from_cli(cli: &crate::Cli) -> Self {
        Self {
            session_options: Some(cli.session_options.clone()),
            initial_prompt: cli.initial_prompt().map(str::to_owned),
            ..Self::new(cli.bridge_script.clone(), cli.startup_launch())
        }
    }

    pub fn session_options(&self) -> Option<&crate::SessionOptions> {
        self.session_options.as_ref()
    }

    /// Release launch overrides once the initially selected session is ready.
    pub fn complete_launch(&mut self) {
        self.session_options = None;
    }

    pub fn take_initial_prompt(&mut self) -> Option<String> {
        if self.session_options.is_some() {
            return None;
        }
        self.initial_prompt.take()
    }

    pub fn launch(&self) -> &StartupLaunch {
        &self.launch
    }

    #[must_use]
    pub fn bridge_script(&self) -> Option<&PathBuf> {
        self.bridge_script.as_ref()
    }

    #[must_use]
    pub fn session_picker_requested(&self) -> bool {
        matches!(self.launch, StartupLaunch::SessionPicker)
    }

    pub fn request_connection(&mut self) {
        if matches!(self.phase, StartupPhase::AwaitingConnection) {
            self.phase = StartupPhase::ConnectionRequested;
        }
    }

    /// Mark the bridge connection as spawned.
    ///
    /// Returns `true` only for the valid transition from requested to started.
    pub fn mark_connection_started(&mut self) -> bool {
        if !matches!(self.phase, StartupPhase::ConnectionRequested) {
            return false;
        }

        self.phase = StartupPhase::ConnectionStarted(if self.session_picker_requested() {
            StartupConnectionPhase::PickerPending { recent_sessions_loaded: false }
        } else {
            StartupConnectionPhase::Running
        });
        true
    }

    pub fn mark_recent_sessions_loaded(&mut self) {
        if let StartupPhase::ConnectionStarted(StartupConnectionPhase::PickerPending {
            recent_sessions_loaded,
        }) = &mut self.phase
        {
            *recent_sessions_loaded = true;
        }
    }

    #[must_use]
    pub fn session_picker_resolved(&self) -> bool {
        matches!(
            self.phase,
            StartupPhase::ConnectionStarted(StartupConnectionPhase::PickerResolved)
        )
    }

    #[must_use]
    pub fn startup_picker_is_loading(&self, connection_ready: bool) -> bool {
        matches!(
            self.phase,
            StartupPhase::ConnectionStarted(StartupConnectionPhase::PickerPending {
                recent_sessions_loaded: false
            })
        ) || (self.session_picker_requested()
            && !self.session_picker_resolved()
            && !connection_ready)
    }

    #[must_use]
    pub fn startup_picker_is_ready(&self) -> bool {
        matches!(
            self.phase,
            StartupPhase::ConnectionStarted(StartupConnectionPhase::PickerPending {
                recent_sessions_loaded: true
            })
        )
    }

    pub fn resolve_session_picker(&mut self) {
        if matches!(
            self.phase,
            StartupPhase::ConnectionStarted(StartupConnectionPhase::PickerPending { .. })
        ) {
            self.phase = StartupPhase::ConnectionStarted(StartupConnectionPhase::PickerResolved);
        }
    }
}
