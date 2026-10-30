// SPDX-License-Identifier: Apache-2.0

mod config;
mod config_files;
mod doctor;
mod logs;
pub mod redaction;
mod reference;
mod style;

use crate::{Cli, Command};
use std::io::Write;

pub fn run_support_command(
    cli: &Cli,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> anyhow::Result<Option<i32>> {
    match &cli.command {
        Some(Command::Completions { shell }) => reference::completions(*shell, stdout).map(Some),
        Some(Command::Man { out_dir }) => reference::man(out_dir).map(Some),
        Some(Command::Doctor(args)) => doctor::run(cli, args, stdout).map(Some),
        Some(Command::Logs(args)) => logs::run(cli, args, stdout, stderr).map(Some),
        Some(Command::Config(args)) => config::run(cli, args, stdout, stderr).map(Some),
        Some(Command::UpdateWorker(args)) => Ok(Some(crate::app::run_update_worker(args))),
        Some(Command::Resume { .. }) | None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::run_support_command;
    use crate::{Cli, Command, ConfigArgs, ConfigCommand, ConfigPathArgs};

    #[test]
    fn no_subcommand_uses_interactive_path() {
        let cli = test_cli(None);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let result = run_support_command(&cli, &mut stdout, &mut stderr).expect("dispatch");

        assert_eq!(result, None);
        assert!(stdout.is_empty());
        assert!(stderr.is_empty());
    }

    #[test]
    fn resume_uses_interactive_path() {
        let cli = test_cli(Some(Command::Resume {
            session_id: Some("abc-123".to_owned()),
            prompt: None,
        }));
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let result = run_support_command(&cli, &mut stdout, &mut stderr).expect("dispatch");

        assert_eq!(result, None);
        assert!(stdout.is_empty());
        assert!(stderr.is_empty());
    }

    #[test]
    fn config_is_a_support_command() {
        let cli = test_cli(Some(Command::Config(ConfigArgs {
            command: Some(ConfigCommand::Path(ConfigPathArgs { json: false, which: None })),
        })));
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let result = run_support_command(&cli, &mut stdout, &mut stderr).expect("dispatch");

        assert_eq!(result, Some(0));
        assert!(!stdout.is_empty());
        assert!(stderr.is_empty());
    }

    fn test_cli(command: Option<Command>) -> Cli {
        Cli {
            command,
            prompt: None,
            resume: None,
            continue_session: false,
            session_options: crate::SessionOptions::default(),
            session_config: crate::SessionConfigArgs::default(),
            no_update_check: false,
            dir: None,
            bridge_script: None,
            enable_logs: false,
            diagnostics_preset: None,
            log_file: None,
            log_filter: None,
            log_append: false,
        }
    }
}
