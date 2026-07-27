// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use crate::agent::client::AgentConnection;
use crate::agent::model;
use crate::app::state::{LoginHint, ModeState, SessionUsageState};
use std::collections::BTreeMap;
use std::rc::Rc;

/// State owned by the active SDK session/runtime boundary.
pub struct SessionRuntimeState {
    pub session_id: Option<model::SessionId>,
    /// Most recently established session, retained across live identity resets for the exit hint.
    last_resumable_session_id: Option<model::SessionId>,
    /// Agent connection handle. `None` while connecting (before bridge is ready).
    pub conn: Option<Rc<AgentConnection>>,
    /// Monotonic session authority epoch used to ignore stale async view data.
    pub session_scope_epoch: u64,
    pub current_model: Option<model::CurrentModel>,
    pub mode: Option<ModeState>,
    /// Latest config options observed from bridge `config_option_update` events.
    pub config_options: BTreeMap<String, serde_json::Value>,
    /// Login hint shown when authentication is required. Rendered above the input field.
    pub login_hint: Option<LoginHint>,
    /// Session-wide usage and cost telemetry from the bridge.
    pub session_usage: SessionUsageState,
    /// Fast mode state telemetry from the SDK.
    pub fast_mode_state: model::FastModeState,
    /// Open-set reason reported by the SDK when fast mode cannot activate.
    pub fast_mode_disabled_reason: Option<String>,
    /// Latest SDK runtime liveness state.
    pub runtime_session_state: Option<model::RuntimeSessionState>,
    /// Latest prompt suggestion from the SDK, shown in the input hint band.
    pub prompt_suggestion: Option<String>,
    /// Latest rate-limit telemetry from the SDK.
    pub last_rate_limit_update: Option<model::RateLimitUpdate>,
    /// Account info from the bridge status snapshot (email, org, subscription).
    pub account_info: Option<model::AccountInfo>,
}

impl Default for SessionRuntimeState {
    fn default() -> Self {
        Self {
            session_id: None,
            last_resumable_session_id: None,
            conn: None,
            session_scope_epoch: 0,
            current_model: None,
            mode: None,
            config_options: BTreeMap::new(),
            login_hint: None,
            session_usage: SessionUsageState::default(),
            fast_mode_state: model::FastModeState::Off,
            fast_mode_disabled_reason: None,
            runtime_session_state: None,
            prompt_suggestion: None,
            last_rate_limit_update: None,
            account_info: None,
        }
    }
}

impl SessionRuntimeState {
    #[must_use]
    pub fn test_default() -> Self {
        Self {
            current_model: Some(
                model::CurrentModel::new("test-model", "test-model", "test-model")
                    .authoritative(true),
            ),
            ..Self::default()
        }
    }

    pub fn bump_session_scope_epoch(&mut self) {
        self.session_scope_epoch = self.session_scope_epoch.saturating_add(1);
    }

    pub(crate) fn activate_session(&mut self, session_id: model::SessionId) {
        self.last_resumable_session_id = Some(session_id.clone());
        self.session_id = Some(session_id);
    }

    /// Return the active or most recently established session that can be resumed after exit.
    #[must_use]
    pub fn resumable_session_id(&self) -> Option<&model::SessionId> {
        self.session_id.as_ref().or(self.last_resumable_session_id.as_ref())
    }

    pub fn clear_identity(&mut self) {
        self.session_id = None;
        self.current_model = None;
        self.mode = None;
        self.fast_mode_state = model::FastModeState::Off;
        self.fast_mode_disabled_reason = None;
        self.session_usage = SessionUsageState::default();
    }
}
