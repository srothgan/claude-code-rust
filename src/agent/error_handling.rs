// SPDX-License-Identifier: Apache-2.0

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnErrorClass {
    PlanLimit,
    AuthRequired,
    AccountAccess,
    ModelUnavailable,
    TransientService,
    Internal,
    Other,
}

pub fn parse_turn_error_class(tag: &str) -> Option<TurnErrorClass> {
    match tag {
        "plan_limit" => Some(TurnErrorClass::PlanLimit),
        "auth_required" => Some(TurnErrorClass::AuthRequired),
        "account_access" => Some(TurnErrorClass::AccountAccess),
        "model_unavailable" => Some(TurnErrorClass::ModelUnavailable),
        "transient_service" => Some(TurnErrorClass::TransientService),
        "internal" => Some(TurnErrorClass::Internal),
        "other" => Some(TurnErrorClass::Other),
        _ => None,
    }
}

pub fn classify_turn_error(input: &str) -> TurnErrorClass {
    let lower = input.to_ascii_lowercase();
    if looks_like_plan_limit_error_lower(&lower) {
        TurnErrorClass::PlanLimit
    } else if looks_like_auth_required_error_lower(&lower) {
        TurnErrorClass::AuthRequired
    } else if looks_like_account_access_error_lower(&lower) {
        TurnErrorClass::AccountAccess
    } else if looks_like_model_unavailable_error_lower(&lower) {
        TurnErrorClass::ModelUnavailable
    } else if looks_like_transient_service_error_lower(&lower) {
        TurnErrorClass::TransientService
    } else if looks_like_internal_error_lower(&lower) {
        TurnErrorClass::Internal
    } else {
        TurnErrorClass::Other
    }
}

pub fn looks_like_internal_error(input: &str) -> bool {
    looks_like_internal_error_lower(&input.to_ascii_lowercase())
}

pub fn summarize_internal_error(input: &str) -> String {
    if let Some(summary) = summarize_permission_schema_error(input) {
        return error_preview(&summary);
    }
    if let Some(msg) = extract_xml_tag_value(input, "message") {
        return error_preview(msg);
    }
    if let Some(msg) = extract_json_string_field(input, "message") {
        return error_preview(&msg);
    }
    let fallback = input.lines().find(|line| !line.trim().is_empty()).unwrap_or(input);
    error_preview(fallback.trim())
}

fn looks_like_plan_limit_error_lower(lower: &str) -> bool {
    [
        "rate limit",
        "rate-limit",
        "max turns",
        "max turn",
        "max budget",
        "quota",
        "plan limit",
        "plan-limit",
        "429",
        "too many requests",
        "usage limit",
        "insufficient quota",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn looks_like_auth_required_error_lower(lower: &str) -> bool {
    [
        "/login",
        "auth required",
        "authentication failed",
        "please log in",
        "login required",
        "not authenticated",
        "unauthorized",
        "verification_required",
        "verification required",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn looks_like_account_access_error_lower(lower: &str) -> bool {
    [
        "oauth_org_not_allowed",
        "oauth org not allowed",
        "organization not allowed",
        "account access not allowed",
        "org not allowed",
        "cloud_credential_error",
        "cloud credential error",
        "cloud provider credentials",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn looks_like_model_unavailable_error_lower(lower: &str) -> bool {
    [
        "model_not_found",
        "model not found",
        "model unavailable",
        "model is unavailable",
        "model is not available",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn looks_like_transient_service_error_lower(lower: &str) -> bool {
    [
        "overloaded",
        "overloaded_error",
        "temporarily overloaded",
        "service overloaded",
        "server overloaded",
        "server_error",
        "server error",
        "http 529",
        "529",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn looks_like_internal_error_lower(lower: &str) -> bool {
    has_internal_error_keywords(lower)
        || looks_like_json_rpc_error_shape(lower)
        || looks_like_xml_error_shape(lower)
}

fn has_internal_error_keywords(lower: &str) -> bool {
    [
        "internal error",
        "agent sdk",
        "claude-agent-sdk",
        "adapter",
        "bridge",
        "json-rpc",
        "rpc",
        "protocol error",
        "transport",
        "handshake failed",
        "session creation failed",
        "connection closed",
        "event channel closed",
        "tool permission request failed",
        "zoderror",
        "invalid_union",
        "bridge command failed",
        "agent stream failed",
        "agent initialization failed",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn looks_like_json_rpc_error_shape(lower: &str) -> bool {
    (lower.contains("\"jsonrpc\"") && lower.contains("\"error\""))
        || lower.contains("\"code\":-32603")
        || lower.contains("\"code\": -32603")
}

fn looks_like_xml_error_shape(lower: &str) -> bool {
    let has_error_node = lower.contains("<error") || lower.contains("<fault");
    let has_detail_node = lower.contains("<message>") || lower.contains("<code>");
    has_error_node && has_detail_node
}

fn summarize_permission_schema_error(input: &str) -> Option<String> {
    let lower = input.to_ascii_lowercase();
    if !lower.contains("tool permission request failed") {
        return None;
    }

    let detail = if let Some(msg) = extract_json_string_field(input, "message") {
        msg
    } else {
        input.lines().find(|line| !line.trim().is_empty()).unwrap_or(input).trim().to_owned()
    };

    Some(format!("Tool permission request failed: {detail}"))
}

pub(crate) fn error_preview(input: &str) -> String {
    crate::logging::preview_text(input, 240).replace('\n', "\\n")
}

pub(crate) fn extract_xml_tag_value<'a>(input: &'a str, tag: &str) -> Option<&'a str> {
    let lower = input.to_ascii_lowercase();
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = lower.find(&open)? + open.len();
    let end = start + lower[start..].find(&close)?;
    let value = input[start..end].trim();
    (!value.is_empty()).then_some(value)
}

fn extract_json_string_field(input: &str, field: &str) -> Option<String> {
    let needle = format!("\"{field}\"");
    let start = input.find(&needle)? + needle.len();
    let rest = input[start..].trim_start();
    let colon_idx = rest.find(':')?;
    // Decode just the field value: permission diagnostics may prefix the JSON with prose.
    serde_json::Deserializer::from_str(rest[colon_idx + 1..].trim_start())
        .into_iter::<String>()
        .next()?
        .ok()
}

#[cfg(test)]
mod tests {
    use super::{
        TurnErrorClass, classify_turn_error, looks_like_internal_error, parse_turn_error_class,
        summarize_internal_error,
    };

    #[test]
    fn error_previews_preserve_unicode_limits_and_escape_after_truncation() {
        assert_eq!(summarize_internal_error(""), "");
        for (cause, expected) in [
            ("é\n界🦀".to_owned(), "é\\n界🦀".to_owned()),
            (format!("{}\n界🦀", "é".repeat(237)), format!("{}\\n界🦀", "é".repeat(237))),
            (
                format!("{}\n界🦀OMITTED_TAIL", "é".repeat(237)),
                format!("{}\\n界🦀...", "é".repeat(237)),
            ),
        ] {
            let payload = format!("<error><message>{cause}</message></error>");
            let summary = summarize_internal_error(&payload);
            assert_eq!(summary, expected);
            assert!(!summary.contains('\n'));
            assert!(!summary.contains("OMITTED_TAIL"));
        }
    }

    #[test]
    fn classifies_fallback_error_categories() {
        for (payload, expected) in [
            ("HTTP 429 Too Many Requests", TurnErrorClass::PlanLimit),
            ("turn failed: max budget exceeded", TurnErrorClass::PlanLimit),
            ("Usage blocked: insufficient quota for this account", TurnErrorClass::PlanLimit),
            ("authentication failed: please log in", TurnErrorClass::AuthRequired),
            ("oauth_org_not_allowed", TurnErrorClass::AccountAccess),
            ("model_not_found", TurnErrorClass::ModelUnavailable),
            ("HTTP 529 overloaded_error", TurnErrorClass::TransientService),
            // An actionable cause remains actionable inside an SDK/transport wrapper.
            ("Bridge command failed: MAX BUDGET exceeded", TurnErrorClass::PlanLimit),
            ("Agent SDK: Authentication Failed: PLEASE LOG IN", TurnErrorClass::AuthRequired),
            ("Bridge command failed: OAUTH_ORG_NOT_ALLOWED", TurnErrorClass::AccountAccess),
            (
                r#"{"error":{"type":"MODEL_NOT_FOUND","message":"resource missing"}}"#,
                TurnErrorClass::ModelUnavailable,
            ),
            (
                "<error><message>Service temporarily OVERLOADED</message></error>",
                TurnErrorClass::TransientService,
            ),
            ("Bridge command failed: socket disappeared", TurnErrorClass::Internal),
            ("worker stopped: socket disappeared", TurnErrorClass::Other),
        ] {
            assert_eq!(classify_turn_error(payload), expected, "payload: {payload}");
        }
    }

    #[test]
    fn classifies_internal_errors() {
        for payload in [
            r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"worker stopped"}}"#,
            // No "rpc" token: this must be recognized by the error-code shape alone.
            r#"{"error":{"code":-32603,"message":"worker stopped"}}"#,
            "<error><message>worker stopped</message></error>",
            "<error><code>-32603</code><message>Adapter process crashed</message></error>",
            r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"internal rpc fault"}}"#,
            r#"Tool permission request failed: ZodError: [{"message":"Invalid input"}]"#,
        ] {
            assert_eq!(classify_turn_error(payload), TurnErrorClass::Internal, "{payload}");
            assert!(looks_like_internal_error(payload), "{payload}");
        }
        let ordinary_failure = "worker stopped: socket disappeared";
        assert_eq!(classify_turn_error(ordinary_failure), TurnErrorClass::Other);
        assert!(!looks_like_internal_error(ordinary_failure));
    }

    #[test]
    fn classifies_other_errors() {
        assert_eq!(classify_turn_error("turn failed: timeout"), TurnErrorClass::Other);
        let plain_bash_failure = "bash: unknown_command: command not found";
        assert_eq!(classify_turn_error(plain_bash_failure), TurnErrorClass::Other);
        assert!(!looks_like_internal_error(plain_bash_failure));
    }

    #[test]
    fn parses_bridge_turn_error_kind_tags() {
        assert_eq!(parse_turn_error_class("plan_limit"), Some(TurnErrorClass::PlanLimit));
        assert_eq!(parse_turn_error_class("auth_required"), Some(TurnErrorClass::AuthRequired));
        assert_eq!(parse_turn_error_class("account_access"), Some(TurnErrorClass::AccountAccess));
        assert_eq!(
            parse_turn_error_class("model_unavailable"),
            Some(TurnErrorClass::ModelUnavailable)
        );
        assert_eq!(
            parse_turn_error_class("transient_service"),
            Some(TurnErrorClass::TransientService)
        );
        assert_eq!(parse_turn_error_class("internal"), Some(TurnErrorClass::Internal));
        assert_eq!(parse_turn_error_class("other"), Some(TurnErrorClass::Other));
        assert_eq!(parse_turn_error_class("unexpected"), None);
    }

    #[test]
    fn summaries_preserve_useful_error_causes() {
        for (payload, expected) in [
            (r#"{"message":"caf\u00e9"}"#, "café"),
            (
                "<error><code>-32603</code><message>Adapter process crashed</message></error>",
                "Adapter process crashed",
            ),
            (
                r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"internal rpc fault"}}"#,
                "internal rpc fault",
            ),
            (
                r#"{"jsonrpc":"2.0","error":{"message":"quoted \"path\" C:\\work\ncaf\u00e9 \u754c \uD83E\uDD80"},"data":"irrelevant detail"}"#,
                "quoted \"path\" C:\\work\\ncafé 界 🦀",
            ),
            (
                "<error><message>worker stopped\ncheck socket</message><detail>irrelevant detail</detail></error>",
                "worker stopped\\ncheck socket",
            ),
            (
                "\n  \n  socket closed unexpectedly  \nignore later detail",
                "socket closed unexpectedly",
            ),
            (r#"{"message":"bad\q"}"#, r#"{"message":"bad\q"}"#),
            (r#"{"message":"unfinished \u00"#, r#"{"message":"unfinished \u00"#),
            (r#"{"message":"unpaired \uD83E"}"#, r#"{"message":"unpaired \uD83E"}"#),
        ] {
            assert_eq!(summarize_internal_error(payload), expected, "payload: {payload}");
        }

        for (payload, cause) in [
            (
                r#"Tool permission request failed: ZodError: [{"message":"Invalid input: expected record, received undefined"}]"#,
                "Invalid input: expected record, received undefined",
            ),
            (
                r#"Tool permission request failed: ZodError: [{"message":"Denied \"C:\\work\"\nuser caf\u00e9 \uD83E\uDD80","path":["irrelevant detail"]}]"#,
                "Denied \"C:\\work\"\\nuser café 🦀",
            ),
        ] {
            let summary = summarize_internal_error(payload);
            assert!(summary.contains("Tool permission request failed"));
            assert!(summary.ends_with(cause), "{summary}");
            assert!(!summary.contains("ZodError"), "{summary}");
            assert!(!summary.contains("irrelevant detail"), "{summary}");
        }

        let malformed = r#"Tool permission request failed: ZodError: [{"message":"bad\q"}]"#;
        let summary = summarize_internal_error(malformed);
        assert!(
            summary.contains(malformed),
            "malformed diagnostic must remain faithful: {summary}"
        );
    }
}
