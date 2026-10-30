// SPDX-License-Identifier: Apache-2.0

use std::path::Path;

/// Abbreviate only a complete home path, without changing the session's raw identity.
pub(super) fn display(cwd: &Path, home: Option<&Path>) -> String {
    if let Some(relative) = home.and_then(|home| cwd.strip_prefix(home).ok()) {
        if relative.as_os_str().is_empty() {
            return "~".to_owned();
        }
        return Path::new("~").join(relative).to_string_lossy().into_owned();
    }
    cwd.to_string_lossy().into_owned()
}
