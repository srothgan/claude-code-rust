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
    fn claims_restore_prior_owners_without_resurrecting_released_or_invalid_claims() {
        use FocusOwner::{Input, Mention, Permission};
        let both = FocusContext::new(true, true);
        for (first, second, first_owner, second_owner) in [
            (FocusTarget::Permission, FocusTarget::Mention, Permission, Mention),
            (FocusTarget::Mention, FocusTarget::Permission, Mention, Permission),
        ] {
            let mut manager = FocusManager::default();
            manager.claim(first, both);
            assert_eq!(manager.owner(both), first_owner);
            manager.claim(second, both);
            assert_eq!(manager.owner(both), second_owner);
            manager.release(second, both);
            assert_eq!(manager.owner(both), first_owner);
            manager.claim(second, both);
            manager.claim(first, both);
            assert_eq!(manager.owner(both), first_owner);
            manager.release(first, both);
            assert_eq!(manager.owner(both), second_owner);
            manager.release(second, both);
            assert_eq!(manager.owner(both), Input, "reclaim must not leave duplicate claims");

            manager.claim(first, both);
            manager.claim(second, both);
            let only_first =
                FocusContext::new(first == FocusTarget::Mention, first == FocusTarget::Permission);
            assert_eq!(manager.owner(only_first), first_owner, "unavailable top is skipped");
            manager.normalize(only_first);
            assert_eq!(
                manager.owner(both),
                first_owner,
                "normalization permanently drops unavailable claims"
            );
            manager.release(first, both);
            assert_eq!(manager.owner(both), Input);
            manager.claim(second, both);
            assert_eq!(manager.owner(both), second_owner, "explicit reclaim restores availability");
        }
    }
}
