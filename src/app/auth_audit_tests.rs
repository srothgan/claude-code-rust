// SPDX-License-Identifier: Apache-2.0

use super::*;
use serde_json::json;
use std::time::{Duration, UNIX_EPOCH};

#[test]
fn credential_files_require_the_trimmed_access_token_and_remain_read_only() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("credentials.json");
    assert_eq!(load_oauth_credentials_at(&path), None);
    assert!(!path.exists());
    std::fs::create_dir(&path).unwrap();
    assert_eq!(load_oauth_credentials_at(&path), None);
    assert!(path.is_dir());
    std::fs::remove_dir(&path).unwrap();
    for (raw, expected) in [
        (
            r#"{"claudeAiOauth":{"accessToken":"  fixture-access  ","refreshToken":"wrong-refresh","unrelated":"wrong-other"}}"#,
            Some("fixture-access"),
        ),
        ("{}", None),
        ("not json", None),
        (r#"{"claudeAiOauth":false}"#, None),
        (r#"{"claudeAiOauth":{"refreshToken":"refresh-only"}}"#, None),
        (r#"{"claudeAiOauth":{"accessToken":"","refreshToken":"refresh-only"}}"#, None),
        (r#"{"claudeAiOauth":{"accessToken":" \t\n "}}"#, None),
        (r#"{"claudeAiOauth":{"accessToken":42}}"#, None),
    ] {
        std::fs::write(&path, raw).unwrap();
        assert_eq!(
            load_oauth_credentials_at(&path).map(|c| c.access_token),
            expected.map(str::to_owned)
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
    }
}

#[test]
fn optional_expiry_preserves_exact_second_and_millisecond_instants() {
    for (expiry, expected) in [
        (json!(1), Some(UNIX_EPOCH + Duration::from_secs(1))),
        (json!(1_735_128_000_i64), Some(UNIX_EPOCH + Duration::from_secs(1_735_128_000))),
        (
            json!(1_735_128_000_123_i64),
            Some(UNIX_EPOCH + Duration::new(1_735_128_000, 123_000_000)),
        ),
        (json!("1735128000123"), Some(UNIX_EPOCH + Duration::new(1_735_128_000, 123_000_000))),
        (json!(" 1735128000 "), Some(UNIX_EPOCH + Duration::from_secs(1_735_128_000))),
        (json!(-1), None),
        (json!("invalid"), None),
        (json!(false), None),
        (json!(null), None),
        (json!("2025-01-01T00:00:00Z"), None),
    ] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("credentials.json");
        let bytes = json!({"claudeAiOauth":{"accessToken":"expiry-fixture","expiresAt":expiry}})
            .to_string();
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(
            load_oauth_credentials_at(&path),
            Some(ClaudeOAuthCredentials {
                access_token: "expiry-fixture".into(),
                expires_at: expected
            })
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), bytes);
    }
}

#[tokio::test]
async fn selected_profile_credentials_are_reloaded_without_fallback_or_creation() {
    if let Some(root) = std::env::var_os("CLAUDE_RS_AUDIT_ROOT") {
        let root = PathBuf::from(root);
        #[cfg(unix)]
        assert_eq!(dirs::home_dir(), Some(root.join("home")));
        let selected = std::env::var("CLAUDE_RS_AUDIT_CASE").unwrap();
        let (token, other) =
            if selected == "first" { ("first-token", "second") } else { ("second-token", "first") };
        let path = root.join(&selected).join(".credentials.json");
        let original = std::fs::read(&path).unwrap();
        let neighbor = root.join(other).join(".credentials.json");
        let neighbor_bytes = std::fs::read(&neighbor).unwrap();
        assert_eq!(credentials_path(), Some(path.clone()));
        assert_eq!(
            load_oauth_credentials(),
            Some(ClaudeOAuthCredentials {
                access_token: token.into(),
                expires_at: Some(UNIX_EPOCH + Duration::from_secs(1))
            })
        );
        assert!(has_credentials(), "presence is distinct from request eligibility");
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let replacement = br#"{"claudeAiOauth":{"accessToken":"replacement-token"}}"#;
        std::fs::write(&path, replacement).unwrap();
        assert_eq!(load_oauth_credentials().unwrap().access_token, "replacement-token");
        assert!(has_credentials());
        assert_eq!(std::fs::read(&path).unwrap(), replacement);
        std::fs::write(&path, b"malformed").unwrap();
        assert_eq!(load_oauth_credentials(), None);
        assert!(!has_credentials());
        assert_eq!(std::fs::read(&path).unwrap(), b"malformed");
        std::fs::remove_file(&path).unwrap();
        assert_eq!(load_oauth_credentials(), None);
        assert!(!has_credentials());
        assert!(!path.exists());
        assert_eq!(std::fs::read(neighbor).unwrap(), neighbor_bytes);
        assert_eq!(
            std::fs::read(root.join("home/.claude/.credentials.json")).unwrap(),
            br#"{"claudeAiOauth":{"accessToken":"default-must-not-win"}}"#
        );
        std::fs::write(path, original).unwrap();
        return;
    }
    let root = tempfile::tempdir().unwrap();
    for selected in ["first", "second"] {
        let directory = root.path().join(selected);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(
            directory.join(".credentials.json"),
            json!({"claudeAiOauth":{"accessToken":format!("{selected}-token"),"expiresAt":1}})
                .to_string(),
        )
        .unwrap();
    }
    std::fs::create_dir_all(root.path().join("home/.claude")).unwrap();
    std::fs::write(
        root.path().join("home/.claude/.credentials.json"),
        br#"{"claudeAiOauth":{"accessToken":"default-must-not-win"}}"#,
    )
    .unwrap();
    for selected in ["first", "second"] {
        let mut command = crate::diagnostics_test_support::command(
            "app::auth::audit_tests::selected_profile_credentials_are_reloaded_without_fallback_or_creation",
            root.path(),
            selected,
        );
        command
            .current_dir(root.path())
            .env("CLAUDE_CONFIG_DIR", root.path().join(selected))
            .env("HOME", root.path().join("home"));
        let output = crate::diagnostics_test_support::run(command).await;
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }
}
