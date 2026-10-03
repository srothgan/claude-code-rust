import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import test from "node:test";

const installerSource = fs.readFileSync(new URL("./install.sh", import.meta.url), "utf8").replace(/\r\n/gu, "\n");
const helpers = [
  "canonical_path", "path_entries", "commands_on_path", "path_has_bin_dir", "path_starts_with_bin_dir",
  "launcher_contents", "write_launcher", "is_script_install_dir", "warn_other_claude_rs_commands",
  "script_install_directory_for_command", "install_directory_overlaps", "uninstall_script_install",
  "acquire_lock", "man_source_dir", "man_link_dir", "remove_man_pages_if_owned", "resolve_npm_install_choice",
  "remove_managed_path_block", "remove_managed_path_blocks", "zsh_profile_file", "zsh_rc_file",
  "managed_path_lines", "manual_path_line",
  "remove_launcher_if_owned",
].map(installerFunction).join("\n");

function installerFunction(name) {
  const start = installerSource.indexOf(`\n${name}() `) + 1;
  assert.ok(start > 0, `Missing installer helper ${name}`);
  const subshell = ["canonical_path", "script_install_directory_for_command", "uninstall_script_install"].includes(name);
  const end = installerSource.indexOf(subshell ? "\n)\n" : "\n}\n", start) + 3;
  return installerSource.slice(start, end);
}

test("Unix PATH discovery distinguishes this installation, other owners, and aliases", { skip: process.platform === "win32" }, () => {
  const harness = String.raw`
set -eu
${helpers}
warn() { printf '%s\n' "$@"; }
ok() { printf '%s\n' "$@"; }
die() { printf '%s\n' "$@" >&2; exit 1; }
yes=1
update=0
sandbox=$(mktemp -d)
trap 'rm -rf "$sandbox"' EXIT
system_path=$PATH
HOME="$sandbox/home"
mkdir "$HOME"
install_dir="$sandbox/current app"
bin_dir="$sandbox/bin"
binary_name=claude-rs
other_dir="$sandbox/older copy's app"
unknown_dir="$sandbox/unknown"
other_bin="$sandbox/other bin"
second_other_bin="$sandbox/second other bin"
mkdir -p "$install_dir" "$bin_dir" "$other_dir" "$unknown_dir" "$sandbox/link"
for directory in "$install_dir" "$other_dir" "$unknown_dir"; do
  printf '#!/bin/sh\nexit 0\n' > "$directory/claude-rs"
  chmod +x "$directory/claude-rs"
done
printf '{"name":"claude-code-rust","version":"1.2.3"}\n' > "$other_dir/package.json"
touch "$other_dir/claude-rs-bridge-bun"
printf '{"name":"claude-code-rust","version":"1.2.3"}\n' > "$install_dir/package.json"
touch "$install_dir/claude-rs-bridge-bun"
write_launcher
current_install_dir="$install_dir"
current_bin_dir="$bin_dir"
install_dir="$other_dir"
bin_dir="$other_bin"
write_launcher
bin_dir="$second_other_bin"
write_launcher
install_dir="$current_install_dir"
bin_dir="$current_bin_dir"
# Both managed profile blocks coexist; cleanup must preserve the current one.
{
  printf '# claude-rs PATH start\n'
  managed_path_lines
  printf '# claude-rs PATH end\n'
  bin_dir="$other_bin"
  printf '# claude-rs PATH start\n'
  managed_path_lines
  printf '# claude-rs PATH end\n'
} > "$HOME/.profile"
bin_dir="$current_bin_dir"
ln -s bin "$sandbox/alias"
ln -s ../bin/claude-rs "$sandbox/link/claude-rs"
PATH="$bin_dir:$sandbox/alias:$sandbox/link:$install_dir:$bin_dir/.:$system_path"
[ -z "$(warn_other_claude_rs_commands)" ]
printf '#!/bin/sh\nexit 0\n' > "$install_dir/claude-rs"
[ -z "$(warn_other_claude_rs_commands)" ]
path_has_bin_dir
path_starts_with_bin_dir
PATH="$sandbox/alias:$system_path"
path_has_bin_dir
path_starts_with_bin_dir
PATH="$sandbox/missing/child:$sandbox/alias:$system_path"
path_has_bin_dir
if path_starts_with_bin_dir; then exit 1; fi
PATH="$unknown_dir:$system_path"
if path_has_bin_dir; then exit 1; fi
cd "$bin_dir"
PATH=":$system_path"
path_has_bin_dir
path_starts_with_bin_dir
PATH="$system_path:"
path_has_bin_dir
PATH="$bin_dir:$other_dir:$other_bin:$second_other_bin:$other_dir/.:$unknown_dir:$system_path"
printf 'COPIES_BEGIN\n'
warn_other_claude_rs_commands
printf 'COPIES_END\n'
alias claude-rs='echo shadowed'
printf 'ALIAS_BEGIN\n'
warn_other_claude_rs_commands
printf 'ALIAS_END\n'
unalias claude-rs

# The real cleanup workflow removes only an explicitly confirmed installation.
answer=0
non_interactive=0
confirm_default_no() {
  [ "$non_interactive" -eq 0 ] || return 1
  printf 'PROMPT: %s\n' "$1"
  [ "$answer" -eq 1 ]
}
yes=0
printf 'DECLINED_BEGIN\n'
warn_other_claude_rs_commands
printf 'DECLINED_END\n'
[ -f "$other_dir/claude-rs" ] && [ -f "$other_bin/claude-rs" ]
answer=1
for mode in yes update non_interactive; do
  yes=0
  update=0
  non_interactive=0
  case "$mode" in
    yes) yes=1 ;;
    update) update=1 ;;
    non_interactive) non_interactive=1 ;;
  esac
  output=$(warn_other_claude_rs_commands)
  if printf '%s' "$output" | grep -q PROMPT; then exit 1; fi
  [ -f "$other_dir/claude-rs" ]
done
yes=0
update=0
non_interactive=0
mkdir "$sandbox/.claude-rs-install.lock"
output=$(warn_other_claude_rs_commands 2> "$sandbox/lock-error")
printf '%s' "$output" | grep -q 'Could not uninstall'
[ -d "$sandbox/.claude-rs-install.lock" ] && [ -f "$other_dir/claude-rs" ]
rmdir "$sandbox/.claude-rs-install.lock"

mkdir -p "$install_dir/share/man/man1" "$other_dir/share/man/man1" "$sandbox/share/man/man1"
touch "$install_dir/share/man/man1/claude-rs.1" "$other_dir/share/man/man1/claude-rs-old.1"
ln -s "$install_dir/share/man/man1/claude-rs.1" "$sandbox/share/man/man1/claude-rs.1"
ln -s "$other_dir/share/man/man1/claude-rs-old.1" "$sandbox/share/man/man1/claude-rs-old.1"
printf 'ACCEPTED_BEGIN\n'
warn_other_claude_rs_commands
printf 'ACCEPTED_END\n'
[ ! -e "$other_dir" ] && [ ! -e "$other_bin/claude-rs" ] && [ ! -e "$second_other_bin/claude-rs" ]
[ -f "$install_dir/claude-rs" ] && [ -f "$bin_dir/claude-rs" ] && [ -f "$unknown_dir/claude-rs" ]
[ -L "$sandbox/share/man/man1/claude-rs.1" ] && [ ! -L "$sandbox/share/man/man1/claude-rs-old.1" ]
grep -F -e "$current_bin_dir" "$HOME/.profile" >/dev/null
if grep -F -e "$other_bin" "$HOME/.profile" >/dev/null; then exit 1; fi
[ ! -d "$sandbox/.claude-rs-install.lock" ]
output=$(uninstall_script_install "$unknown_dir")
[ -f "$unknown_dir/claude-rs" ]
install_directory_overlaps / "$install_dir"
install_directory_overlaps "$install_dir/child" "$install_dir"

# npm remains a separate, identified owner with its own opt-in removal flow.
root_package=claude-code-rust
npm_root="$sandbox/npm"
keep_npm=0
remove_npm=0
detect_npm_install() { npm_package_version=1.2.3; return 0; }
remove_npm_install() { printf 'NPM_REMOVED\n'; }
answer=0
output=$(resolve_npm_install_choice)
if printf '%s' "$output" | grep -q NPM_REMOVED; then exit 1; fi
answer=1
for mode in yes keep_npm non_interactive; do
  yes=0
  keep_npm=0
  non_interactive=0
  case "$mode" in
    yes) yes=1 ;;
    keep_npm) keep_npm=1 ;;
    non_interactive) non_interactive=1 ;;
  esac
  output=$(resolve_npm_install_choice)
  if printf '%s' "$output" | grep -q NPM_REMOVED; then exit 1; fi
done
yes=0
keep_npm=0
non_interactive=0
output=$(resolve_npm_install_choice)
printf '%s' "$output" | grep -q NPM_REMOVED
remove_npm=1
non_interactive=1
output=$(resolve_npm_install_choice)
printf '%s' "$output" | grep -q NPM_REMOVED

# Explicit uninstall still cleans its own launcher, manual, and profile block.
uninstall_script_install
[ ! -e "$install_dir" ] && [ ! -e "$bin_dir/claude-rs" ]
[ ! -L "$sandbox/share/man/man1/claude-rs.1" ]
if grep -q '# claude-rs PATH start' "$HOME/.profile"; then exit 1; fi
[ -f "$unknown_dir/claude-rs" ]
`;
  const result = spawnSync("sh", ["-c", harness], { encoding: "utf8", timeout: 20000 });
  assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`);
  assert.equal(result.stderr, "", "Unavailable PATH entries produced diagnostics");
  const copies = result.stdout.split("COPIES_BEGIN\n")[1].split("COPIES_END\n")[0];
  assert.equal((copies.match(/Another .*on PATH/g) ?? []).length, 2, copies);
  assert.match(copies, /Another script installation/);
  assert.doesNotMatch(copies, /use this installer with/);
  assert.match(copies, /tool that installed it/);
  assert.doesNotMatch(copies, /npm uninstall/);
  const alias = result.stdout.split("ALIAS_BEGIN\n")[1].split("ALIAS_END\n")[0];
  assert.match(alias, /shell alias or function/);
  const declined = result.stdout.split("DECLINED_BEGIN\n")[1].split("DECLINED_END\n")[0];
  assert.equal((declined.match(/PROMPT:/g) ?? []).length, 1, declined);
  assert.match(declined, /Uninstall this other script installation at .*older copy's app\?/);
  const accepted = result.stdout.split("ACCEPTED_BEGIN\n")[1].split("ACCEPTED_END\n")[0];
  assert.equal((accepted.match(/PROMPT:/g) ?? []).length, 1, accepted);
  assert.match(accepted, /Removed script install directory/);
  assert.equal((accepted.match(/Another .*on PATH/g) ?? []).length, 2, "Cleanup warned about a copy it had already removed");
});

test("Unix cleanup prompt restores terminal state when cancelled", { skip: process.platform !== "linux" }, async () => {
  const prefix = installerSource.slice(0, installerSource.indexOf("\nneed_cmd() {"));
  const promptHelpers = [
    "canonical_path", "path_entries", "commands_on_path", "launcher_contents",
    "is_script_install_dir", "script_install_directory_for_command", "install_directory_overlaps",
    "warn_other_claude_rs_commands", "stop_download_process", "cleanup", "on_signal",
  ].map(installerFunction).join("\n");
  const harness = `${prefix}\n${promptHelpers}\n` + String.raw`
non_interactive=0
progress_enabled=1
yes=0
update=0
frame_open=1
flow_label=Installation
tmpdir=$(mktemp -d)
install_dir="$tmpdir/current"
bin_dir="$tmpdir/bin"
other_dir="$tmpdir/other"
mkdir -p "$install_dir" "$bin_dir" "$other_dir"
for app_dir in "$install_dir" "$other_dir"; do
  printf '#!/bin/sh\nexit 0\n' > "$app_dir/claude-rs"
  chmod +x "$app_dir/claude-rs"
  touch "$app_dir/claude-rs-bridge-bun"
  printf '{"name":"claude-code-rust"}\n' > "$app_dir/package.json"
done
PATH="$other_dir:$PATH"
before=$(stty -g < /dev/tty)
trap 'status=$?; cleanup "$status"; after=$(stty -g < /dev/tty); [ "$before" != "$after" ] || echo TTY_RESTORED; exit "$status"' EXIT
trap 'on_signal 130' INT
warn_other_claude_rs_commands
echo UNEXPECTED_COMPLETION
`;
  const encoded = Buffer.from(harness).toString("base64");
  const env = { ...process.env, TERM: "xterm", LC_ALL: "C", NO_COLOR: "1" };
  delete env.CI;
  // GNU timeout must preserve the foreground process group for Ctrl-C delivery.
  const child = spawn("script", ["-q", "-e", "-c", `printf '%s' '${encoded}' | base64 -d | timeout --foreground 5 sh`, "/dev/null"], { env });
  let output = "";
  let cancelled = false;
  const status = await new Promise((resolve, reject) => {
    child.on("error", reject);
    child.stdin.on("error", reject);
    child.stderr.on("data", (chunk) => { output += chunk; });
    child.stdout.on("data", (chunk) => {
      output += chunk;
      if (!cancelled && output.includes("Uninstall this other script installation at")) {
        cancelled = true;
        child.stdin.write("\u0003");
      }
    });
    child.on("close", resolve);
  });
  assert.equal(status, 130, output);
  assert.match(output, /TTY_RESTORED/);
  assert.match(output, /Installation cancelled/);
  assert.doesNotMatch(output, /UNEXPECTED_COMPLETION/);
});
