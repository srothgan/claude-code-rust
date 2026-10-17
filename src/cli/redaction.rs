// SPDX-License-Identifier: Apache-2.0

use std::path::Path;

use serde_json::Value;

const REDACTED: &str = "[redacted]";
/// Name fragments that make a `name=value` or `name: value` pair secret. A fragment
/// matches inside a longer name (`CLAUDE_CODE_OAUTH_TOKEN`, `x-api-key`, `access_token`).
const SECRET_ASSIGNMENT_KEYS: &[&str] =
    &["api_key", "api-key", "apikey", "token", "secret", "password", "credential", "authorization"];
/// Credential schemes that precede the secret in an `Authorization` value.
const AUTH_SCHEMES: &[&str] = &["basic ", "digest ", "negotiate ", "bearer ", "token "];

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

/// Line-oriented redaction for text that reaches the always-on log, support bundles and
/// failure reports.
///
/// The TypeScript twin is `redactStartupDetail` in `agent-sdk/src/bridge/startup_failures.ts`.
/// This function is the reference: its table test (`redacts_credential_shapes`) lists the
/// shapes both must cover, and a rule added to one is added to the other.
pub fn redact_text(input: &str) -> String {
    redact_text_with_home(input, dirs::home_dir().as_deref())
}

/// Credential rules only, for output whose paths are the point (`config path`).
pub fn redact_credentials(input: &str) -> String {
    redact_text_with_home(input, None)
}

pub fn redact_line(line: &str) -> String {
    redact_line_with_home(line, dirs::home_dir().as_deref())
}

fn redact_text_with_home(input: &str, home: Option<&Path>) -> String {
    input.lines().map(|line| redact_line_with_home(line, home)).collect::<Vec<_>>().join("\n")
}

fn redact_line_with_home(line: &str, home: Option<&Path>) -> String {
    let mut redacted = if let Ok(mut value) = serde_json::from_str::<Value>(line) {
        redact_json_value(&mut value);
        serde_json::to_string(&value).unwrap_or_else(|_| line.to_owned())
    } else {
        line.to_owned()
    };

    redacted = redact_bearer_tokens(&redacted);
    redacted = redact_sk_tokens(&redacted);
    redacted = redact_url_userinfo(&redacted);
    for key in SECRET_ASSIGNMENT_KEYS {
        redacted = redact_assignment_values(&redacted, key);
    }
    match home.and_then(Path::to_str) {
        Some(home) => redact_home_prefix(&redacted, home),
        None => redacted,
    }
}

/// Replaces the user's home directory with `~` so error text (a failed spawn, a stack
/// trace, a hook's output) names the file without naming the account. A path that only
/// starts with the same characters (`/Users/alice2`) is a different directory and stays.
/// A Windows-style home matches case-insensitively, with `/`, `\` or `\\` (JSON-escaped)
/// for each separator.
fn redact_home_prefix(input: &str, home: &str) -> String {
    let home = home.trim_end_matches(['/', '\\']);
    if home.is_empty() {
        return input.to_owned();
    }
    let windows_style = home.contains('\\') || home.as_bytes().get(1) == Some(&b':');
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut copied = 0;
    let mut index = 0;
    while index < bytes.len() {
        if let Some(end) = match_home(bytes, index, home.as_bytes(), windows_style)
            && home_boundary_before(bytes, index)
            && home_boundary_after(bytes, end)
        {
            output.push_str(&input[copied..index]);
            output.push('~');
            copied = end;
            index = end;
            continue;
        }
        index += 1;
    }
    output.push_str(&input[copied..]);
    output
}

/// End of the match of `home` at `start`, if any.
fn match_home(bytes: &[u8], start: usize, home: &[u8], windows_style: bool) -> Option<usize> {
    let mut at = start;
    for expected in home {
        let actual = *bytes.get(at)?;
        if windows_style && matches!(expected, b'/' | b'\\') {
            match actual {
                b'\\' if bytes.get(at + 1) == Some(&b'\\') => at += 2,
                b'/' | b'\\' => at += 1,
                _ => return None,
            }
        } else if windows_style && actual.eq_ignore_ascii_case(expected) || actual == *expected {
            at += 1;
        } else {
            return None;
        }
    }
    Some(at)
}

fn is_path_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-') || byte >= 0x80
}

fn home_boundary_before(bytes: &[u8], start: usize) -> bool {
    let Some(previous) = start.checked_sub(1).map(|index| bytes[index]) else {
        return true;
    };
    match previous {
        b'/' | b'\\' => bytes[..start].ends_with(b"://"),
        b'.' | b'~' => false,
        byte => !is_path_name_byte(byte),
    }
}

fn home_boundary_after(bytes: &[u8], end: usize) -> bool {
    match bytes.get(end) {
        None => true,
        Some(b'.') => !bytes.get(end + 1).copied().is_some_and(is_path_name_byte),
        Some(byte) => !is_path_name_byte(*byte),
    }
}

pub fn is_secret_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase().replace('-', "_");
    is_token_name(key, &normalized)
        || normalized.contains("secret")
        || normalized.contains("api_key")
        || normalized.contains("apikey")
        || normalized.contains("authorization")
        || normalized.contains("password")
        || normalized.contains("credential")
        || normalized == "accesskey"
        || normalized == "refreshkey"
}

/// `token`, `access_token`, `refresh-token` and `accessToken` name a credential;
/// `max_tokens` and `input_tokens` count them.
fn is_token_name(key: &str, normalized: &str) -> bool {
    normalized == "token" || normalized.ends_with("_token") || key.ends_with("Token")
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

/// Provider keys (`sk-...`, `sk-ant-...`) wherever they appear, even without a name.
fn redact_sk_tokens(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut copied = 0;
    let mut index = 0;
    while index + 3 < bytes.len() {
        let at_boundary = index == 0 || !is_word_byte(bytes[index - 1]);
        if at_boundary && bytes[index..index + 3].eq_ignore_ascii_case(b"sk-") {
            let end = index
                + 3
                + bytes[index + 3..].iter().take_while(|byte| is_token_byte(**byte)).count();
            if end > index + 3 {
                output.push_str(&input[copied..index]);
                output.push_str(REDACTED);
                copied = end;
                index = end;
                continue;
            }
        }
        index += 1;
    }
    output.push_str(&input[copied..]);
    output
}

/// `https://user:pass@host` keeps the scheme and host and drops the userinfo.
fn redact_url_userinfo(input: &str) -> String {
    let mut output = input.to_owned();
    for scheme in ["https://", "http://"] {
        let mut search_start = 0;
        while let Some(relative) = find_ascii_case_insensitive(&output[search_start..], scheme) {
            let start = search_start + relative + scheme.len();
            let userinfo_len = output.as_bytes()[start..]
                .iter()
                .take_while(|byte| !byte.is_ascii_whitespace() && **byte != b'/' && **byte != b'@')
                .count();
            if userinfo_len > 0 && output.as_bytes().get(start + userinfo_len) == Some(&b'@') {
                output.replace_range(start..start + userinfo_len, REDACTED);
                search_start = start + REDACTED.len();
            } else {
                search_start = start;
            }
        }
    }
    output
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

fn redact_assignment_values(input: &str, key: &str) -> String {
    let mut output = input.to_owned();
    let mut search_start = 0;

    while let Some(relative_key_start) = find_ascii_case_insensitive(&output[search_start..], key) {
        let key_start = search_start + relative_key_start;
        let name_end = key_start
            + key.len()
            + output.as_bytes()[key_start + key.len()..]
                .iter()
                .take_while(|byte| is_token_byte(**byte))
                .count();
        if key == "token" && !is_token_key_span(&output, key_start, name_end) {
            search_start = key_start + key.len();
            continue;
        }
        let Some((value_start, quote)) = assignment_value_start(&output, name_end) else {
            search_start = key_start + key.len();
            continue;
        };
        let scheme_len = if quote.is_none() { auth_scheme_len(&output[value_start..]) } else { 0 };
        let value_end = assignment_value_end(&output, value_start + scheme_len, quote);
        if value_end <= value_start {
            search_start = value_start;
            continue;
        }
        output.replace_range(value_start..value_end, REDACTED);
        search_start = value_start + REDACTED.len();
    }

    output
}

/// Whether the name spanning `key_start..name_end` is `token` or ends in `_token`,
/// `-token` or `Token`, and not a longer word such as `max_tokens` or `tokenizer`.
fn is_token_key_span(input: &str, key_start: usize, name_end: usize) -> bool {
    let bytes = input.as_bytes();
    let key_end = key_start + "token".len();
    if name_end != key_end {
        return false;
    }
    // After a letter or digit only the camelCase `accessToken` form counts.
    key_start.checked_sub(1).is_none_or(|index| !bytes[index].is_ascii_alphanumeric())
        || bytes[key_start] == b'T'
}

/// Length of a leading credential scheme plus its spaces (`Basic `), so the credential
/// after it is redacted with it instead of being left behind.
fn auth_scheme_len(value: &str) -> usize {
    let lowered = value.to_ascii_lowercase();
    AUTH_SCHEMES.iter().find(|scheme| lowered.starts_with(**scheme)).map_or(0, |scheme| {
        scheme.len() + value[scheme.len()..].bytes().take_while(|byte| *byte == b' ').count()
    })
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
    use std::path::Path;

    use super::{
        redact_env_value, redact_json_value, redact_line, redact_text, redact_text_with_home,
    };

    #[test]
    fn json_key_rule_keeps_token_counts_and_redacts_credentials() {
        let mut value = serde_json::json!({
            "max_tokens": 4096,
            "input_tokens": 120,
            "access_token": "a",
            "refresh-token": "b",
            "token": "c"
        });
        redact_json_value(&mut value);
        assert_eq!(value["max_tokens"], 4096);
        assert_eq!(value["input_tokens"], 120);
        assert_eq!(value["access_token"], "[redacted]");
        assert_eq!(value["refresh-token"], "[redacted]");
        assert_eq!(value["token"], "[redacted]");
    }

    #[test]
    fn credential_only_redaction_keeps_paths() {
        let home = dirs::home_dir().expect("home");
        let text = format!("{}/.claude/settings.json token=abc", home.display());
        assert_eq!(
            super::redact_credentials(&text),
            format!("{}/.claude/settings.json token=[redacted]", home.display())
        );
    }

    // Mirrors the home table in agent-sdk/src/bridge/startup_failures.test.ts.
    #[test]
    fn replaces_the_home_directory_prefix_with_a_tilde() {
        let cases: &[(&str, &str, &str)] = &[
            ("/Users/alice", "open '/Users/alice/x'", "open '~/x'"),
            ("/Users/alice/", "open '/Users/alice/x'", "open '~/x'"),
            (
                "/Users/alice",
                "ENOENT /Users/alice/.claude/settings.json: missing",
                "ENOENT ~/.claude/settings.json: missing",
            ),
            ("/Users/alice", "in /Users/alice.", "in ~."),
            ("/Users/alice", "at file:///Users/alice/a.js:1", "at file://~/a.js:1"),
            ("/Users/alice", "a: /Users/alice and /Users/alice/b", "a: ~ and ~/b"),
            ("/Users/alice", "open '/Users/alice2/x'", "open '/Users/alice2/x'"),
            ("/Users/alice", "open '/Users/alice.bak/x'", "open '/Users/alice.bak/x'"),
            ("/Users/alice", "open '/srv/Users/alice/x'", "open '/srv/Users/alice/x'"),
            ("/Users/alice", "open '/Users/bob/x'", "open '/Users/bob/x'"),
            ("C:\\Users\\alice", r"open 'C:\Users\alice\x'", r"open '~\x'"),
            ("C:\\Users\\alice", "open 'C:/Users/alice/x'", "open '~/x'"),
            ("C:\\Users\\alice", "open 'c:\\users\\ALICE\\x'", "open '~\\x'"),
            ("C:\\Users\\alice", r"open 'C:\Users\alice2\x'", r"open 'C:\Users\alice2\x'"),
            ("C:\\Users\\alice", r#"{"p":"C:\\Users\\alice\\x"}"#, r#"{"p":"~\\x"}"#),
        ];
        for (home, input, expected) in cases {
            assert_eq!(
                redact_text_with_home(input, Some(Path::new(home))),
                *expected,
                "home: {home}, input: {input}"
            );
        }
        assert_eq!(redact_text_with_home("/Users/alice/x", None), "/Users/alice/x");
        assert_eq!(redact_text_with_home("/x", Some(Path::new("/"))), "/x");
    }

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

    // Mirrors the table in agent-sdk/src/bridge/startup_failures.test.ts.
    #[test]
    fn redacts_credential_shapes() {
        let cases: &[(&str, &str)] = &[
            ("GET /v1?token=abc&x=1", "GET /v1?token=[redacted]&x=1"),
            (
                "CLAUDE_CODE_OAUTH_TOKEN=sk-ant-oat01-abcDEF123",
                "CLAUDE_CODE_OAUTH_TOKEN=[redacted]",
            ),
            ("x-api-key: sk-ant-api03-secretvalue", "x-api-key: [redacted]"),
            ("access_token=abc123 next", "access_token=[redacted] next"),
            (
                "fetch https://user:pass@example.com/repo",
                "fetch https://[redacted]@example.com/repo",
            ),
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
        for (input, expected) in cases {
            assert_eq!(redact_text(input), *expected, "input: {input}");
        }
    }
}
