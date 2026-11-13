// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::app::keymap::{
    AppAction, KeyAction, KeyBinding, KeyBindingSource, KeyContext, ResolvedKeymap,
};
use std::collections::BTreeMap;

fn document(app: &mut App, input: &str) -> String {
    app.input.set_text(input);
    crate::app::input_submit::submit_input(app);
    let message = app.transcript.messages.last().expect("documentation response");
    let Some(MessageBlock::Text(text)) = message.blocks.first() else {
        panic!("documentation text")
    };
    text.text.clone()
}

fn table(markdown: &str) -> BTreeMap<String, String> {
    let mut rows = BTreeMap::new();
    for line in markdown.lines().filter(|line| line.trim_start().starts_with('|')) {
        let cells = line.trim().trim_matches('|').split('|').map(str::trim).collect::<Vec<_>>();
        if cells.len() == 2 {
            assert!(
                rows.insert(cells[0].to_owned(), cells[1].to_owned()).is_none(),
                "duplicate row: {line}"
            );
        }
    }
    rows
}

#[test]
fn model_docs_associate_each_identity_description_and_supported_capability() {
    let mut app = App::test_default();
    app.sdk_inventory.available_models = vec![
        model::AvailableModel::new("reasoning-id", "Reasoning model")
            .description("Distinct reasoning description")
            .supports_effort(true)
            .supported_effort_levels(vec![model::EffortLevel::Low, model::EffortLevel::Max])
            .supports_adaptive_thinking(Some(true))
            .supports_fast_mode(Some(false))
            .supports_auto_mode(Some(true)),
        model::AvailableModel::new("quick-id", "Quick model")
            .description("Distinct quick description")
            .supports_fast_mode(Some(true))
            .supports_adaptive_thinking(Some(false))
            .supports_auto_mode(Some(false)),
        model::AvailableModel::new("plain-id", "Plain model")
            .description("Distinct plain description"),
    ];
    let rows = table(&document(&mut app, "/docs models"));
    for (name, id, description, yes, no) in [
        (
            "Reasoning model",
            "reasoning-id",
            "Distinct reasoning description",
            vec!["Effort", "Low", "Max", "Adaptive thinking", "Auto mode"],
            vec!["Fast mode", "High", "Medium"],
        ),
        (
            "Quick model",
            "quick-id",
            "Distinct quick description",
            vec!["Fast mode"],
            vec!["Effort", "Adaptive thinking", "Auto mode"],
        ),
        (
            "Plain model",
            "plain-id",
            "Distinct plain description",
            vec![],
            vec!["Effort", "Adaptive thinking", "Auto mode", "Fast mode"],
        ),
    ] {
        let details = rows.get(name).expect("model row");
        assert!(details.contains(id) && details.contains(description), "{name}: {details}");
        for capability in yes {
            assert!(details.contains(capability), "{name}: missing {capability}");
        }
        for capability in no {
            assert!(!details.contains(capability), "{name}: unexpected {capability}");
        }
        for other in
            ["reasoning-id", "quick-id", "plain-id"].into_iter().filter(|other| *other != id)
        {
            assert!(!details.contains(other), "{name}: foreign ID {other}");
        }
    }
}

#[test]
fn command_docs_keep_app_ownership_and_associate_remote_descriptions() {
    let mut app = App::test_default();
    app.sdk_inventory.available_commands = vec![
        model::AvailableCommand::new("/config", "Foreign config metadata"),
        model::AvailableCommand::new("/fast", "Foreign fast metadata"),
        model::AvailableCommand::new("/deploy", "Deploy release to staging"),
        model::AvailableCommand::new("/verify", "Verify release signatures"),
    ];
    let rows = table(&document(&mut app, "/docs commands"));
    assert_eq!(rows.get("/deploy").map(String::as_str), Some("Deploy release to staging"));
    assert_eq!(rows.get("/verify").map(String::as_str), Some("Verify release signatures"));
    let config = rows.get("/config").expect("app config row").to_lowercase();
    assert!(config.contains("settings"), "{config}");
    let fast = rows.get("/fast").expect("app fast row").to_lowercase();
    assert!(
        fast.contains("fast mode") && fast.contains("enable") && fast.contains("disable"),
        "{fast}"
    );
    for (command, concepts) in [
        ("/btw", &["contextual", "question"][..]),
        ("/cancel", &["cancel", "turn"][..]),
        ("/compact", &["compact", "context"][..]),
        ("/docs", &["documentation"][..]),
        ("/help", &["help"][..]),
        ("/mode", &["modes", "session"][..]),
        ("/model", &["models", "session"][..]),
        ("/new-session", &["new", "session"][..]),
        ("/resume", &["session", "picker"][..]),
        ("/rewind", &["restore", "conversation", "code"][..]),
    ] {
        let description = rows.get(command).expect("app command row").to_lowercase();
        for concept in concepts {
            assert!(description.contains(concept), "{command}: missing {concept} in {description}");
        }
    }
    assert!(!rows.values().any(|description| description.contains("Foreign")));
}

#[test]
fn shortcut_docs_follow_the_remapped_submit_binding_at_its_action_row() {
    let mut app = App::test_default();
    let before = table(&document(&mut app, "/docs shortcuts"));
    assert_eq!(before.get("Enter").map(String::as_str), Some("Send message"));
    app.keymap = ResolvedKeymap::from_bindings([KeyBinding::new(
        KeyContext::ChatInput,
        "ctrl-x".parse().expect("key"),
        KeyAction::App(AppAction::SubmitInput),
        KeyBindingSource::Config,
    )])
    .expect("remapped keymap");
    let after = table(&document(&mut app, "/docs shortcuts"));
    assert_eq!(after.get("Ctrl+X").map(String::as_str), Some("Send message"));
    assert!(
        !after
            .iter()
            .any(|(key, action)| key.split(", ").any(|key| key == "Enter")
                && action == "Send message")
    );
}
