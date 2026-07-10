#!/bin/sh
set -eu

repo_owner="srothgan"
repo_name="claude-code-rust"
repo_slug="$repo_owner/$repo_name"
root_package="claude-code-rust"

release="${CLAUDE_RS_RELEASE:-latest}"
install_dir="${CLAUDE_RS_INSTALL_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/claude-rs}"
bin_dir="${CLAUDE_RS_BIN_DIR:-$HOME/.local/bin}"
yes=0
non_interactive=0
no_modify_path=0
verify=0
run_after_install=0
remove_npm=0
keep_npm=0
uninstall=0

case "${CLAUDE_RS_NON_INTERACTIVE:-}" in
  1 | true | TRUE | yes | YES) non_interactive=1 ;;
esac
case "${CLAUDE_RS_NO_MODIFY_PATH:-}" in
  1 | true | TRUE | yes | YES) no_modify_path=1 ;;
esac
case "${CLAUDE_RS_VERIFY:-}" in
  1 | true | TRUE | yes | YES) verify=1 ;;
esac
case "${CLAUDE_RS_RUN:-}" in
  1 | true | TRUE | yes | YES) run_after_install=1 ;;
esac
case "${CLAUDE_RS_REMOVE_NPM:-}" in
  1 | true | TRUE | yes | YES) remove_npm=1 ;;
esac
case "${CLAUDE_RS_KEEP_NPM:-}" in
  1 | true | TRUE | yes | YES) keep_npm=1 ;;
esac
case "${CLAUDE_RS_UNINSTALL:-}" in
  1 | true | TRUE | yes | YES) uninstall=1 ;;
esac
if [ -n "${CI:-}" ]; then
  non_interactive=1
fi

usage() {
  cat <<'EOF'
Usage: install.sh [options]

Options:
  --release <version>       Release tag or version. Defaults to latest.
  --install-dir <dir>       App install directory.
  --bin-dir <dir>           Directory for the claude-rs launcher.
  --yes, -y                 Accept safe installer prompts.
  --non-interactive         Do not prompt.
  --no-modify-path          Do not update shell profile PATH.
  --verify                  Run strict runtime diagnostics after install.
  --run                     Start claude-rs after a successful install.
  --remove-npm              Remove an existing global npm install when found.
  --keep-npm                Keep an existing global npm install without prompting.
  --uninstall               Remove the script install layout and managed PATH block.
  --help                    Show this help.

Environment:
  CLAUDE_RS_RELEASE
  CLAUDE_RS_INSTALL_DIR
  CLAUDE_RS_BIN_DIR
  CLAUDE_RS_NON_INTERACTIVE
  CLAUDE_RS_NO_MODIFY_PATH
  CLAUDE_RS_VERIFY
  CLAUDE_RS_RUN
  CLAUDE_RS_REMOVE_NPM
  CLAUDE_RS_KEEP_NPM
  CLAUDE_RS_UNINSTALL
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --release)
      shift
      [ "$#" -gt 0 ] || { echo "missing value for --release" >&2; exit 1; }
      release="$1"
      ;;
    --install-dir)
      shift
      [ "$#" -gt 0 ] || { echo "missing value for --install-dir" >&2; exit 1; }
      install_dir="$1"
      ;;
    --bin-dir)
      shift
      [ "$#" -gt 0 ] || { echo "missing value for --bin-dir" >&2; exit 1; }
      bin_dir="$1"
      ;;
    --yes | -y)
      yes=1
      ;;
    --non-interactive)
      non_interactive=1
      ;;
    --no-modify-path)
      no_modify_path=1
      ;;
    --verify)
      verify=1
      ;;
    --run)
      run_after_install=1
      ;;
    --remove-npm)
      remove_npm=1
      ;;
    --keep-npm)
      keep_npm=1
      ;;
    --uninstall)
      uninstall=1
      ;;
    --help | -h)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
  shift
done

[ "$remove_npm" -eq 0 ] || [ "$keep_npm" -eq 0 ] || {
  echo "error: --remove-npm and --keep-npm cannot be used together" >&2
  exit 1
}

green=""
yellow=""
red=""
reset=""
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  green="$(printf '\033[32m')"
  yellow="$(printf '\033[33m')"
  red="$(printf '\033[31m')"
  reset="$(printf '\033[0m')"
fi

case "${LC_ALL:-${LC_CTYPE:-${LANG:-}}}" in
  C | POSIX)
    ok_mark="OK"
    warn_mark="WARN"
    fail_mark="ERROR"
    ;;
  *)
    ok_mark="$(printf '\342\234\223')"
    warn_mark="!"
    fail_mark="$(printf '\342\234\227')"
    ;;
esac

info() {
  printf '%s\n' "$*"
}

ok() {
  printf '%s%s%s %s\n' "$green" "$ok_mark" "$reset" "$*"
}

warn() {
  printf '%s%s%s %s\n' "$yellow" "$warn_mark" "$reset" "$*" >&2
}

warn_detail() {
  printf '%s\n' "$*" >&2
}

die() {
  printf '%s%s%s %s\n' "$red" "$fail_mark" "$reset" "$*" >&2
  exit 1
}

can_prompt() {
  [ "$non_interactive" -eq 0 ] && [ -r /dev/tty ] && [ -w /dev/tty ]
}

launch_installed() {
  if [ -r /dev/tty ]; then
    "$install_dir/$binary_name" < /dev/tty
  else
    "$install_dir/$binary_name"
  fi
}

confirm_default_no() {
  prompt="$1"
  can_prompt || return 1
  printf '%s [y/N] ' "$prompt" > /dev/tty
  IFS= read -r answer < /dev/tty || answer=
  case "$answer" in
    y | Y | yes | YES) return 0 ;;
    *) return 1 ;;
  esac
}

die_unavailable() {
  printf '%s\n' "install script is currently not available for this release" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

download() {
  url="$1"
  destination="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$destination"
    return $?
  fi
  if command -v wget >/dev/null 2>&1; then
    wget -q -O "$destination" "$url"
    return $?
  fi
  die "required command not found: curl or wget"
}

sha256_file() {
  file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file" | sed 's/[ 	].*//'
    return
  fi
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file" | sed 's/[ 	].*//'
    return
  fi
  if command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 "$file" | sed 's/^.*= //'
    return
  fi
  die "required command not found: sha256sum, shasum, or openssl"
}

json_tag_name() {
  sed -n 's/.*"tag_name"[ 	]*:[ 	]*"\([^"]*\)".*/\1/p' "$1" | sed -n '1p'
}

resolve_tag() {
  requested="$1"
  case "$requested" in
    latest | "")
      latest_json="$tmpdir/latest.json"
      download "https://api.github.com/repos/$repo_slug/releases/latest" "$latest_json" ||
        die "could not resolve latest GitHub Release"
      tag="$(json_tag_name "$latest_json")"
      [ -n "$tag" ] || die "could not parse latest GitHub Release tag"
      printf '%s\n' "$tag"
      ;;
    v*)
      printf '%s\n' "$requested"
      ;;
    *)
      printf 'v%s\n' "$requested"
      ;;
  esac
}

detect_target() {
  os_name="$(uname -s)"
  machine="$(uname -m)"
  case "$os_name:$machine" in
    Darwin:arm64 | Darwin:aarch64)
      printf '%s\n' "darwin-arm64"
      ;;
    Darwin:x86_64)
      if command -v sysctl >/dev/null 2>&1 && [ "$(sysctl -n hw.optional.arm64 2>/dev/null || printf 0)" = "1" ]; then
        printf '%s\n' "darwin-arm64"
      else
        printf '%s\n' "darwin-x64"
      fi
      ;;
    Linux:x86_64 | Linux:amd64)
      require_glibc
      printf '%s\n' "linux-x64-gnu"
      ;;
    Linux:aarch64 | Linux:arm64)
      require_glibc
      printf '%s\n' "linux-arm64-gnu"
      ;;
    Linux:*)
      die "unsupported Linux architecture: $machine"
      ;;
    *)
      die "unsupported platform: $os_name $machine"
      ;;
  esac
}

require_glibc() {
  if command -v getconf >/dev/null 2>&1 && getconf GNU_LIBC_VERSION >/dev/null 2>&1; then
    return
  fi
  if command -v ldd >/dev/null 2>&1 && ldd --version 2>&1 | sed -n '1p' | grep -qi 'glibc\|gnu libc'; then
    return
  fi
  die "unsupported Linux libc: install script archives currently require glibc. Use npm or build from source on musl systems."
}

archive_name_for_target() {
  target="$1"
  version="${tag#v}"
  case "$target" in
    darwin-arm64 | darwin-x64 | linux-arm64-gnu | linux-x64-gnu)
      printf '%s\n' "$root_package-$version-$target.tar.gz"
      ;;
    *)
      die "unsupported Unix install target: $target"
      ;;
  esac
}

checksum_for_archive() {
  checksum_file="$1"
  archive="$2"
  expected_path="dist-install/$archive"
  while read -r sha file rest; do
    [ -z "${rest:-}" ] || continue
    case "$file" in
      "$expected_path" | "*$expected_path")
        printf '%s\n' "$sha"
        return
        ;;
    esac
  done < "$checksum_file"
}

validate_tar_listing() {
  archive="$1"
  top=""
  tar -tzf "$archive" | while IFS= read -r entry; do
    case "$entry" in
      "" | /* | ../* | */../* | */..)
        echo "unsafe archive path: $entry" >&2
        exit 1
        ;;
    esac
  done

  if tar -tvzf "$archive" | sed -n '/^l/p' | sed -n '1p' | grep . >/dev/null 2>&1; then
    die "archive contains symlinks"
  fi

  top="$(tar -tzf "$archive" | sed 's|/.*||' | sed '/^$/d' | sort -u | sed -n '1p')"
  top_count="$(tar -tzf "$archive" | sed 's|/.*||' | sed '/^$/d' | sort -u | wc -l | sed 's/[ 	]//g')"
  [ "$top_count" = "1" ] || die "archive must contain exactly one top-level directory"
  [ -n "$top" ] || die "archive top-level directory is empty"
}

validate_extracted_app() {
  app="$1"
  for required in \
    "$binary_name" \
    "$runtime_name" \
    "package.json" \
    "THIRD-PARTY-NOTICES.md" \
    "agent-sdk/package.json" \
    "agent-sdk/dist/bridge.js" \
    "agent-sdk/dist/types.js" \
    "node_modules/@anthropic-ai/claude-agent-sdk/package.json"
  do
    [ -f "$app/$required" ] || die "archive is missing required file: $required"
  done
  [ -x "$app/$binary_name" ] || die "installed binary is not executable"
  [ -x "$app/$runtime_name" ] || die "bundled Bun runtime is not executable"
}

acquire_lock() {
  parent="$1"
  lock_dir="$parent/.claude-rs-install.lock"
  if mkdir "$lock_dir" 2>/dev/null; then
    printf '%s\n' "$lock_dir"
    return
  fi
  die "another claude-rs installer appears to be running: $lock_dir"
}

replace_app_dir() {
  source_app="$1"
  final_app="$2"
  backup=""
  if [ -e "$final_app" ]; then
    backup="$final_app.backup.$$"
    mv "$final_app" "$backup" || die "could not move existing install directory to backup"
  fi
  if mv "$source_app" "$final_app"; then
    [ -z "$backup" ] || rm -rf "$backup"
    return
  fi
  if [ -n "$backup" ] && [ -e "$backup" ]; then
    mv "$backup" "$final_app" || true
  fi
  die "could not move new app into install directory"
}

write_launcher() {
  mkdir -p "$bin_dir"
  launcher="$bin_dir/claude-rs"
  tmp_launcher="$launcher.tmp.$$"
  app_binary="$install_dir/$binary_name"
  {
    printf '%s\n' '#!/bin/sh'
    printf "exec '%s' \"\$@\"\n" "$(printf '%s' "$app_binary" | sed "s/'/'\\\\''/g")"
  } > "$tmp_launcher"
  chmod 755 "$tmp_launcher"
  mv "$tmp_launcher" "$launcher"
}

detect_npm_install() {
  command -v npm >/dev/null 2>&1 || return 1
  npm_root="$(npm root -g 2>/dev/null || true)"
  [ -n "$npm_root" ] || return 1
  npm_package_json="$npm_root/$root_package/package.json"
  [ -f "$npm_package_json" ] || return 1
  npm_package_version="$(
    sed -n 's/.*"version"[ 	]*:[ 	]*"\([^"]*\)".*/\1/p' "$npm_package_json" | sed -n '1p'
  )"
  [ -n "$npm_package_version" ] || npm_package_version="unknown"
  return 0
}

remove_npm_install() {
  npm uninstall -g "$root_package" >/dev/null 2>&1 ||
    die "could not remove npm install. Run manually: npm uninstall -g $root_package"
  ok "Removed npm install"
}

resolve_npm_install_choice() {
  if ! detect_npm_install; then
    return
  fi

  warn "Existing npm install found: $root_package $npm_package_version"
  if [ "$remove_npm" -eq 1 ]; then
    remove_npm_install
    return
  fi

  if [ "$keep_npm" -eq 0 ] &&
    confirm_default_no 'Remove the npm install so only this installer owns `claude-rs` on PATH?'
  then
    remove_npm_install
    return
  fi

  warn "Existing npm install kept. Remove later with: npm uninstall -g $root_package"
}

is_script_install_dir() {
  app="$1"
  [ -d "$app" ] || return 1
  [ -f "$app/package.json" ] || return 1
  [ -f "$app/claude-rs" ] || return 1
  [ -f "$app/claude-rs-bridge-bun" ] || return 1
  grep -q '"name"[ 	]*:[ 	]*"claude-code-rust"' "$app/package.json" 2>/dev/null
}

remove_launcher_if_owned() {
  launcher="$bin_dir/claude-rs"
  app_binary="$install_dir/claude-rs"
  [ -f "$launcher" ] || return
  if grep -F "$app_binary" "$launcher" >/dev/null 2>&1; then
    rm -f "$launcher"
    ok "Removed launcher $launcher"
  else
    warn "not removing $launcher because it does not point at $app_binary"
  fi
}

remove_managed_path_block() {
  profile="$HOME/.profile"
  [ -f "$profile" ] || return
  tmp_profile="$profile.tmp.$$"
  awk '
    /^# claude-rs PATH start$/ { skip = 1; next }
    /^# claude-rs PATH end$/ { skip = 0; next }
    skip != 1 { print }
  ' "$profile" > "$tmp_profile" && mv "$tmp_profile" "$profile"
}

uninstall_script_install() {
  install_parent="$(dirname "$install_dir")"
  mkdir -p "$install_parent"
  lock_dir="$(acquire_lock "$install_parent")"

  remove_launcher_if_owned
  remove_managed_path_block

  if [ -e "$install_dir" ]; then
    if is_script_install_dir "$install_dir"; then
      rm -rf "$install_dir"
      ok "Removed script install directory $install_dir"
    else
      warn "not removing $install_dir because it does not look like a claude-rs script install"
    fi
  fi

  ok "Script install uninstall complete"
}

manual_path_line() {
  if [ "$bin_dir" = "$HOME/.local/bin" ]; then
    # shellcheck disable=SC2016
    printf '%s\n' 'export PATH="$HOME/.local/bin:$PATH"'
  else
    # shellcheck disable=SC2016
    printf 'export PATH="%s:$PATH"\n' "$bin_dir"
  fi
}

path_has_bin_dir() {
  case ":$PATH:" in
    *":$bin_dir:"*) return 0 ;;
    *) return 1 ;;
  esac
}

path_starts_with_bin_dir() {
  case "$PATH:" in
    "$bin_dir:"*) return 0 ;;
    *) return 1 ;;
  esac
}

maybe_update_path() {
  if path_starts_with_bin_dir; then
    ok "PATH already points to this script install"
    return
  fi
  [ "$no_modify_path" -eq 1 ] && {
    warn "PATH update skipped"
    info "Add this to your shell profile:"
    manual_path_line
    if path_has_bin_dir; then
      warn "$bin_dir is already on PATH but not first; another claude-rs may take precedence in new shells"
    fi
    return
  }

  should_modify=0
  if [ "$yes" -eq 1 ]; then
    should_modify=1
  elif [ "$non_interactive" -eq 0 ] && [ -r /dev/tty ] && [ -w /dev/tty ]; then
    printf 'Add %s to PATH in %s? [y/N] ' "$bin_dir" "$HOME/.profile" > /dev/tty
    IFS= read -r answer < /dev/tty || answer=
    case "$answer" in
      y | Y | yes | YES) should_modify=1 ;;
    esac
  fi

  if [ "$should_modify" -eq 1 ]; then
    profile="$HOME/.profile"
    remove_managed_path_block
    {
      printf '\n# claude-rs PATH start\n'
      manual_path_line
      printf '# claude-rs PATH end\n'
    } >> "$profile"
    ok "Updated PATH for new shells"
  else
    warn "PATH update skipped"
    info "Add this to your shell profile:"
    manual_path_line
  fi
}

warn_other_claude_rs_commands() {
  launcher="$bin_dir/claude-rs"
  if command -v which >/dev/null 2>&1; then
    which -a claude-rs 2>/dev/null | while IFS= read -r candidate; do
      [ -n "$candidate" ] || continue
      [ "$candidate" = "$launcher" ] && continue
      warn "Another claude-rs is also on PATH: $candidate"
      warn_detail "  If a new shell runs that copy, remove it with: npm uninstall -g $root_package"
    done
  fi
}

if [ "$uninstall" -eq 1 ]; then
  need_cmd mkdir
  need_cmd rm
  need_cmd grep
  need_cmd awk
  need_cmd mv
  trap 'rm -rf "${lock_dir:-}"' EXIT HUP INT TERM
  uninstall_script_install
  exit 0
fi

need_cmd uname
need_cmd mktemp
need_cmd mkdir
need_cmd mv
need_cmd rm
need_cmd tar
need_cmd chmod
need_cmd grep
need_cmd awk

tmpdir="$(mktemp -d "${TMPDIR:-/tmp}/claude-rs-install.XXXXXX")"
cleanup() {
  [ -z "${lock_dir:-}" ] || rm -rf "$lock_dir"
  rm -rf "$tmpdir"
}
trap cleanup EXIT HUP INT TERM

target="$(detect_target)"
case "$target" in
  darwin-arm64)
    target_label="macOS arm64"
    binary_name="claude-rs"
    runtime_name="claude-rs-bridge-bun"
    ;;
  darwin-x64)
    target_label="macOS x64"
    binary_name="claude-rs"
    runtime_name="claude-rs-bridge-bun"
    ;;
  linux-arm64-gnu)
    target_label="Linux arm64 glibc"
    binary_name="claude-rs"
    runtime_name="claude-rs-bridge-bun"
    ;;
  linux-x64-gnu)
    target_label="Linux x64 glibc"
    binary_name="claude-rs"
    runtime_name="claude-rs-bridge-bun"
    ;;
  *)
    die "unsupported Unix install target: $target"
    ;;
esac

info "Installing claude-rs"
info ""
ok "$target_label detected"
ok "Install location: $install_dir"

tag="$(resolve_tag "$release")"
archive_name="$(archive_name_for_target "$target")"
base_url="https://github.com/$repo_slug/releases/download/$tag"
checksum_file="$tmpdir/SHA256SUMS"
archive_file="$tmpdir/$archive_name"

ok "Release $tag selected"
resolve_npm_install_choice

download "$base_url/SHA256SUMS" "$checksum_file" || die "could not download SHA256SUMS for $tag"
if ! download "$base_url/$archive_name" "$archive_file"; then
  die_unavailable
fi
ok "Downloaded release archive"

expected_sha="$(checksum_for_archive "$checksum_file" "$archive_name")"
[ -n "$expected_sha" ] || die "SHA256SUMS does not contain dist-install/$archive_name"
actual_sha="$(sha256_file "$archive_file")"
[ "$actual_sha" = "$expected_sha" ] || die "checksum mismatch for $archive_name"
ok "Verified release archive integrity"

validate_tar_listing "$archive_file"
extract_dir="$tmpdir/extract"
mkdir -p "$extract_dir"
tar -xzf "$archive_file" -C "$extract_dir"
set -- "$extract_dir"/*
if [ "$#" -ne 1 ] || [ ! -d "$1" ]; then
  die "archive extraction did not produce exactly one app directory"
fi
extracted_app="$1"
validate_extracted_app "$extracted_app"

install_parent="$(dirname "$install_dir")"
mkdir -p "$install_parent"
lock_dir="$(acquire_lock "$install_parent")"
replace_app_dir "$extracted_app" "$install_dir"
write_launcher
ok "Installed files"

maybe_update_path
PATH="$bin_dir:$PATH"
export PATH

version_output="$("$install_dir/$binary_name" --version)" ||
  die "installed claude-rs did not run successfully"
[ -n "$version_output" ] || die "installed claude-rs did not print a version"
"$install_dir/$binary_name" --help >/dev/null ||
  die "installed claude-rs help check failed"
ok "Verified $version_output"

if [ "$verify" -eq 1 ]; then
  doctor_output="$("$install_dir/$binary_name" doctor --strict 2>&1)" || {
    [ -z "$doctor_output" ] || printf '%s\n' "$doctor_output"
    die "runtime diagnostics failed"
  }
  ok "Runtime diagnostics passed"
fi

resolved="$(command -v claude-rs || true)"
launcher="$bin_dir/claude-rs"
if [ "$resolved" != "$launcher" ]; then
  warn "claude-rs resolves to $resolved instead of $launcher"
fi
warn_other_claude_rs_commands

info ""
info "claude-rs is installed."
if [ "$run_after_install" -eq 1 ] || confirm_default_no "Start claude-rs now?"; then
  launch_installed
else
  if [ "$no_modify_path" -eq 1 ]; then
    info "Run directly: $install_dir/$binary_name"
  else
    info "Run in a new shell: claude-rs"
  fi
fi
