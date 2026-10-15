// SPDX-License-Identifier: Apache-2.0

//! Read-only Git comparisons. Git owns tracked-file patches and statistics.

use anyhow::{Context, bail};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Comparison {
    #[default]
    Head,
    Staged,
    Unstaged,
}

impl Comparison {
    pub(crate) const ALL: [Self; 3] = [Self::Head, Self::Staged, Self::Unstaged];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Head => "HEAD",
            Self::Staged => "Staged",
            Self::Unstaged => "Unstaged",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Head => "HEAD -> working tree + untracked files",
            Self::Staged => "HEAD -> index",
            Self::Unstaged => "Index -> working tree + untracked files",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FileChange {
    pub path: String,
    pub previous_path: Option<String>,
    pub status: char,
    pub counts: Option<(usize, usize)>,
    old_mode: String,
    new_mode: String,
    old_oid: String,
    new_oid: String,
}

#[derive(Debug)]
pub(crate) struct Snapshot {
    pub root: PathBuf,
    pub comparison: Comparison,
    pub files: Vec<FileChange>,
    pub(super) head: String,
}

#[derive(Debug)]
pub(crate) struct FileContent {
    pub patch: String,
    pub full_text: Option<String>,
    pub notice: Option<String>,
    pub deleted: bool,
}

fn command(cwd: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(cwd)
        .args([
            "--no-pager",
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "diff.suppressBlankEmpty=false",
        ])
        .env("GIT_LITERAL_PATHSPECS", "1")
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command
}

fn checked(output: Output) -> anyhow::Result<Vec<u8>> {
    if !output.status.success() {
        bail!("Git read failed: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(output.stdout)
}

fn run(cwd: &Path, args: &[&str]) -> anyhow::Result<Vec<u8>> {
    checked(command(cwd).args(args).output().context("Cannot run Git")?)
}

fn text(bytes: Vec<u8>) -> anyhow::Result<String> {
    String::from_utf8(bytes).context("Git returned a path or patch that is not UTF-8")
}

fn diff_command(root: &Path, comparison: Comparison, head: &str) -> Command {
    let mut command = command(root);
    command.args([
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--find-renames",
        "--output-indicator-new=+",
        "--output-indicator-old=-",
        "--output-indicator-context= ",
    ]);
    match comparison {
        Comparison::Head => {
            command.arg(head);
        }
        Comparison::Staged => {
            command.arg("--cached").arg(head);
        }
        Comparison::Unstaged => {}
    }
    command
}

pub(crate) fn snapshot(cwd: &Path, comparison: Comparison) -> anyhow::Result<Snapshot> {
    let root = text(run(cwd, &["rev-parse", "--show-toplevel"])?)?;
    let root = PathBuf::from(root.strip_suffix('\n').unwrap_or(&root));
    let head = command(&root).args(["rev-parse", "--verify", "--quiet", "HEAD"]).output()?;
    let head = if head.status.success() {
        text(head.stdout)?.trim().to_owned()
    } else if head.status.code() == Some(1) {
        // hash-object without -w calculates the empty-tree ID without writing an object.
        text(run(&root, &["hash-object", "-t", "tree", "--stdin"])?)?.trim().to_owned()
    } else {
        checked(head)?;
        bail!("Cannot resolve HEAD");
    };
    let output = diff_command(&root, comparison, &head)
        .args(["--raw", "--numstat", "-z", "--no-abbrev", "--"])
        .output()?;
    let mut files = parse_changes(&checked(output)?)?;
    if comparison != Comparison::Staged {
        for path in run(&root, &["ls-files", "--others", "--exclude-standard", "-z"])?
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
        {
            let path = std::str::from_utf8(path).context("Untracked path is not UTF-8")?.to_owned();
            let counts = working_file(&root.join(&path))
                .ok()
                .and_then(|bytes| decode_file(bytes).ok())
                .map(|content| (content.lines().count(), 0));
            files.push(FileChange {
                path,
                previous_path: None,
                status: '?',
                counts,
                old_mode: "000000".into(),
                new_mode: "100644".into(),
                old_oid: String::new(),
                new_oid: String::new(),
            });
        }
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(Snapshot { root, comparison, files, head })
}

fn parse_changes(bytes: &[u8]) -> anyhow::Result<Vec<FileChange>> {
    let mut tokens = bytes.split(|byte| *byte == 0).filter(|token| !token.is_empty());
    let mut files = BTreeMap::new();
    while let Some(token) = tokens.next() {
        let token = std::str::from_utf8(token).context("Git change metadata is not UTF-8")?;
        if let Some(header) = token.strip_prefix(':') {
            let header: Vec<_> = header.split_whitespace().collect();
            let [old_mode, new_mode, old_oid, new_oid, status] = header.as_slice() else {
                bail!("Invalid Git change metadata");
            };
            let mut status = status.chars().next().context("Missing Git file status")?;
            let first = next_path(&mut tokens)?;
            let (path, previous_path) = if matches!(status, 'R' | 'C') {
                (next_path(&mut tokens)?, Some(first))
            } else {
                (first, None)
            };
            if files.get(&path).is_some_and(|file: &FileChange| file.status == 'U') {
                status = 'U';
            }
            files.insert(
                path.clone(),
                FileChange {
                    path,
                    previous_path,
                    status,
                    counts: None,
                    old_mode: (*old_mode).into(),
                    new_mode: (*new_mode).into(),
                    old_oid: (*old_oid).into(),
                    new_oid: (*new_oid).into(),
                },
            );
        } else {
            let parts: Vec<_> = token.splitn(3, '\t').collect();
            let [added, removed, path] = parts.as_slice() else {
                bail!("Invalid Git line statistics");
            };
            let path = if path.is_empty() {
                let _old_path = next_path(&mut tokens)?;
                next_path(&mut tokens)?
            } else {
                (*path).to_owned()
            };
            let file = files.get_mut(&path).context("Git statistics have no matching file")?;
            if *added != "-" && *removed != "-" {
                file.counts = Some((added.parse()?, removed.parse()?));
            }
        }
    }
    Ok(files.into_values().collect())
}

fn next_path<'a>(tokens: &mut impl Iterator<Item = &'a [u8]>) -> anyhow::Result<String> {
    Ok(std::str::from_utf8(tokens.next().context("Missing Git path")?)
        .context("Git path is not UTF-8")?
        .to_owned())
}

fn working_file(path: &Path) -> anyhow::Result<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("Cannot read {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Ok(std::fs::read_link(path)?.into_os_string().into_encoded_bytes());
    }
    if !metadata.is_file() {
        bail!("{} is not a regular file", path.display());
    }
    std::fs::read(path).with_context(|| format!("Cannot read {}", path.display()))
}

fn decode_file(bytes: Vec<u8>) -> anyhow::Result<String> {
    if bytes.contains(&0) {
        bail!("Binary file; no text preview available.");
    }
    String::from_utf8(bytes).context("File is not UTF-8; no text preview available.")
}

pub(crate) fn content(
    root: &Path,
    comparison: Comparison,
    head: &str,
    file: &FileChange,
) -> anyhow::Result<FileContent> {
    let deleted = file.new_mode == "000000" && file.status != 'U';
    let bytes = if file.old_mode == "160000" || file.new_mode == "160000" {
        None
    } else if deleted {
        Some(run(root, &["cat-file", "blob", &file.old_oid])?)
    } else if comparison == Comparison::Staged && file.status != 'U' {
        Some(run(root, &["cat-file", "blob", &file.new_oid])?)
    } else {
        Some(working_file(&root.join(&file.path))?)
    };
    let (full_text, mut notice) = match bytes.map(decode_file) {
        Some(Ok(text)) => (Some(text), None),
        Some(Err(error)) => (None, Some(error.to_string())),
        None => (None, Some("Submodule: the diff shows its commit change.".into())),
    };
    if file.status == 'U' && full_text.is_some() {
        notice = Some("Unmerged file: showing working-tree conflict markers.".into());
    }
    let patch = if file.status == '?' {
        full_text.as_ref().map_or_else(String::new, |new| {
            similar::TextDiff::from_lines("", new)
                .unified_diff()
                .header("/dev/null", &file.path)
                .to_string()
        })
    } else {
        let mut command = diff_command(root, comparison, head);
        command.arg("--patch").arg("--");
        if let Some(previous) = &file.previous_path {
            command.arg(previous);
        }
        command.arg(&file.path);
        String::from_utf8_lossy(&checked(command.output()?)?).into_owned()
    };
    Ok(FileContent { patch, full_text, notice, deleted })
}
