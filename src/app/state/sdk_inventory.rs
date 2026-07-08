// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::agent::model;

/// SDK-provided inventories and session-history caches surfaced by the app.
#[derive(Default)]
pub struct SdkInventoryState {
    /// Current SDK task state from `TaskCreate`/`TaskUpdate`/`TaskGet`/`TaskList`.
    pub tasks: Vec<model::TaskItem>,
    /// Commands advertised by the agent via `AvailableCommandsUpdate`.
    pub available_commands: Vec<model::AvailableCommand>,
    /// Rewind candidates loaded from persisted SDK session history.
    pub rewind_targets: Vec<model::RewindTarget>,
    /// Session id that owns `rewind_targets`.
    pub rewind_targets_session_id: Option<model::SessionId>,
    /// True while a rewind target refresh request is in flight.
    pub rewind_targets_in_flight: bool,
    /// Subagents advertised by the agent via `AvailableAgentsUpdate`.
    pub available_agents: Vec<model::AvailableAgent>,
    /// Models advertised by the agent SDK for the active session.
    pub available_models: Vec<model::AvailableModel>,
}

impl SdkInventoryState {
    pub fn clear_rewind_targets(&mut self) {
        self.rewind_targets.clear();
        self.rewind_targets_session_id = None;
        self.rewind_targets_in_flight = false;
    }
}
