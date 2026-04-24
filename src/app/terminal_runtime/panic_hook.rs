// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use std::panic::PanicHookInfo;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

type PanicHandler = dyn Fn(&PanicHookInfo<'_>) + Send + Sync + 'static;

pub(crate) struct PanicRestoreHook {
    previous: Option<Box<PanicHandler>>,
}

pub(crate) fn restore_once<F>(restored: &AtomicBool, restore: F)
where
    F: FnOnce(),
{
    if restored.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
        restore();
    }
}

impl PanicRestoreHook {
    pub(crate) fn install<F>(restored: Arc<AtomicBool>, restore: F) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        let previous = std::panic::take_hook();
        let previous_shared = Arc::new(previous);
        let previous_for_hook = Arc::clone(&previous_shared);
        let previous_for_restore = Arc::clone(&previous_shared);
        let restore = Arc::new(restore);
        let restore_for_hook = Arc::clone(&restore);

        std::panic::set_hook(Box::new(move |panic_info| {
            restore_once(restored.as_ref(), || restore_for_hook.as_ref()());
            previous_for_hook.as_ref()(panic_info);
        }));

        Self {
            previous: Some(Box::new(move |panic_info| previous_for_restore.as_ref()(panic_info))),
        }
    }
}

impl Drop for PanicRestoreHook {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }

        let Some(previous) = self.previous.take() else {
            return;
        };

        let _current = std::panic::take_hook();
        std::panic::set_hook(previous);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static PANIC_HOOK_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn restore_once_only_runs_restore_logic_once() {
        let restored = AtomicBool::new(false);
        let restore_calls = AtomicUsize::new(0);

        restore_once(&restored, || {
            restore_calls.fetch_add(1, Ordering::SeqCst);
        });
        restore_once(&restored, || {
            restore_calls.fetch_add(1, Ordering::SeqCst);
        });

        assert_eq!(restore_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn panic_hook_restores_once_and_chains_previous_hook() {
        let _guard = PANIC_HOOK_TEST_LOCK.lock().expect("lock panic hook test");
        let original = panic::take_hook();
        let previous_calls = Arc::new(AtomicUsize::new(0));
        let previous_calls_for_hook = Arc::clone(&previous_calls);
        panic::set_hook(Box::new(move |_panic_info| {
            previous_calls_for_hook.fetch_add(1, Ordering::SeqCst);
        }));

        let restored = Arc::new(AtomicBool::new(false));
        let restore_calls = Arc::new(AtomicUsize::new(0));
        let restore_calls_for_hook = Arc::clone(&restore_calls);
        let hook = PanicRestoreHook::install(Arc::clone(&restored), move || {
            restore_calls_for_hook.fetch_add(1, Ordering::SeqCst);
        });

        let _ = panic::catch_unwind(|| {
            panic!("trigger hook");
        });

        assert_eq!(restore_calls.load(Ordering::SeqCst), 1);
        assert_eq!(previous_calls.load(Ordering::SeqCst), 1);

        drop(hook);

        let _ = panic::catch_unwind(|| {
            panic!("trigger restored previous hook");
        });

        assert_eq!(restore_calls.load(Ordering::SeqCst), 1);
        assert_eq!(previous_calls.load(Ordering::SeqCst), 2);

        let _installed = panic::take_hook();
        panic::set_hook(original);
    }
}
