// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

/// Logical focus target that can claim directional key navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusTarget {
    Mention,
    Permission,
}

/// Effective owner of directional/navigation keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusOwner {
    Input,
    Mention,
    Permission,
}

#[derive(Debug, Clone, Copy)]
pub struct FocusContext {
    pub mention_active: bool,
    pub permission_active: bool,
}

impl FocusContext {
    #[must_use]
    pub const fn new(mention_active: bool, permission_active: bool) -> Self {
        Self { mention_active, permission_active }
    }

    #[must_use]
    pub const fn supports(self, target: FocusTarget) -> bool {
        match target {
            FocusTarget::Mention => self.mention_active,
            FocusTarget::Permission => self.permission_active,
        }
    }
}

impl From<FocusTarget> for FocusOwner {
    fn from(value: FocusTarget) -> Self {
        match value {
            FocusTarget::Mention => Self::Mention,
            FocusTarget::Permission => Self::Permission,
        }
    }
}

/// Focus claim manager:
/// latest valid claim wins; invalid claims are dropped during normalization.
#[derive(Debug, Clone, Default)]
pub struct FocusManager {
    stack: Vec<FocusTarget>,
}

impl FocusManager {
    /// Resolve the current focus owner for key routing.
    #[must_use]
    pub fn owner(&self, context: FocusContext) -> FocusOwner {
        for target in self.stack.iter().rev().copied() {
            if context.supports(target) {
                return target.into();
            }
        }
        FocusOwner::Input
    }

    /// Claim focus for the target. Latest valid claim wins.
    pub fn claim(&mut self, target: FocusTarget, context: FocusContext) {
        self.stack.retain(|t| *t != target);
        self.stack.push(target);
        self.normalize(context);
    }

    /// Release focus claim for the target.
    pub fn release(&mut self, target: FocusTarget, context: FocusContext) {
        if let Some(idx) = self.stack.iter().rposition(|t| *t == target) {
            self.stack.remove(idx);
        }
        self.normalize(context);
    }

    /// Remove claims no longer valid in the current context.
    pub fn normalize(&mut self, context: FocusContext) {
        self.stack.retain(|target| context.supports(*target));
    }
}

#[cfg(test)]
mod tests {
    use super::{FocusContext, FocusManager, FocusOwner, FocusTarget};

    #[test]
    fn owner_defaults_to_input_without_claims() {
        let mgr = FocusManager::default();
        let ctx = FocusContext::new(false, false);
        assert_eq!(mgr.owner(ctx), FocusOwner::Input);
    }

    #[test]
    fn latest_valid_claim_wins() {
        let mut mgr = FocusManager::default();
        let ctx = FocusContext::new(true, true);
        mgr.claim(FocusTarget::Permission, ctx);
        mgr.claim(FocusTarget::Mention, ctx);
        assert_eq!(mgr.owner(ctx), FocusOwner::Mention);
    }

    #[test]
    fn invalid_claims_are_normalized_out() {
        let mut mgr = FocusManager::default();
        let valid_ctx = FocusContext::new(true, false);
        let invalid_ctx = FocusContext::new(false, false);
        mgr.claim(FocusTarget::Mention, valid_ctx);
        assert_eq!(mgr.owner(valid_ctx), FocusOwner::Mention);
        mgr.normalize(invalid_ctx);
        assert_eq!(mgr.owner(invalid_ctx), FocusOwner::Input);
    }
}
