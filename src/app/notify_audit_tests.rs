// SPDX-License-Identifier: Apache-2.0
use super::*;

#[test]
fn terminal_delivery_uses_safe_independent_protocol_bytes_and_exclusive_fallbacks() {
    use PreferredNotifChannel as C;
    use TerminalCapabilities as T;
    #[derive(Default)]
    struct Sink {
        bytes: Vec<u8>,
        flushed: bool,
    }
    impl std::io::Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.flushed = true;
            Ok(())
        }
    }
    for (channel, terminal, sequence, bell, desktop) in [
        (C::Iterm2, T::Iterm2, "\x1b]9;Claude Code: 夜   Lab\x1b\\", false, false),
        (C::Ghostty, T::Ghostty, "\x1b]9;Claude Code: 夜   Lab\x1b\\", false, false),
        (C::Kitty, T::Kitty, "\x1b]99;;Claude Code: 夜   Lab\x1b\\", false, false),
        (C::Auto, T::Iterm2, "\x1b]9;Claude Code: 夜   Lab\x1b\\", false, false),
        (C::Auto, T::Ghostty, "\x1b]9;Claude Code: 夜   Lab\x1b\\", false, false),
        (C::Auto, T::Kitty, "\x1b]99;;Claude Code: 夜   Lab\x1b\\", false, false),
        (C::Auto, T::Other, "\x07", true, true),
        (C::Iterm2, T::Other, "", false, true),
        (C::Ghostty, T::Kitty, "", false, true),
        (C::Kitty, T::Iterm2, "", false, true),
        (C::Iterm2WithBell, T::Other, "\x07", true, true),
        (C::Iterm2WithBell, T::Iterm2, "\x1b]9;Claude Code: 夜   Lab\x1b\\\x07", true, false),
        (C::TerminalBell, T::Kitty, "\x07", true, false),
        (C::NotificationsDisabled, T::Iterm2, "", false, false),
    ] {
        let plan = notification_plan(channel, terminal, "夜\n\r\t\x1b\x07\u{9c}Lab");
        assert_eq!(plan.body, "夜   Lab");
        assert_eq!((plan.ring_bell, plan.send_desktop), (bell, desktop));
        assert_eq!(plan.escape_sequence.is_some(), sequence.starts_with('\x1b'));
        let mut sink = Sink::default();
        deliver_terminal_notification(&plan, &mut sink);
        assert_eq!(sink.bytes, sequence.as_bytes(), "{channel:?} {terminal:?}");
        if !sequence.is_empty() {
            assert!(sink.flushed);
        }
    }
}
