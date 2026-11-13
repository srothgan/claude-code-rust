// SPDX-License-Identifier: Apache-2.0

use crate::{Cli, StartupLaunch};
use clap::{Parser, error::ErrorKind};

#[test]
fn prompt_policy_rejects_same_role_pairs_and_accepts_independent_roles() {
    for (text, file) in [
        ("--system-prompt", "--system-prompt-file"),
        ("--append-system-prompt", "--append-system-prompt-file"),
    ] {
        for args in [[text, "text", file, "absent.md"], [file, "absent.md", text, "text"]] {
            let error = Cli::try_parse_from(["claude-rs"].into_iter().chain(args)).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::ArgumentConflict);
            assert!(error.to_string().contains(text) && error.to_string().contains(file));
        }
    }
    for replacement in ["--system-prompt", "--system-prompt-file"] {
        for append in ["--append-system-prompt", "--append-system-prompt-file"] {
            let cli =
                Cli::try_parse_from(["claude-rs", replacement, "replacement", append, "append"])
                    .unwrap();
            cli.validate().unwrap();
        }
    }
}

#[test]
fn fork_eligibility_preserves_resume_intent_and_initial_prompt() {
    let cases: &[(&[&str], StartupLaunch, Option<&str>)] = &[
        (&["--resume"], StartupLaunch::SessionPicker, None),
        (
            &["-r", "session-1", "next prompt"],
            StartupLaunch::ResumeSession { session_id: "session-1".into() },
            Some("next prompt"),
        ),
        (&["--continue", "next prompt"], StartupLaunch::ContinueSession, Some("next prompt")),
        (&["resume"], StartupLaunch::SessionPicker, None),
        (
            &["resume", "session-2", "subcommand prompt"],
            StartupLaunch::ResumeSession { session_id: "session-2".into() },
            Some("subcommand prompt"),
        ),
        (&["--resume", "--", "picker prompt"], StartupLaunch::SessionPicker, Some("picker prompt")),
    ];
    for (args, expected, prompt) in cases {
        let cli = Cli::try_parse_from(
            ["claude-rs", "--fork-session"].into_iter().chain(args.iter().copied()),
        )
        .unwrap();
        cli.validate().unwrap();
        assert_eq!(&cli.startup_launch(), expected);
        assert_eq!(cli.initial_prompt(), *prompt);
        assert!(cli.session_options.fork_session);
    }
    for args in [&[][..], &["new prompt"][..], &["--name", "Experiment"][..]] {
        let cli = Cli::try_parse_from(
            ["claude-rs", "--fork-session"].into_iter().chain(args.iter().copied()),
        )
        .unwrap();
        let error = cli.validate().unwrap_err();
        assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
        assert!(error.to_string().contains("--fork-session"));
    }
}

#[test]
fn each_launch_option_is_rejected_by_support_commands_before_file_resolution() {
    let options: &[&[&str]] = &[
        &["--model", "opus"],
        &["--effort", "max"],
        &["--permission-mode", "plan"],
        &["--agent", "reviewer"],
        &["--name", "Experiment"],
        &["--fork-session"],
        &["--add-dir", "absent-dir"],
        &["--settings", "absent-settings.json"],
        &["--mcp-config", "absent-mcp.json"],
        &["--strict-mcp-config"],
        &["--system-prompt", "replace"],
        &["--system-prompt-file", "absent-replace.md"],
        &["--append-system-prompt", "append"],
        &["--append-system-prompt-file", "absent-append.md"],
    ];
    let root = tempfile::tempdir().unwrap();
    for option in options {
        let cli = Cli::try_parse_from(
            ["claude-rs", "resume", "session-1"].into_iter().chain(option.iter().copied()),
        )
        .unwrap();
        cli.validate().unwrap();
        assert_eq!(
            cli.startup_launch(),
            StartupLaunch::ResumeSession { session_id: "session-1".into() }
        );
        for command in ["doctor", "logs", "config"] {
            let mut cli = Cli::try_parse_from(
                ["claude-rs", command].into_iter().chain(option.iter().copied()),
            )
            .unwrap();
            cli.dir = Some(root.path().join("nonexistent-working-directory"));
            let error = cli.validate().unwrap_err();
            assert_eq!(error.kind(), ErrorKind::ArgumentConflict, "{command} {option:?}: {error}");
        }
    }
    for command in ["doctor", "logs", "config"] {
        let cli = Cli::try_parse_from(["claude-rs", command]).unwrap();
        cli.validate().unwrap();
    }
    assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
}
