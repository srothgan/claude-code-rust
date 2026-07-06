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

case "${CLAUDE_RS_NON_INTERACTIVE:-}" in
  1 | true | TRUE | yes | YES) non_interactive=1 ;;
esac
case "${CLAUDE_RS_NO_MODIFY_PATH:-}" in
  1 | true | TRUE | yes | YES) no_modify_path=1 ;;
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
  --yes, -y                 Accept prompts.
  --non-interactive         Do not prompt.
  --no-modify-path          Do not update shell profile PATH.
  --help                    Show this help.

Environment:
  CLAUDE_RS_RELEASE
  CLAUDE_RS_INSTALL_DIR
  CLAUDE_RS_BIN_DIR
  CLAUDE_RS_NON_INTERACTIVE
  CLAUDE_RS_NO_MODIFY_PATH
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

info() {
  printf '%s\n' "$*"
}

warn() {
  printf 'warning: %s\n' "$*" >&2
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
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

maybe_update_path() {
  path_has_bin_dir && return
  [ "$no_modify_path" -eq 1 ] && {
    info "Add this to your shell profile:"
    manual_path_line
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
    {
      printf '\n# claude-rs PATH start\n'
      manual_path_line
      printf '# claude-rs PATH end\n'
    } >> "$profile"
    info "Updated PATH in $profile"
  else
    info "Add this to your shell profile:"
    manual_path_line
  fi
}

need_cmd uname
need_cmd mktemp
need_cmd mkdir
need_cmd mv
need_cmd rm
need_cmd tar
need_cmd chmod

tmpdir="$(mktemp -d "${TMPDIR:-/tmp}/claude-rs-install.XXXXXX")"
cleanup() {
  [ -z "${lock_dir:-}" ] || rm -rf "$lock_dir"
  rm -rf "$tmpdir"
}
trap cleanup EXIT HUP INT TERM

target="$(detect_target)"
case "$target" in
  darwin-* | linux-*)
    binary_name="claude-rs"
    runtime_name="claude-rs-bridge-bun"
    ;;
  *)
    die "unsupported Unix install target: $target"
    ;;
esac

tag="$(resolve_tag "$release")"
archive_name="$(archive_name_for_target "$target")"
base_url="https://github.com/$repo_slug/releases/download/$tag"
checksum_file="$tmpdir/SHA256SUMS"
archive_file="$tmpdir/$archive_name"

info "Installing claude-rs $tag for $target"
download "$base_url/SHA256SUMS" "$checksum_file" || die "could not download SHA256SUMS for $tag"
if ! download "$base_url/$archive_name" "$archive_file"; then
  die_unavailable
fi

expected_sha="$(checksum_for_archive "$checksum_file" "$archive_name")"
[ -n "$expected_sha" ] || die "SHA256SUMS does not contain dist-install/$archive_name"
actual_sha="$(sha256_file "$archive_file")"
[ "$actual_sha" = "$expected_sha" ] || die "checksum mismatch for $archive_name"

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

maybe_update_path
PATH="$bin_dir:$PATH"
export PATH

claude-rs --version
claude-rs --help >/dev/null
claude-rs doctor --strict

resolved="$(command -v claude-rs || true)"
launcher="$bin_dir/claude-rs"
if [ "$resolved" != "$launcher" ]; then
  warn "claude-rs resolves to $resolved instead of $launcher"
fi

info "Installed claude-rs to $install_dir"
