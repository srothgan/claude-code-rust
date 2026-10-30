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
    use crate::Cli;

    #[test]
    fn interactive_launches_defer_silently_to_the_application() {
        use clap::Parser;
        let cases: &[&[&str]] = &[
            &["claude-rs"],
            &["claude-rs", "resume"],
            &["claude-rs", "resume", "session-1", "initial prompt"],
            &["claude-rs", "--resume"],
            &["claude-rs", "--continue"],
        ];
        for args in cases {
            let cli = Cli::try_parse_from(args.iter().copied()).unwrap();
            cli.validate().unwrap();
            let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
            assert_eq!(run_support_command(&cli, &mut stdout, &mut stderr).unwrap(), None);
            assert!(stdout.is_empty() && stderr.is_empty(), "{args:?}");
        }
    }
}
