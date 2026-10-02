// SPDX-License-Identifier: Apache-2.0

use crate::Cli;
use clap::CommandFactory;
use std::io::Write;
use std::path::Path;

pub(super) fn completions(
    shell: clap_complete::Shell,
    stdout: &mut impl Write,
) -> anyhow::Result<i32> {
    // Generate to memory because clap_complete's writer interface cannot return I/O errors.
    let mut script = Vec::new();
    clap_complete::generate(shell, &mut Cli::command(), "claude-rs", &mut script);
    stdout.write_all(&script)?;
    Ok(0)
}

pub(super) fn man(out_dir: &Path) -> anyhow::Result<i32> {
    std::fs::create_dir_all(out_dir)?;
    clap_mangen::generate_to(Cli::command(), out_dir)?;
    Ok(0)
}
