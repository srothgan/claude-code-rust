use super::*;
use std::collections::HashSet;

#[test]
fn key_spec_from_event_accepts_standard_ctrl_v_encoding() {
    let key = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL);

    assert_eq!(KeySpec::from_event(key), Some("ctrl-v".parse().expect("parse key")));
}

#[test]
fn key_spec_from_event_accepts_raw_control_character_encoding() {
    let key = KeyEvent::new(KeyCode::Char('\u{16}'), KeyModifiers::NONE);

    assert_eq!(KeySpec::from_event(key), Some("ctrl-v".parse().expect("parse key")));
}

#[test]
fn key_spec_from_event_rejects_raw_control_character_with_alt_as_plain_ctrl() {
    let key = KeyEvent::new(KeyCode::Char('\u{16}'), KeyModifiers::ALT);

    assert_ne!(KeySpec::from_event(key), Some("ctrl-v".parse().expect("parse key")));
}

#[test]
fn key_spec_matching_uses_exact_modifiers() {
    let spec: KeySpec = "ctrl-v".parse().expect("parse key");
    let ctrl_alt_v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL | KeyModifiers::ALT);

    assert!(!spec.matches_event(ctrl_alt_v));
}

#[test]
fn key_spec_parse_and_display_are_canonical() {
    for (raw, canonical) in [
        ("ctrl-a", "ctrl-a"),
        ("alt-b", "alt-b"),
        ("shift-enter", "shift-enter"),
        ("cmd-z", "cmd-z"),
        ("esc", "esc"),
        ("backspace", "backspace"),
        ("delete", "delete"),
        ("ctrl-left", "ctrl-left"),
        ("option-f", "alt-f"),
        ("command-shift-z", "cmd-shift-z"),
    ] {
        let spec: KeySpec = raw.parse().expect("parse key");

        assert_eq!(spec.to_string(), canonical);
    }
}

#[test]
fn resolved_keymap_resolves_binding_in_the_right_context() {
    let keymap = ResolvedKeymap::from_bindings([KeyBinding::new(
        KeyContext::ChatInput,
        "ctrl-a".parse().expect("parse key"),
        KeyAction::Input(InputAction::MoveLineStart),
        KeyBindingSource::Default,
    )])
    .expect("build keymap");

    assert_eq!(
        keymap.action_for(KeyContext::ChatInput, &"ctrl-a".parse().expect("parse key")),
        Some(KeyAction::Input(InputAction::MoveLineStart))
    );
}

#[test]
fn resolved_keymap_resolves_global_fallback_with_metadata() {
    let keymap = ResolvedKeymap::from_bindings([KeyBinding::new(
        KeyContext::Global,
        "ctrl-q".parse().expect("parse key"),
        KeyAction::App(AppAction::Quit),
        KeyBindingSource::Default,
    )])
    .expect("build keymap");

    assert_eq!(
        keymap.resolve(KeyContext::ChatInput, &"ctrl-q".parse().expect("parse key")),
        Some(ResolvedKeyAction {
            action: KeyAction::App(AppAction::Quit),
            requested_context: KeyContext::ChatInput,
            matched_context: KeyContext::Global,
            source: KeyBindingSource::Default,
        })
    );
}

#[test]
fn resolved_keymap_help_bindings_follow_resolution_chain() {
    let keymap = ResolvedKeymap::from_bindings([
        KeyBinding::new(
            KeyContext::Global,
            "ctrl-q".parse().expect("parse key"),
            KeyAction::App(AppAction::Quit),
            KeyBindingSource::Default,
        ),
        KeyBinding::new(
            KeyContext::ChatInput,
            "ctrl-x".parse().expect("parse key"),
            KeyAction::App(AppAction::SubmitInput),
            KeyBindingSource::Default,
        ),
    ])
    .expect("build keymap");

    let bindings = keymap.help_bindings_for_context(KeyContext::ChatInput);

    assert_eq!(bindings.len(), 2);
    assert_eq!(bindings[0].spec, "ctrl-q".parse().expect("parse key"));
    assert_eq!(bindings[0].matched_context, KeyContext::Global);
    assert_eq!(bindings[0].descriptor().label, "Quit");
    assert_eq!(bindings[1].spec, "ctrl-x".parse().expect("parse key"));
    assert_eq!(bindings[1].matched_context, KeyContext::ChatInput);
    assert_eq!(bindings[1].descriptor().label, "Send message");
}

#[test]
fn resolved_keymap_does_not_resolve_binding_outside_resolution_chain() {
    let keymap = ResolvedKeymap::from_bindings([KeyBinding::new(
        KeyContext::ChatInput,
        "ctrl-a".parse().expect("parse key"),
        KeyAction::Input(InputAction::MoveLineStart),
        KeyBindingSource::Default,
    )])
    .expect("build keymap");

    assert_eq!(
        keymap.resolve(KeyContext::InlinePermission, &"ctrl-a".parse().expect("parse key")),
        None
    );
}

#[test]
fn resolved_keymap_rejects_duplicate_binding_in_same_context() {
    let result = ResolvedKeymap::from_bindings([
        KeyBinding::new(
            KeyContext::ChatInput,
            "ctrl-a".parse().expect("parse key"),
            KeyAction::Input(InputAction::MoveLineStart),
            KeyBindingSource::Default,
        ),
        KeyBinding::new(
            KeyContext::ChatInput,
            "ctrl-a".parse().expect("parse key"),
            KeyAction::App(AppAction::Redraw),
            KeyBindingSource::Default,
        ),
    ]);

    assert!(matches!(result, Err(KeymapBuildError::DuplicateBinding { .. })));
}

#[test]
fn resolved_keymap_rejects_shadowed_global_binding() {
    let result = ResolvedKeymap::from_bindings([
        KeyBinding::new(
            KeyContext::Global,
            "ctrl-g".parse().expect("parse key"),
            KeyAction::App(AppAction::CancelTurn),
            KeyBindingSource::Config,
        ),
        KeyBinding::new(
            KeyContext::ChatInput,
            "ctrl-g".parse().expect("parse key"),
            KeyAction::Input(InputAction::MoveLineStart),
            KeyBindingSource::Config,
        ),
    ]);

    assert!(matches!(result, Err(KeymapBuildError::ShadowedGlobalBinding { .. })));
}

#[test]
fn resolved_keymap_rejects_protected_global_action_conflict() {
    let result = ResolvedKeymap::from_bindings([
        KeyBinding::new(
            KeyContext::Global,
            "ctrl-q".parse().expect("parse key"),
            KeyAction::App(AppAction::Quit),
            KeyBindingSource::Default,
        ),
        KeyBinding::new(
            KeyContext::ChatInput,
            "ctrl-q".parse().expect("parse key"),
            KeyAction::Input(InputAction::MoveLineStart),
            KeyBindingSource::Config,
        ),
    ]);

    assert!(matches!(result, Err(KeymapBuildError::ProtectedGlobalActionConflict { .. })));
}

#[cfg(not(target_os = "macos"))]
#[test]
fn resolved_keymap_rejects_cmd_bindings_off_macos() {
    let result = ResolvedKeymap::from_bindings([KeyBinding::new(
        KeyContext::ChatInput,
        "cmd-z".parse().expect("parse key"),
        KeyAction::Input(InputAction::Undo),
        KeyBindingSource::Config,
    )]);

    assert!(matches!(result, Err(KeymapBuildError::PlatformInvalidBinding { .. })));
}

#[cfg(target_os = "windows")]
#[test]
fn resolved_keymap_rejects_ctrl_z_suspend_on_windows() {
    let result = ResolvedKeymap::from_bindings([KeyBinding::new(
        KeyContext::Global,
        "ctrl-z".parse().expect("parse key"),
        KeyAction::Terminal(TerminalAction::Suspend),
        KeyBindingSource::Config,
    )]);

    assert!(matches!(result, Err(KeymapBuildError::PlatformInvalidBinding { .. })));
}

#[test]
fn action_catalog_has_unique_stable_ids() {
    let mut ids = HashSet::new();

    for descriptor in action_catalog() {
        assert!(!descriptor.id.is_empty(), "{descriptor:?}");
        assert!(!descriptor.label.is_empty(), "{descriptor:?}");
        assert!(!descriptor.description.is_empty(), "{descriptor:?}");
        assert!(!descriptor.default_contexts.is_empty(), "{descriptor:?}");
        assert!(ids.insert(descriptor.id), "duplicate action id {}", descriptor.id);
        assert_eq!(KeyAction::from_id(descriptor.id), Some(descriptor.action));
        assert_eq!(descriptor.action.id(), descriptor.id);
        assert_eq!(descriptor.action.label(), descriptor.label);
        assert_eq!(descriptor.action.description(), descriptor.description);
    }

    assert_eq!(KeyAction::from_id("input.missing"), None);
}

#[test]
fn default_bindings_reference_catalogued_actions() {
    for binding in default_bindings() {
        let descriptor = action_descriptor(binding.action)
            .unwrap_or_else(|| panic!("missing descriptor for {:?}", binding.action));

        assert_eq!(descriptor.action, binding.action);
    }
}

#[test]
fn default_keymap_contains_chat_input_readline_bindings() {
    let keymap = ResolvedKeymap::defaults();

    assert_eq!(
        keymap.action_for_event(
            KeyContext::ChatInput,
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
        ),
        Some(KeyAction::Input(InputAction::Yank))
    );
    assert_eq!(
        keymap.action_for_event(
            KeyContext::ChatInput,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
        ),
        Some(KeyAction::Input(InputAction::MoveLineStart))
    );
}

#[test]
fn default_keymap_does_not_bind_permission_ctrl_shortcuts() {
    let keymap = ResolvedKeymap::defaults();

    for key in [
        KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
        KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
    ] {
        assert_eq!(keymap.action_for_event(KeyContext::InlinePermission, key), None);
    }
}

#[test]
fn default_keymap_resolves_enter_by_context() {
    let keymap = ResolvedKeymap::defaults();
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);

    assert_eq!(
        keymap.action_for_event(KeyContext::ChatInput, enter),
        Some(KeyAction::App(AppAction::SubmitInput))
    );
    assert_eq!(
        keymap.action_for_event(KeyContext::InlinePermission, enter),
        Some(KeyAction::Interaction(InteractionAction::Confirm))
    );
    assert_eq!(
        keymap.action_for_event(
            KeyContext::InlineQuestion,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        ),
        Some(KeyAction::Interaction(InteractionAction::ToggleSelection))
    );
    assert_eq!(
        keymap.action_for_event(
            KeyContext::InlineQuestion,
            KeyEvent::new(KeyCode::Home, KeyModifiers::NONE),
        ),
        Some(KeyAction::Interaction(InteractionAction::MoveStart))
    );
    assert_eq!(
        keymap.action_for_event(
            KeyContext::InlineQuestion,
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
        ),
        Some(KeyAction::Interaction(InteractionAction::ToggleNotes))
    );
}

#[test]
fn default_keymap_uses_platform_history_bindings() {
    let keymap = ResolvedKeymap::defaults();

    #[cfg(target_os = "macos")]
    {
        assert_eq!(
            keymap.action_for_event(
                KeyContext::ChatInput,
                KeyEvent::new(KeyCode::Char('z'), KeyModifiers::SUPER),
            ),
            Some(KeyAction::Input(InputAction::Undo))
        );
        assert_eq!(
            keymap.action_for_event(
                KeyContext::ChatInput,
                KeyEvent::new(KeyCode::Char('z'), KeyModifiers::SUPER | KeyModifiers::SHIFT),
            ),
            Some(KeyAction::Input(InputAction::Redo))
        );
    }

    #[cfg(target_os = "windows")]
    {
        assert_eq!(
            keymap.action_for_event(
                KeyContext::ChatInput,
                KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL),
            ),
            Some(KeyAction::Input(InputAction::Undo))
        );
        assert_eq!(
            keymap.action_for_event(
                KeyContext::ChatInput,
                KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL | KeyModifiers::SHIFT,),
            ),
            Some(KeyAction::Input(InputAction::Redo))
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        assert_eq!(
            keymap.action_for_event(
                KeyContext::ChatInput,
                KeyEvent::new(KeyCode::Char('_'), KeyModifiers::CONTROL),
            ),
            Some(KeyAction::Input(InputAction::Undo))
        );
        assert_eq!(
            keymap.action_for_event(
                KeyContext::ChatInput,
                KeyEvent::new(KeyCode::Char('/'), KeyModifiers::CONTROL),
            ),
            Some(KeyAction::Input(InputAction::Undo))
        );
    }

    #[cfg(unix)]
    {
        assert_eq!(
            keymap
                .resolve_event(
                    KeyContext::ChatInput,
                    KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL),
                )
                .map(|resolved| resolved.action),
            Some(KeyAction::Terminal(TerminalAction::Suspend))
        );
    }

    #[cfg(target_os = "windows")]
    {
        assert_ne!(
            keymap
                .resolve_event(
                    KeyContext::ChatInput,
                    KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL),
                )
                .map(|resolved| resolved.action),
            Some(KeyAction::Terminal(TerminalAction::Suspend))
        );
    }
}

#[test]
fn default_keymap_bindings_are_conflict_free() {
    ResolvedKeymap::validate_defaults().expect("default keymap should be conflict-free");
}
