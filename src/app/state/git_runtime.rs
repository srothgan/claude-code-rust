// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::prelude::*;

impl App {
    #[must_use]
    pub fn git_branch(&self) -> Option<&str> {
        self.git_context.branch_name()
    }

    pub fn sync_git_context(&mut self) {
        if self.git_context.sync_to_cwd(Path::new(&self.cwd_raw)) {
            self.request_chat_repaint();
        }
    }

    pub fn tick_git_context(&mut self, now: Instant) {
        if self.git_context.tick(Path::new(&self.cwd_raw), now) {
            self.request_chat_repaint();
        }
    }

    #[cfg(test)]
    pub fn set_git_branch_for_test(&mut self, branch: Option<&str>) {
        self.git_context.set_branch_for_test(branch);
    }
}
