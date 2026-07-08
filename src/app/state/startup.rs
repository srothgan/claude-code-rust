// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

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
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum StartupLaunch {
    #[default]
    NewSession,
    ResumeSession {
        session_id: String,
    },
    SessionPicker,
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
    pub fn new(
        bridge_script: Option<PathBuf>,
        resume_id: Option<String>,
        session_picker_requested: bool,
    ) -> Self {
        let launch = if session_picker_requested {
            StartupLaunch::SessionPicker
        } else if let Some(session_id) = resume_id {
            StartupLaunch::ResumeSession { session_id }
        } else {
            StartupLaunch::NewSession
        };

        Self { bridge_script, launch, phase: StartupPhase::AwaitingConnection }
    }

    #[must_use]
    pub fn bridge_script(&self) -> Option<&PathBuf> {
        self.bridge_script.as_ref()
    }

    #[must_use]
    pub fn resume_id(&self) -> Option<&str> {
        match &self.launch {
            StartupLaunch::ResumeSession { session_id } => Some(session_id.as_str()),
            StartupLaunch::NewSession | StartupLaunch::SessionPicker => None,
        }
    }

    #[must_use]
    pub fn resume_requested(&self) -> bool {
        matches!(self.launch, StartupLaunch::ResumeSession { .. })
    }

    #[must_use]
    pub fn session_picker_requested(&self) -> bool {
        matches!(self.launch, StartupLaunch::SessionPicker)
    }

    #[must_use]
    pub fn connection_requested(&self) -> bool {
        matches!(self.phase, StartupPhase::ConnectionRequested | StartupPhase::ConnectionStarted(_))
    }

    #[must_use]
    pub fn connection_started(&self) -> bool {
        matches!(self.phase, StartupPhase::ConnectionStarted(_))
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
    pub fn recent_sessions_loaded(&self) -> bool {
        matches!(
            self.phase,
            StartupPhase::ConnectionStarted(
                StartupConnectionPhase::PickerPending { recent_sessions_loaded: true }
                    | StartupConnectionPhase::PickerResolved
            )
        )
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
