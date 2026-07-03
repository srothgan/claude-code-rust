// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use serde_json::Value;

const REDACTED: &str = "[redacted]";

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

fn is_secret_key(key: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::{redact_env_value, redact_json_value};

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
}
