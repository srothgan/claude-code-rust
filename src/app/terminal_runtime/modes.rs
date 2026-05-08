// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crossterm::cursor::{Hide, Show};
use crossterm::event::{DisableFocusChange, DisableMouseCapture, EnableFocusChange};
#[cfg(target_os = "macos")]
use crossterm::event::{
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    DisableLineWrap, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use std::io::Stdout;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TerminalModeAction {
    EnableRawMode,
    DisableRawMode,
    EnterAlternateScreen,
    LeaveAlternateScreen,
    DisableMouseCapture,
    EnableFocusChange,
    DisableFocusChange,
    HideCursor,
    ShowCursor,
    DisableLineWrap,
    EnableLineWrap,
    #[cfg(target_os = "macos")]
    PushKeyboardEnhancement,
    #[cfg(target_os = "macos")]
    PopKeyboardEnhancement,
}

const CHAT_STARTUP_ACTIONS: &[TerminalModeAction] = &[
    TerminalModeAction::EnableRawMode,
    #[cfg(target_os = "macos")]
    TerminalModeAction::PushKeyboardEnhancement,
    TerminalModeAction::EnableFocusChange,
    TerminalModeAction::DisableLineWrap,
    TerminalModeAction::ShowCursor,
];

const ENTER_FULLSCREEN_ACTIONS: &[TerminalModeAction] = &[
    TerminalModeAction::HideCursor,
    TerminalModeAction::EnableLineWrap,
    TerminalModeAction::EnterAlternateScreen,
];

const EXIT_FULLSCREEN_ACTIONS: &[TerminalModeAction] = &[
    TerminalModeAction::LeaveAlternateScreen,
    TerminalModeAction::DisableLineWrap,
    TerminalModeAction::ShowCursor,
];

const SHUTDOWN_RESTORE_ACTIONS: &[TerminalModeAction] = &[
    TerminalModeAction::ShowCursor,
    TerminalModeAction::EnableLineWrap,
    TerminalModeAction::DisableMouseCapture,
    TerminalModeAction::DisableFocusChange,
    #[cfg(target_os = "macos")]
    TerminalModeAction::PopKeyboardEnhancement,
    TerminalModeAction::DisableRawMode,
];

const RELEASE_TO_CHILD_ACTIONS: &[TerminalModeAction] = &[
    TerminalModeAction::ShowCursor,
    TerminalModeAction::EnableLineWrap,
    TerminalModeAction::DisableFocusChange,
    #[cfg(target_os = "macos")]
    TerminalModeAction::PopKeyboardEnhancement,
    TerminalModeAction::DisableRawMode,
];

const RETURN_FROM_CHILD_ACTIONS: &[TerminalModeAction] = CHAT_STARTUP_ACTIONS;

pub(super) fn chat_startup_actions() -> &'static [TerminalModeAction] {
    CHAT_STARTUP_ACTIONS
}

pub(super) fn enter_fullscreen_actions() -> &'static [TerminalModeAction] {
    ENTER_FULLSCREEN_ACTIONS
}

pub(super) fn exit_fullscreen_actions() -> &'static [TerminalModeAction] {
    EXIT_FULLSCREEN_ACTIONS
}

pub(super) fn shutdown_restore_actions() -> &'static [TerminalModeAction] {
    SHUTDOWN_RESTORE_ACTIONS
}

pub(super) fn release_to_child_actions() -> &'static [TerminalModeAction] {
    RELEASE_TO_CHILD_ACTIONS
}

pub(super) fn return_from_child_actions() -> &'static [TerminalModeAction] {
    RETURN_FROM_CHILD_ACTIONS
}

pub(super) fn apply_actions(
    stdout: &mut Stdout,
    actions: &[TerminalModeAction],
) -> std::io::Result<()> {
    let mut first_error = None;

    for action in actions {
        let result = match action {
            TerminalModeAction::EnableRawMode => enable_raw_mode(),
            TerminalModeAction::DisableRawMode => disable_raw_mode(),
            TerminalModeAction::EnterAlternateScreen => execute!(stdout, EnterAlternateScreen),
            TerminalModeAction::LeaveAlternateScreen => execute!(stdout, LeaveAlternateScreen),
            TerminalModeAction::DisableMouseCapture => execute!(stdout, DisableMouseCapture),
            TerminalModeAction::EnableFocusChange => execute!(stdout, EnableFocusChange),
            TerminalModeAction::DisableFocusChange => execute!(stdout, DisableFocusChange),
            TerminalModeAction::HideCursor => execute!(stdout, Hide),
            TerminalModeAction::ShowCursor => execute!(stdout, Show),
            TerminalModeAction::DisableLineWrap => execute!(stdout, DisableLineWrap),
            TerminalModeAction::EnableLineWrap => {
                execute!(stdout, crossterm::terminal::EnableLineWrap)
            }
            #[cfg(target_os = "macos")]
            TerminalModeAction::PushKeyboardEnhancement => execute!(
                stdout,
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            ),
            #[cfg(target_os = "macos")]
            TerminalModeAction::PopKeyboardEnhancement => {
                execute!(stdout, PopKeyboardEnhancementFlags)
            }
        };

        if let Err(err) = result
            && first_error.is_none()
        {
            let kind = err.kind();
            first_error = Some(std::io::Error::new(
                kind,
                format!("terminal action {action:?} failed: {err}"),
            ));
        }
    }

    match first_error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_startup_actions_match_expected_order() {
        assert_eq!(
            chat_startup_actions(),
            &[
                TerminalModeAction::EnableRawMode,
                #[cfg(target_os = "macos")]
                TerminalModeAction::PushKeyboardEnhancement,
                TerminalModeAction::EnableFocusChange,
                TerminalModeAction::DisableLineWrap,
                TerminalModeAction::ShowCursor,
            ]
        );
    }

    #[test]
    fn enter_fullscreen_actions_match_expected_order() {
        assert_eq!(
            enter_fullscreen_actions(),
            &[
                TerminalModeAction::HideCursor,
                TerminalModeAction::EnableLineWrap,
                TerminalModeAction::EnterAlternateScreen,
            ]
        );
    }

    #[test]
    fn exit_fullscreen_actions_match_expected_order() {
        assert_eq!(
            exit_fullscreen_actions(),
            &[
                TerminalModeAction::LeaveAlternateScreen,
                TerminalModeAction::DisableLineWrap,
                TerminalModeAction::ShowCursor,
            ]
        );
    }

    #[test]
    fn shutdown_restore_actions_match_expected_order() {
        assert_eq!(
            shutdown_restore_actions(),
            &[
                TerminalModeAction::ShowCursor,
                TerminalModeAction::EnableLineWrap,
                TerminalModeAction::DisableMouseCapture,
                TerminalModeAction::DisableFocusChange,
                #[cfg(target_os = "macos")]
                TerminalModeAction::PopKeyboardEnhancement,
                TerminalModeAction::DisableRawMode,
            ]
        );
    }

    #[test]
    fn release_to_child_actions_match_expected_order() {
        assert_eq!(
            release_to_child_actions(),
            &[
                TerminalModeAction::ShowCursor,
                TerminalModeAction::EnableLineWrap,
                TerminalModeAction::DisableFocusChange,
                #[cfg(target_os = "macos")]
                TerminalModeAction::PopKeyboardEnhancement,
                TerminalModeAction::DisableRawMode,
            ]
        );
    }

    #[test]
    fn return_from_child_actions_match_chat_startup() {
        assert_eq!(return_from_child_actions(), chat_startup_actions());
    }

    #[test]
    fn chat_startup_actions_disable_line_wrap_for_inline_chat() {
        assert!(
            chat_startup_actions().contains(&TerminalModeAction::DisableLineWrap),
            "inline chat must explicitly disable terminal line wrapping",
        );
    }

    #[test]
    fn chat_startup_actions_do_not_enter_alternate_screen() {
        assert!(!chat_startup_actions().contains(&TerminalModeAction::EnterAlternateScreen));
    }

    #[test]
    fn shutdown_restore_actions_restore_shell_friendly_defaults() {
        assert!(shutdown_restore_actions().contains(&TerminalModeAction::ShowCursor));
        assert!(shutdown_restore_actions().contains(&TerminalModeAction::EnableLineWrap));
        #[cfg(target_os = "macos")]
        assert!(shutdown_restore_actions().contains(&TerminalModeAction::PopKeyboardEnhancement));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn chat_startup_actions_enable_keyboard_enhancement() {
        assert!(chat_startup_actions().contains(&TerminalModeAction::PushKeyboardEnhancement));
    }
}
