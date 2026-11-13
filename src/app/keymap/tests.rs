// SPDX-License-Identifier: Apache-2.0
use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn fixture_binding(context: KeyContext, spec: KeySpec, action: KeyAction) -> KeyBinding {
    KeyBinding::new(context, spec, action, KeyBindingSource::Config)
}

#[test]
fn event_resolution_normalizes_encodings_without_losing_modifier_identity() {
    use InputAction::{MoveCharLeft, MoveCharRight, MoveLineEnd, MoveLineStart, Undo};
    let keymap = ResolvedKeymap::from_bindings([
        fixture_binding(
            KeyContext::ChatInput,
            KeySpec::char('v', KeyModifiers::CONTROL),
            KeyAction::Input(MoveLineStart),
        ),
        fixture_binding(
            KeyContext::ChatInput,
            KeySpec::char('a', KeyModifiers::CONTROL),
            KeyAction::Input(MoveLineEnd),
        ),
        fixture_binding(
            KeyContext::ChatInput,
            KeySpec::char('v', KeyModifiers::CONTROL | KeyModifiers::SHIFT),
            KeyAction::Input(Undo),
        ),
        fixture_binding(
            KeyContext::ChatInput,
            KeySpec::new(KeyCodeSpec::Tab, KeyModifiers::SHIFT),
            KeyAction::Input(MoveCharRight),
        ),
        fixture_binding(
            KeyContext::ChatInput,
            KeySpec::new(KeyCodeSpec::Tab, KeyModifiers::NONE),
            KeyAction::Input(MoveCharLeft),
        ),
    ])
    .expect("disjoint typed bindings");
    for (code, modifiers, expected) in [
        (KeyCode::Char('v'), KeyModifiers::CONTROL, Some(MoveLineStart)),
        (KeyCode::Char('V'), KeyModifiers::CONTROL, Some(MoveLineStart)),
        (KeyCode::Char('\u{16}'), KeyModifiers::NONE, Some(MoveLineStart)),
        (KeyCode::Char('a'), KeyModifiers::CONTROL, Some(MoveLineEnd)),
        (KeyCode::Char('\u{1}'), KeyModifiers::NONE, Some(MoveLineEnd)),
        (KeyCode::Char('v'), KeyModifiers::CONTROL | KeyModifiers::SHIFT, Some(Undo)),
        (KeyCode::Char('V'), KeyModifiers::CONTROL | KeyModifiers::SHIFT, Some(Undo)),
        (KeyCode::Char('v'), KeyModifiers::CONTROL | KeyModifiers::ALT, None),
        (KeyCode::Char('\u{16}'), KeyModifiers::ALT, None),
        (KeyCode::Char('a'), KeyModifiers::ALT, None),
        (KeyCode::BackTab, KeyModifiers::NONE, Some(MoveCharRight)),
        (KeyCode::BackTab, KeyModifiers::SHIFT, Some(MoveCharRight)),
        (KeyCode::Tab, KeyModifiers::SHIFT, Some(MoveCharRight)),
        (KeyCode::Tab, KeyModifiers::NONE, Some(MoveCharLeft)),
    ] {
        let event = KeyEvent::new(code, modifiers);
        assert_eq!(
            keymap.resolve_event(KeyContext::ChatInput, event).map(|resolved| resolved.action),
            expected.map(KeyAction::Input),
            "{event:?}"
        );
    }
}

fn context_fixture() -> ResolvedKeymap {
    ResolvedKeymap::from_bindings([
        fixture_binding(
            KeyContext::ChatInput,
            KeySpec::char('x', KeyModifiers::CONTROL),
            KeyAction::Input(InputAction::Undo),
        ),
        fixture_binding(
            KeyContext::InlinePermission,
            KeySpec::char('x', KeyModifiers::CONTROL),
            KeyAction::Interaction(InteractionAction::Confirm),
        ),
        fixture_binding(
            KeyContext::InlineQuestion,
            KeySpec::char('j', KeyModifiers::CONTROL),
            KeyAction::Interaction(InteractionAction::MoveNext),
        ),
        fixture_binding(
            KeyContext::Global,
            KeySpec::char('q', KeyModifiers::CONTROL),
            KeyAction::App(AppAction::Quit),
        ),
    ])
    .expect("sibling contexts may reuse keys")
}

#[test]
fn effective_context_resolution_keeps_siblings_isolated_and_global_actions_available() {
    let keymap = context_fixture();
    for (context, ch, expected) in [
        (
            KeyContext::ChatInput,
            'x',
            Some((KeyAction::Input(InputAction::Undo), KeyContext::ChatInput)),
        ),
        (
            KeyContext::InlinePermission,
            'x',
            Some((
                KeyAction::Interaction(InteractionAction::Confirm),
                KeyContext::InlinePermission,
            )),
        ),
        (
            KeyContext::InlineQuestion,
            'j',
            Some((KeyAction::Interaction(InteractionAction::MoveNext), KeyContext::InlineQuestion)),
        ),
        (KeyContext::InlinePermission, 'j', None),
        (KeyContext::InlineQuestion, 'x', None),
        (KeyContext::ChatInput, 'j', None),
        (KeyContext::ChatBlocked, 'x', None),
        (KeyContext::ChatInput, 'w', None),
    ] {
        assert_eq!(
            keymap
                .resolve_event(context, KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL))
                .map(|r| (r.action, r.matched_context)),
            expected
        );
    }
    for context in [
        KeyContext::ChatBlocked,
        KeyContext::ChatInput,
        KeyContext::AutocompleteMention,
        KeyContext::AutocompleteSlash,
        KeyContext::AutocompleteSubagent,
        KeyContext::InlinePermission,
        KeyContext::InlineQuestion,
    ] {
        let resolved = keymap
            .resolve_event(context, KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL))
            .expect("global fallback");
        assert_eq!(
            (resolved.action, resolved.matched_context),
            (KeyAction::App(AppAction::Quit), KeyContext::Global)
        );
    }
}

#[test]
fn effective_help_preserves_key_action_context_associations_without_sibling_leaks() {
    let keymap = context_fixture();
    for (context, local) in [
        (KeyContext::ChatInput, Some(('x', KeyAction::Input(InputAction::Undo)))),
        (
            KeyContext::InlinePermission,
            Some(('x', KeyAction::Interaction(InteractionAction::Confirm))),
        ),
        (
            KeyContext::InlineQuestion,
            Some(('j', KeyAction::Interaction(InteractionAction::MoveNext))),
        ),
        (KeyContext::ChatBlocked, None),
    ] {
        let mut expected = vec![(
            KeySpec::char('q', KeyModifiers::CONTROL),
            KeyAction::App(AppAction::Quit),
            KeyContext::Global,
        )];
        if let Some((ch, action)) = local {
            expected.push((KeySpec::char(ch, KeyModifiers::CONTROL), action, context));
        }
        let actual: Vec<_> = keymap
            .help_bindings_for_context(context)
            .into_iter()
            .map(|b| (b.spec, b.action, b.matched_context))
            .collect();
        for association in &expected {
            assert_eq!(
                actual.iter().filter(|value| *value == association).count(),
                1,
                "each effective binding advertised once"
            );
        }
        assert!(
            actual.iter().all(|association| expected.contains(association)),
            "no sibling-only help"
        );
    }
}

#[test]
fn conflicting_bindings_report_the_rejected_key_context_and_actions_in_either_order() {
    let spec = KeySpec::char('g', KeyModifiers::CONTROL);
    let local = KeyAction::Input(InputAction::MoveLineStart);
    for reverse in [false, true] {
        let mut duplicates = vec![
            fixture_binding(KeyContext::ChatInput, spec.clone(), local),
            fixture_binding(
                KeyContext::ChatInput,
                spec.clone(),
                KeyAction::Input(InputAction::Undo),
            ),
        ];
        if reverse {
            duplicates.reverse();
        }
        let expected = KeymapBuildError::DuplicateBinding {
            context: KeyContext::ChatInput,
            spec: spec.clone(),
            existing_action: if reverse { KeyAction::Input(InputAction::Undo) } else { local },
            duplicate_action: if reverse { local } else { KeyAction::Input(InputAction::Undo) },
        };
        assert_eq!(
            ResolvedKeymap::from_bindings(duplicates).expect_err("duplicate collision"),
            expected
        );
        for global in [
            KeyAction::App(AppAction::CancelTurn),
            KeyAction::App(AppAction::Quit),
            KeyAction::App(AppAction::Redraw),
            KeyAction::Terminal(TerminalAction::Suspend),
        ] {
            let mut bindings = vec![
                fixture_binding(KeyContext::ChatInput, spec.clone(), local),
                fixture_binding(KeyContext::Global, spec.clone(), global),
            ];
            if reverse {
                bindings.reverse();
            }
            let expected = if global == KeyAction::App(AppAction::CancelTurn) {
                KeymapBuildError::ShadowedGlobalBinding {
                    context: KeyContext::ChatInput,
                    spec: spec.clone(),
                    context_action: local,
                    global_action: global,
                }
            } else {
                KeymapBuildError::ProtectedGlobalActionConflict {
                    context: KeyContext::ChatInput,
                    spec: spec.clone(),
                    action: global,
                }
            };
            assert_eq!(
                ResolvedKeymap::from_bindings(bindings).expect_err("global collision"),
                expected
            );
        }
    }
    let accepted = context_fixture();
    assert_eq!(
        accepted
            .resolve_event(
                KeyContext::ChatInput,
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)
            )
            .expect("valid sibling reuse")
            .action,
        KeyAction::Input(InputAction::Undo)
    );
}

#[test]
fn platform_admission_preserves_supported_history_bindings() {
    let context = KeyContext::ChatInput;
    let undo = KeyAction::Input(InputAction::Undo);
    let ordinary = KeySpec::char('u', KeyModifiers::CONTROL);
    let keymap = ResolvedKeymap::from_bindings([fixture_binding(context, ordinary, undo)])
        .expect("ordinary control binding accepted");
    assert_eq!(
        keymap
            .resolve_event(context, KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
            .expect("ordinary control")
            .action,
        undo
    );
    let command = KeySpec::char('z', KeyModifiers::SUPER);
    let result = ResolvedKeymap::from_bindings([fixture_binding(context, command.clone(), undo)]);
    if cfg!(target_os = "macos") {
        assert_eq!(
            result
                .expect("native Cmd binding")
                .resolve_event(context, KeyEvent::new(KeyCode::Char('z'), KeyModifiers::SUPER))
                .expect("Cmd undo")
                .action,
            undo
        );
    } else {
        assert!(
            matches!(result, Err(KeymapBuildError::PlatformInvalidBinding {context:actual, spec, ..}) if actual == context && spec == command)
        );
    }
    let control_z = KeySpec::char('z', KeyModifiers::CONTROL);
    ResolvedKeymap::from_bindings([fixture_binding(context, control_z.clone(), undo)])
        .expect("Ctrl+Z Undo remains constructible");
    let suspend = ResolvedKeymap::from_bindings([fixture_binding(
        KeyContext::Global,
        control_z.clone(),
        KeyAction::Terminal(TerminalAction::Suspend),
    )]);
    if cfg!(target_os = "windows") {
        assert!(
            matches!(suspend, Err(KeymapBuildError::PlatformInvalidBinding {context:KeyContext::Global, spec, ..}) if spec == control_z)
        );
    } else {
        assert_eq!(
            suspend
                .expect("Unix suspend binding")
                .resolve_event(context, KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL))
                .expect("suspend")
                .action,
            KeyAction::Terminal(TerminalAction::Suspend)
        );
    }
}
