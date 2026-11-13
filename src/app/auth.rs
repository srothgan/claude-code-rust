// SPDX-License-Identifier: Apache-2.0

use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[cfg(test)]
#[path = "auth_audit_tests.rs"]
mod audit_tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaudeOAuthCredentials {
    pub access_token: String,
    pub expires_at: Option<SystemTime>,
}

/// File credentials in the active Claude configuration directory.
pub(crate) fn credentials_path() -> Option<PathBuf> {
    crate::claude_paths::ClaudePaths::resolve(None).map(|paths| paths.credentials())
}

pub(crate) fn load_oauth_credentials() -> Option<ClaudeOAuthCredentials> {
    let path = credentials_path()?;
    load_oauth_credentials_at(&path)
}

/// Returns `true` when valid OAuth credentials exist on disk.
///
/// Reads the active profile's `.credentials.json` and checks that
/// `claudeAiOauth.accessToken` is a non-empty string.
pub fn has_credentials() -> bool {
    load_oauth_credentials().is_some()
}

fn load_oauth_credentials_at(path: &Path) -> Option<ClaudeOAuthCredentials> {
    let contents = std::fs::read_to_string(path).ok()?;
    let json = serde_json::from_str::<serde_json::Value>(&contents).ok()?;
    parse_oauth_credentials(&json)
}

fn parse_oauth_credentials(json: &serde_json::Value) -> Option<ClaudeOAuthCredentials> {
    let oauth = json.get("claudeAiOauth")?;
    let access_token = oauth.get("accessToken")?.as_str()?.trim();
    if access_token.is_empty() {
        return None;
    }

    Some(ClaudeOAuthCredentials {
        access_token: access_token.to_owned(),
        expires_at: oauth.get("expiresAt").and_then(crate::epoch_time::parse_value),
    })
}
