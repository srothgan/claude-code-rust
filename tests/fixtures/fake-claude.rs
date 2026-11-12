// SPDX-License-Identifier: Apache-2.0
// A native fake CLI for isolated auth, plugin inventory, and MCP removal tests.

use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

fn main() -> io::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if std::env::var("FAKE_AUTH_MODE").as_deref() == Ok("persistent-plugins") {
        let runtime = std::env::var_os("FAKE_PLUGIN_RUNTIME").expect("fixture runtime");
        let status = std::process::Command::new(runtime)
            .arg(std::env::var_os("FAKE_PLUGIN_SCRIPT").expect("fixture script"))
            .args(&args)
            .status()?;
        std::process::exit(status.code().unwrap_or(1));
    }
    if std::env::var("FAKE_AUTH_MODE").as_deref() == Ok("plugin-inventory") {
        assert!(
            [
                vec!["plugin", "list", "--json"],
                vec!["plugin", "list", "--available", "--json"],
                vec!["plugin", "marketplace", "list", "--json"],
            ]
            .iter()
            .any(|allowed| args.iter().map(String::as_str).eq(allowed.iter().copied())),
            "the local-folder workflow may only inspect persistent CLI inventory: {args:?}"
        );
        println!(
            "{}",
            if args.iter().any(|arg| arg == "--available") { r#"{"available":[]}"# } else { "[]" }
        );
        return Ok(());
    }
    if std::env::var("FAKE_AUTH_MODE").is_ok_and(|mode| mode.starts_with("mcp-")) {
        let profile =
            PathBuf::from(std::env::var_os("CLAUDE_CONFIG_DIR").expect("isolated profile"));
        let cwd = std::env::current_dir()?;
        let mut journal = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(profile.join("mcp-cli-calls"))?;
        writeln!(journal, "{}\n{}", args.join("\t"), cwd.display())?;
        assert_eq!(&args[..3], &["mcp", "remove", "--scope"]);
        assert_eq!(args.len(), 5);
        assert_eq!(args[4], "target");
        let file = match args[3].as_str() {
            "user" => profile.join("user-mcp.json"),
            "project" => cwd.join(".mcp.json"),
            "local" => cwd.join("local-mcp.json"),
            scope => panic!("unexpected scope {scope}"),
        };
        if profile.join("mcp-fail").exists() {
            eprintln!("FIXTURE_MCP_WRITE_DENIED");
            std::process::exit(7);
        }
        // Deliberately independent, minimal fixture mutation: the CLI owns these
        // documents; the app may only pass the exact scope/name and working directory.
        let before = std::fs::read_to_string(&file)?;
        let entry = r#""target":{"command":"fixture-target"},"#;
        assert!(before.contains(entry), "target must exist in chosen scope");
        std::fs::write(file, before.replacen(entry, "", 1))?;
        return Ok(());
    }
    let profile = PathBuf::from(std::env::var_os("CLAUDE_CONFIG_DIR").expect("isolated profile"));
    assert!(args == ["auth", "login"] || args == ["auth", "logout"]);
    let mut calls = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(profile.join("auth-calls"))?;
    writeln!(calls, "{}", args.join("\t"))?;
    std::fs::write(profile.join("auth-pid"), std::process::id().to_string())?;
    if args[1] == "logout" {
        std::fs::remove_file(profile.join(".credentials.json"))?;
        return Ok(());
    }
    let _ = std::fs::remove_file(profile.join("auth-release"));
    println!("AUTH_CHILD_READY");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    std::fs::write(profile.join("auth-input"), input)?;
    println!("AUTH_INPUT_RECORDED");
    io::stdout().flush()?;
    while !profile.join("auth-release").exists() {
        std::thread::sleep(Duration::from_millis(20));
    }
    if std::env::var("FAKE_AUTH_MODE").as_deref() == Ok("failure") {
        std::process::exit(7);
    }
    std::fs::write(
        profile.join(".credentials.json"),
        r#"{"claudeAiOauth":{"accessToken":"fixture-only-not-a-token"}}"#,
    )?;
    Ok(())
}
