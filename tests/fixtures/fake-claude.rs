// SPDX-License-Identifier: Apache-2.0
// A native fake `claude auth login` executable. Never uses real credentials.

use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

fn main() -> io::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    assert_eq!(args, ["auth", "login"]);
    let profile = PathBuf::from(std::env::var_os("CLAUDE_CONFIG_DIR").expect("isolated profile"));
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
