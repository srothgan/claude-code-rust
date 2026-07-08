// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use serde_json::Value;

const REDACTED: &str = "[redacted]";
const SECRET_ASSIGNMENT_KEYS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "api_key",
    "apikey",
    "accessToken",
    "refreshToken",
    "authorization",
    "password",
    "secret",
    "credential",
];

pub fn redact_json_value(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if is_secret_key(key) {
                    *value = Value::String(REDACTED.to_owned());
                } else {
                    redact_json_value(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact_json_value(value);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

pub fn redact_env_value(name: &str, value: Option<&str>) -> Option<String> {
    value.map(|value| if is_secret_key(name) { REDACTED.to_owned() } else { value.to_owned() })
}

pub fn redact_text(input: &str) -> String {
    input.lines().map(redact_line).collect::<Vec<_>>().join("\n")
}

pub fn redact_line(line: &str) -> String {
    let mut redacted = if let Ok(mut value) = serde_json::from_str::<Value>(line) {
        redact_json_value(&mut value);
        serde_json::to_string(&value).unwrap_or_else(|_| line.to_owned())
    } else {
        line.to_owned()
    };

    redacted = redact_bearer_tokens(&redacted);
    for key in SECRET_ASSIGNMENT_KEYS {
        redacted = redact_assignment_values(&redacted, key);
    }
    redacted
}

pub fn is_secret_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase().replace('-', "_");
    normalized.contains("token")
        || normalized.contains("secret")
        || normalized.contains("api_key")
        || normalized.contains("apikey")
        || normalized.contains("authorization")
        || normalized.contains("password")
        || normalized.contains("credential")
        || normalized == "accesskey"
        || normalized == "refreshkey"
}

fn redact_bearer_tokens(input: &str) -> String {
    let mut output = input.to_owned();
    let mut search_start = 0;
    while let Some(relative_start) = find_ascii_case_insensitive(&output[search_start..], "Bearer ")
    {
        let start = search_start + relative_start;
        let value_start = start + "Bearer ".len();
        let value_end = output[value_start..]
            .find(is_value_delimiter)
            .map_or(output.len(), |offset| value_start + offset);
        if value_end <= value_start {
            break;
        }
        output.replace_range(value_start..value_end, REDACTED);
        search_start = value_start + REDACTED.len();
    }
    output
}

fn redact_assignment_values(input: &str, key: &str) -> String {
    let mut output = input.to_owned();
    let mut search_start = 0;

    while let Some(relative_key_start) = find_ascii_case_insensitive(&output[search_start..], key) {
        let key_start = search_start + relative_key_start;
        let Some((value_start, quote)) = assignment_value_start(&output, key_start + key.len())
        else {
            search_start = key_start + key.len();
            continue;
        };
        let value_end = assignment_value_end(&output, value_start, quote);
        if value_end <= value_start {
            search_start = value_start;
            continue;
        }
        output.replace_range(value_start..value_end, REDACTED);
        search_start = value_start + REDACTED.len();
    }

    output
}

fn assignment_value_start(input: &str, offset: usize) -> Option<(usize, Option<u8>)> {
    let bytes = input.as_bytes();
    let mut index = offset;
    if bytes.get(index).is_some_and(|byte| *byte == b'"' || *byte == b'\'') {
        index += 1;
    }
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    if !bytes.get(index).is_some_and(|byte| *byte == b':' || *byte == b'=') {
        return None;
    }
    index += 1;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    let quote = bytes.get(index).copied().filter(|byte| *byte == b'"' || *byte == b'\'');
    if quote.is_some() {
        index += 1;
    }
    Some((index, quote))
}

fn assignment_value_end(input: &str, start: usize, quote: Option<u8>) -> usize {
    let bytes = input.as_bytes();
    let mut index = start;
    while let Some(byte) = bytes.get(index) {
        if quote.is_some_and(|quote| *byte == quote)
            || quote.is_none() && is_value_delimiter(char::from(*byte))
        {
            break;
        }
        index += 1;
    }
    index
}

fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    haystack.to_ascii_lowercase().find(&needle.to_ascii_lowercase())
}

fn is_value_delimiter(ch: char) -> bool {
    ch.is_ascii_whitespace() || matches!(ch, '"' | '\'' | ',' | ';' | '&' | '}')
}

#[cfg(test)]
mod tests {
    use super::{redact_env_value, redact_json_value, redact_line, redact_text};

    #[test]
    fn redacts_secret_json_keys_recursively() {
        let mut value = serde_json::json!({
            "model": "sonnet",
            "env": {
                "ANTHROPIC_API_KEY": "sk-ant-secret",
                "KEEP": "visible"
            },
            "claudeAiOauth": {
                "accessToken": "access",
                "refreshToken": "refresh"
            }
        });

        redact_json_value(&mut value);

        assert_eq!(value["model"], "sonnet");
        assert_eq!(value["env"]["KEEP"], "visible");
        assert_eq!(value["env"]["ANTHROPIC_API_KEY"], "[redacted]");
        assert_eq!(value["claudeAiOauth"]["accessToken"], "[redacted]");
        assert_eq!(value["claudeAiOauth"]["refreshToken"], "[redacted]");
    }

    #[test]
    fn redacts_secret_env_values_only() {
        assert_eq!(
            redact_env_value("ANTHROPIC_API_KEY", Some("sk-ant-secret")).as_deref(),
            Some("[redacted]")
        );
        assert_eq!(redact_env_value("PATH", Some("bin")).as_deref(), Some("bin"));
        assert_eq!(redact_env_value("PATH", None), None);
    }

    #[test]
    fn redacts_json_log_lines() {
        let line =
            r#"{"message":"x","fields":{"Authorization":"Bearer secret-token","keep":"ok"}}"#;

        let redacted = redact_line(line);

        assert!(redacted.contains(r#""keep":"ok""#));
        assert!(!redacted.contains("secret-token"));
        assert!(redacted.contains("[redacted]"));
    }

    #[test]
    fn redacts_common_text_credentials() {
        let text = "ANTHROPIC_API_KEY=sk-ant-secret\nAuthorization: Bearer oauth-secret\nraw Bearer raw-secret";

        let redacted = redact_text(text);

        assert!(!redacted.contains("sk-ant-secret"));
        assert!(!redacted.contains("oauth-secret"));
        assert!(!redacted.contains("raw-secret"));
        assert!(redacted.contains("ANTHROPIC_API_KEY=[redacted]"));
        assert!(redacted.contains("Authorization: [redacted]"));
        assert!(redacted.contains("Bearer [redacted]"));
    }
}
