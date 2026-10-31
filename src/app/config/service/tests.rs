// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::agent::events::ClientEvent;
use crate::agent::settings::SettingsSnapshot;
use serde_json::json;

#[path = "audit_tests.rs"]
mod audit;

#[tokio::test]
async fn acknowledged_spinner_tip_setting_only_controls_tips() {
    let mut app = App::test_default();
    app.session_runtime.session_id = Some("session-1".into());
    app.transcript.messages.push(crate::app::ChatMessage::new(
        crate::app::MessageRole::Assistant,
        Vec::new(),
        None,
    ));
    app.bind_active_turn_assistant(0);
    app.status = crate::app::AppStatus::Running;
    let start = std::time::Instant::now();
    app.begin_turn_activity(start);
    let now = start;
    let initial = app.activity_presentation(now).expect("active");
    assert!(initial.tip.is_some());
    for enabled in [false, true] {
        let mut acknowledged = SettingsSnapshot::test_value("spinnerTipsEnabled", json!(enabled));
        acknowledged.cwd.clone_from(&app.cwd_raw);
        app.config.pending_settings_request =
            Some(crate::app::config::PendingSettingsRequest::Inspection("tips".into()));
        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::SettingsResultReceived {
                session_id: "session-1".into(),
                request_id: Some("tips".into()),
                result: SettingsResult {
                    persistence: SettingsPersistence::NotRequested,
                    application: SettingsApplication::Host,
                    snapshot: Some(acknowledged),
                    error: None,
                },
            },
        );
        let presentation = app.activity_presentation(now).expect("active turn");
        assert_eq!(presentation.tip.is_some(), enabled);
        assert!(!presentation.thinking, "settings cannot invent SDK thinking");
        assert_eq!(presentation.label, initial.label);
        assert_eq!(presentation.verb, initial.verb);
    }
}
