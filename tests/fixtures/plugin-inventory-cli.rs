// SPDX-License-Identifier: Apache-2.0
// Standalone, std-only process fixture. All inputs and mutations live in its cwd.
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut journal = OpenOptions::new().create(true).append(true).open("journal").unwrap();
    writeln!(journal, "{}\t{}", std::env::current_dir().unwrap().display(), args.join("\t"))
        .unwrap();
    let mode = fs::read_to_string("failure").unwrap_or_default();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let file = match args.as_slice() {
        ["plugin", "list", "--json"] => "installed.json",
        ["plugin", "list", "--available", "--json"] => {
            if mode == "available" {
                eprintln!("fixture marketplace unavailable");
                return ExitCode::from(23);
            }
            if mode == "malformed" {
                println!("{{broken inventory");
                return ExitCode::SUCCESS;
            }
            "available.json"
        }
        ["plugin", "marketplace", "list", "--json"] => "sources.json",
        ["plugin", "disable", "alpha@fixture", "--scope", "user"] => {
            if mode == "mutation" {
                eprintln!("fixture registration locked");
                return ExitCode::from(24);
            }
            fs::copy("installed-after.json", "installed.json").unwrap();
            return ExitCode::SUCCESS;
        }
        ["plugin", "marketplace", "add", "team/new-market", "--scope", "user"] => {
            fs::copy("sources-after.json", "sources.json").unwrap();
            return ExitCode::SUCCESS;
        }
        _ => {
            eprintln!("unexpected fixture command: {args:?}");
            return ExitCode::from(25);
        }
    };
    print!("{}", fs::read_to_string(file).unwrap());
    ExitCode::SUCCESS
}
