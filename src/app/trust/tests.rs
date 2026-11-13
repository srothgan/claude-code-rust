// SPDX-License-Identifier: Apache-2.0
use super::*;
use serde_json::json;

#[test]
fn accepting_trust_preserves_fresh_auth_and_project_data() {
    let fixture = tempfile::tempdir().expect("tempdir");
    let path = fixture.path().join(".claude.json");
    let project = fixture.path().join("project");
    let mut app = App::test_default();
    app.cwd_raw = project.to_string_lossy().into_owned();
    app.trust.preferences_path = Some(path.clone());
    std::fs::write(&path, r#"{"projects": {}}"#).expect("fixture");
    initialize(&mut app);
    let key = store::normalize_project_key(&project);
    let mut external =
        json!({"oauthAccount": {"accountUuid": "keep"}, "unknown": [1,2], "projects": {}});
    external["projects"][&key] = json!({"mcpServers": {"server": {"command": "keep"}}});
    std::fs::write(&path, serde_json::to_vec(&external).expect("serialize"))
        .expect("external edit");
    accept(&mut app).expect("accept");
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("JSON");
    assert_eq!(saved["oauthAccount"], external["oauthAccount"]);
    assert_eq!(saved["unknown"], external["unknown"]);
    assert_eq!(saved["projects"][&key]["mcpServers"], external["projects"][&key]["mcpServers"]);
    assert!(store::read_status(&saved, &project).trusted);
}
