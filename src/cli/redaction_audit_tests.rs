// SPDX-License-Identifier: Apache-2.0

use super::{redact_json_value, redact_line, redact_text};

#[test]
fn recursive_json_redaction_preserves_safe_values_and_array_associations() {
    let mut input = serde_json::json!({"model":"sonnet","max_tokens":4096,"input_tokens":120,
        "items":[{"Authorization":"header-secret","API-Key":"api-secret","keep":true},
            {"accessToken":"access-secret","refresh-token":"refresh-secret","token":"token-secret","context":"visible"},42,null,false,"safe"],
        "credentials":{"private":"whole-object-secret"},"env":{"PASSWORD":"password-secret","KEEP":"visible"}});
    redact_json_value(&mut input);
    assert_eq!(
        input,
        serde_json::json!({"model":"sonnet","max_tokens":4096,"input_tokens":120,
        "items":[{"Authorization":"[redacted]","API-Key":"[redacted]","keep":true},
            {"accessToken":"[redacted]","refresh-token":"[redacted]","token":"[redacted]","context":"visible"},42,null,false,"safe"],
        "credentials":"[redacted]","env":{"PASSWORD":"[redacted]","KEEP":"visible"}})
    );
}

#[test]
fn structured_log_redaction_preserves_context_and_scrubs_message_credentials() {
    let input = r#"{"target":"bridge.sdk","event_name":"retry","message":"request Bearer message-secret failed","fields":{"Authorization":"header-secret","cause":"timeout","items":[{"access_token":"array-secret","attempt":2}],"keep":true}}"#;
    let output = redact_line(input);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&output).unwrap(),
        serde_json::json!({
        "target":"bridge.sdk","event_name":"retry","message":"request Bearer [redacted] failed",
        "fields":{"Authorization":"[redacted]","cause":"timeout","items":[{"access_token":"[redacted]","attempt":2}],"keep":true}})
    );
    for secret in ["message-secret", "header-secret", "array-secret"] {
        assert!(!output.contains(secret));
    }
}

#[test]
fn redacts_credential_shapes() {
    let cases: &[(&str, &str)] = &[
        ("GET /v1?token=abc&x=1", "GET /v1?token=[redacted]&x=1"),
        ("CLAUDE_CODE_OAUTH_TOKEN=sk-ant-oat01-abcDEF123", "CLAUDE_CODE_OAUTH_TOKEN=[redacted]"),
        ("x-api-key: sk-ant-api03-secretvalue", "x-api-key: [redacted]"),
        ("access_token=abc123 next", "access_token=[redacted] next"),
        ("fetch https://user:pass@example.com/repo", "fetch https://[redacted]@example.com/repo"),
        ("Authorization: Basic dXNlcjpwYXNz", "Authorization: [redacted]"),
        ("Authorization: Bearer abc.def", "Authorization: [redacted]"),
        ("Authorization: token ghp_abc", "Authorization: [redacted]"),
        ("key sk-live_abc123 end", "key [redacted] end"),
        ("max_tokens=4096 input_tokens: 120", "max_tokens=4096 input_tokens: 120"),
        ("refresh-token=abc next", "refresh-token=[redacted] next"),
        ("token: abc max_tokens=1", "token: [redacted] max_tokens=1"),
        ("accessToken: abc next", "accessToken: [redacted] next"),
        ("task-1 and risk-free are not keys", "task-1 and risk-free are not keys"),
    ];
    let additional = [
        ("raw Bearer ; retry Bearer later-secret", "raw Bearer ; retry Bearer [redacted]"),
        (
            "raw Bearer , then Bearer second-secret; Bearer third-secret",
            "raw Bearer , then Bearer [redacted]; Bearer [redacted]",
        ),
        (
            "ANTHROPIC_API_KEY=first-secret\nAuthorization: Bearer second-secret\nraw Bearer third-secret",
            "ANTHROPIC_API_KEY=[redacted]\nAuthorization: [redacted]\nraw Bearer [redacted]",
        ),
        (
            "before password=one-secret; token=two-secret after",
            "before password=[redacted]; token=[redacted] after",
        ),
        (
            "\u{96ea} Bearer first-secret; retry Bearer second-secret fin",
            "\u{96ea} Bearer [redacted]; retry Bearer [redacted] fin",
        ),
    ];
    for (input, expected) in cases.iter().chain(additional.iter()) {
        assert_eq!(redact_text(input), *expected, "input: {input}");
    }
}
