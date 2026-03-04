// Claude Code Rust - A native Rust terminal interface for Claude Code
// Copyright (C) 2025  Simon Peter Rothgang
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use std::path::PathBuf;

/// Resolved path to `~/.claude/.credentials.json`.
fn credentials_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join(".credentials.json"))
}

/// Returns `true` when valid OAuth credentials exist on disk.
///
/// Reads `~/.claude/.credentials.json` and checks that
/// `claudeAiOauth.accessToken` is a non-empty string.
pub fn has_credentials() -> bool {
    let Some(path) = credentials_path() else {
        return false;
    };
    has_credentials_at(&path)
}

/// Testable inner function that checks a specific file path.
fn has_credentials_at(path: &std::path::Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return false;
    };
    json.get("claudeAiOauth")
        .and_then(|oauth| oauth.get("accessToken"))
        .and_then(|t| t.as_str())
        .is_some_and(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn returns_false_for_nonexistent_file() {
        let path = std::path::Path::new("/tmp/claude_test_nonexistent_credentials.json");
        assert!(!has_credentials_at(path));
    }

    #[test]
    fn returns_false_for_empty_json() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        write!(tmp, "{{}}").unwrap();
        assert!(!has_credentials_at(tmp.path()));
    }

    #[test]
    fn returns_false_for_empty_access_token() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        write!(tmp, r#"{{"claudeAiOauth":{{"accessToken":"","refreshToken":"tok"}}}}"#).unwrap();
        assert!(!has_credentials_at(tmp.path()));
    }

    #[test]
    fn returns_true_for_valid_oauth() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        write!(
            tmp,
            r#"{{"claudeAiOauth":{{"accessToken":"sk-ant-oat01-test","refreshToken":"sk-ant-ort01-test","expiresAt":9999999999999}}}}"#
        )
        .unwrap();
        assert!(has_credentials_at(tmp.path()));
    }

    #[test]
    fn returns_false_for_malformed_json() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        write!(tmp, "not json at all").unwrap();
        assert!(!has_credentials_at(tmp.path()));
    }
}
