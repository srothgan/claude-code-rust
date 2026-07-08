// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

mod config;
mod doctor;
mod logs;
pub mod redaction;

use crate::{Cli, Command};
use std::io::Write;

pub fn run_support_command(
    cli: &Cli,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> anyhow::Result<Option<i32>> {
    match &cli.command {
        Some(Command::Doctor(args)) => doctor::run(cli, args, stdout).map(Some),
        Some(Command::Logs(args)) => logs::run(cli, args, stdout, stderr).map(Some),
        Some(Command::Config(args)) => config::run(cli, args, stdout, stderr).map(Some),
        Some(Command::Resume { .. }) | None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::run_support_command;
    use crate::{Cli, Command, ConfigArgs, ConfigCommand, ConfigPathArgs, DoctorArgs, LogsArgs};

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
        let cli = test_cli(Some(Command::Resume { session_id: Some("abc-123".to_owned()) }));
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let result = run_support_command(&cli, &mut stdout, &mut stderr).expect("dispatch");

        assert_eq!(result, None);
        assert!(stdout.is_empty());
        assert!(stderr.is_empty());
    }

    #[test]
    fn doctor_is_a_support_command() {
        let cli = test_cli(Some(Command::Doctor(DoctorArgs { json: true, strict: false })));
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let result = run_support_command(&cli, &mut stdout, &mut stderr).expect("dispatch");

        assert_eq!(result, Some(0));
        assert!(!stdout.is_empty());
        assert!(stderr.is_empty());
    }

    #[test]
    fn logs_is_a_support_command() {
        let cli = test_cli(Some(Command::Logs(LogsArgs {
            path: true,
            latest: false,
            tail: None,
            bundle: false,
            output: None,
            yes: false,
        })));
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let result = run_support_command(&cli, &mut stdout, &mut stderr).expect("dispatch");

        assert_eq!(result, Some(0));
        assert!(!stdout.is_empty());
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
            no_update_check: false,
            dir: None,
            bridge_script: None,
            enable_logs: false,
            diagnostics_preset: None,
            log_file: None,
            log_filter: None,
            log_append: false,
            enable_perf: false,
            perf_log: None,
            perf_append: false,
        }
    }
}
