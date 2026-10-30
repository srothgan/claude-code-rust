// SPDX-License-Identifier: Apache-2.0

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let executable = std::env::current_exe()?;
    let root = executable.parent().ok_or("fixture directory")?;
    let arguments = std::env::args().skip(1).collect::<Vec<_>>().join("\n") + "\n";
    std::fs::write(root.join("argv"), arguments)?;
    match std::fs::read_to_string(root.join("mode"))?.as_str() {
        "valid" => println!("  v4.5.6  "),
        "nonzero" => {
            println!("4.5.6");
            eprintln!("probe refused");
            return Err("fixture refusal".into());
        }
        "malformed" => println!("fixture-invalid-version"),
        "empty" => {}
        _ => return Err("unknown fixture mode".into()),
    }
    Ok(())
}
