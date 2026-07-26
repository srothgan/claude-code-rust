#!/bin/sh
set -eu

repo_owner="srothgan"
repo_name="claude-code-rust"
repo_slug="$repo_owner/$repo_name"
root_package="claude-code-rust"
download_retry_count=3
download_connect_timeout_seconds=30
download_low_speed_bytes_per_second=1024
download_low_speed_time_seconds=30

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
update=0
path_updated=0

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
case "${CLAUDE_RS_UPDATE:-}" in
  1 | true | TRUE | yes | YES) update=1 ;;
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
  --yes, -y                 Reinstall the selected version when already installed;
                            accept safe prompts; optional prompts are skipped.
  --non-interactive         Do not prompt.
  --no-modify-path          Do not update shell profile PATH.
  --verify                  Show download diagnostics and run strict runtime
                            diagnostics after install.
  --run                     Start claude-rs after a successful install.
  --remove-npm              Remove an existing global npm install when found.
  --keep-npm                Keep an existing global npm install without prompting.
  --uninstall               Remove the script install layout and managed PATH block.
  --update                  Update an existing script install in place.
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
  CLAUDE_RS_UPDATE
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
    --update)
      update=1
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

[ "$update" -eq 0 ] || [ "$uninstall" -eq 0 ] || {
  echo "error: --update and --uninstall cannot be used together" >&2
  exit 1
}
if [ "$update" -eq 1 ]; then
  yes=1
  non_interactive=1
  no_modify_path=1
  keep_npm=1
fi
[ "$remove_npm" -eq 0 ] || [ "$keep_npm" -eq 0 ] || {
  echo "error: --remove-npm and --keep-npm cannot be used together" >&2
  exit 1
}

green=""
yellow=""
red=""
cyan=""
reset=""
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  green="$(printf '\033[32m')"
  yellow="$(printf '\033[33m')"
  red="$(printf '\033[31m')"
  cyan="$(printf '\033[36m')"
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
progress_frames="$(printf '| / - \134')"

progress_enabled=0
progress_pid=""
progress_rendered_file=""
download_pid=""
if [ -t 1 ] && [ -z "${CI:-}" ] && [ "${TERM:-}" != "dumb" ] && [ -w /dev/tty ]; then
  progress_enabled=1
fi

progress_render() {
  progress_message="$1"
  sleep 0.2
  while :; do
    # shellcheck disable=SC2086 # Frames are intentionally split on spaces.
    for progress_frame in $progress_frames; do
      : > "$progress_rendered_file"
      printf '\r\033[2K%s%s%s %s' "$cyan" "$progress_frame" "$reset" "$progress_message" > /dev/tty
      sleep 0.15
    done
  done
}

progress_stop() {
  saved_progress_pid="${progress_pid:-}"
  saved_progress_rendered_file="${progress_rendered_file:-}"
  progress_pid=""
  progress_rendered_file=""

  if [ -n "$saved_progress_pid" ]; then
    kill "$saved_progress_pid" 2>/dev/null || :
    wait "$saved_progress_pid" 2>/dev/null || :
  fi
  if [ "$progress_enabled" -eq 1 ] && [ -n "$saved_progress_rendered_file" ] && [ -f "$saved_progress_rendered_file" ]; then
    printf '\r\033[2K' > /dev/tty 2>/dev/null || :
  fi
  [ -z "$saved_progress_rendered_file" ] || rm -f "$saved_progress_rendered_file"
}

progress_start() {
  progress_stop
  [ "$progress_enabled" -eq 1 ] || return 0

  progress_rendered_file="$tmpdir/.progress-rendered"
  rm -f "$progress_rendered_file"
  progress_render "$1" &
  progress_pid=$!
}

progress_done() {
  progress_stop
  ok "$1"
}

info() {
  progress_stop
  printf '%s\n' "$*"
}

ok() {
  progress_stop
  printf '%s%s%s %s\n' "$green" "$ok_mark" "$reset" "$*"
}

warn() {
  progress_stop
  printf '%s%s%s %s\n' "$yellow" "$warn_mark" "$reset" "$*" >&2
}

warn_detail() {
  progress_stop
  printf '%s\n' "$*" >&2
}

die() {
  progress_stop
  printf '%s%s%s %s\n' "$red" "$fail_mark" "$reset" "$*" >&2
  exit 1
}

can_prompt() {
  [ "$non_interactive" -eq 0 ] && [ -r /dev/tty ] && [ -w /dev/tty ]
}

launch_installed() {
  progress_stop
  if [ -r /dev/tty ]; then
    "$install_dir/$binary_name" < /dev/tty
  else
    "$install_dir/$binary_name"
  fi
}

confirm_default_no() {
  prompt="$1"
  progress_stop
  can_prompt || return 1
  printf '%s [y/N] ' "$prompt" > /dev/tty
  IFS= read -r answer < /dev/tty || answer=
  case "$answer" in
    y | Y | yes | YES) return 0 ;;
    *) return 1 ;;
  esac
}

die_unavailable() {
  progress_stop
  printf '%s\n' "install script is currently not available for this release" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

warn_missing_claude_cli() {
  command -v claude >/dev/null 2>&1 && return 0
  warn "Claude Code CLI ('claude') not found on PATH"
  warn_detail "  Install it from https://claude.com/claude-code"
}

download() {
  url="$1"
  destination="$2"
  if command -v curl >/dev/null 2>&1; then
    if [ "$progress_enabled" -eq 0 ]; then
      curl -fsSL \
        --retry "$download_retry_count" \
        --connect-timeout "$download_connect_timeout_seconds" \
        --speed-limit "$download_low_speed_bytes_per_second" \
        --speed-time "$download_low_speed_time_seconds" \
        "$url" -o "$destination"
      return $?
    fi
    if curl -fsSL \
      --retry "$download_retry_count" \
      --connect-timeout "$download_connect_timeout_seconds" \
      --speed-limit "$download_low_speed_bytes_per_second" \
      --speed-time "$download_low_speed_time_seconds" \
      "$url" -o "$destination" 2>"$tmpdir/download.stderr"; then
      return 0
    else
      download_status=$?
    fi
    progress_stop
    while IFS= read -r download_detail || [ -n "$download_detail" ]; do
      warn_detail "$download_detail"
    done < "$tmpdir/download.stderr"
    return "$download_status"
  fi
  if command -v wget >/dev/null 2>&1; then
    if [ "$progress_enabled" -eq 0 ]; then
      wget -q -O "$destination" "$url"
      return $?
    fi
    if wget -q -O "$destination" "$url" 2>"$tmpdir/download.stderr"; then
      return 0
    else
      download_status=$?
    fi
    progress_stop
    while IFS= read -r download_detail || [ -n "$download_detail" ]; do
      warn_detail "$download_detail"
    done < "$tmpdir/download.stderr"
    return "$download_status"
  fi
  die "required command not found: curl or wget"
}

format_download_bytes() {
  awk -v bytes="$1" 'BEGIN {
    split("B KiB MiB GiB", units, " ")
    value = bytes + 0
    unit = 1
    while (value >= 1024 && unit < 4) {
      value /= 1024
      unit++
    }
    if (unit == 1) {
      printf "%.0f %s", value, units[unit]
    } else {
      printf "%.1f %s", value, units[unit]
    }
  }'
}

write_download_diagnostic() {
  download_stats="$1"
  download_label="$2"
  [ "$verify" -eq 1 ] || return 0

  download_stats="${download_stats#__CLAUDE_RS_DOWNLOAD_STATS__}"
  saved_ifs="$IFS"
  IFS="$(printf '\t')"
  # shellcheck disable=SC2086 # curl's tab-separated fields are intentionally split.
  set -- $download_stats
  IFS="$saved_ifs"
  [ "$#" -eq 4 ] || return 0

  download_http_code="$1"
  download_size="$2"
  download_speed="$3"
  download_elapsed="$4"
  download_size_text="$(format_download_bytes "$download_size")"
  download_speed_text="$(format_download_bytes "$download_speed")"
  info "  $download_label: $download_size_text in ${download_elapsed}s ($download_speed_text/s, HTTP $download_http_code)"
}

download_content_length() {
  headers_path="$1"
  [ -f "$headers_path" ] || {
    printf '0\n'
    return
  }
  awk '
    tolower($1) == "content-length:" {
      gsub("\r", "", $2)
      length = $2
    }
    END { print length + 0 }
  ' "$headers_path"
}

format_download_progress() {
  downloaded_bytes="$1"
  total_bytes="$2"
  elapsed_seconds="$3"
  include_diagnostics="$4"
  unknown_position="$5"

  awk \
    -v downloaded="$downloaded_bytes" \
    -v total="$total_bytes" \
    -v elapsed="$elapsed_seconds" \
    -v diagnostics="$include_diagnostics" \
    -v unknown_position="$unknown_position" '
    function repeat(character, count, result) {
      result = ""
      while (count-- > 0) {
        result = result character
      }
      return result
    }
    function human(bytes, value, unit) {
      split("B KiB MiB GiB", units, " ")
      value = bytes + 0
      unit = 1
      while (value >= 1024 && unit < 4) {
        value /= 1024
        unit++
      }
      return unit == 1 ? sprintf("%.0f %s", value, units[unit]) : sprintf("%.1f %s", value, units[unit])
    }
    function eta_text(seconds, hours, minutes) {
      if (seconds < 0) {
        return "--:--"
      }
      seconds = int(seconds + 0.999)
      hours = int(seconds / 3600)
      minutes = int((seconds % 3600) / 60)
      seconds %= 60
      return hours > 0 ? sprintf("%02d:%02d:%02d", hours, minutes, seconds) : sprintf("%02d:%02d", minutes, seconds)
    }
    BEGIN {
      width = 10
      if (total > 0) {
        percent = int((downloaded * 100) / total)
        if (percent < 0) percent = 0
        if (percent > 100) percent = 100
        completed = int((percent * width) / 100)
        if (percent >= 100) {
          bar = repeat("=", width)
        } else {
          equals = completed < width ? completed : width - 1
          bar = repeat("=", equals) ">" repeat(".", width - equals - 1)
        }
        line = sprintf("[%s] %3d%% Downloading release archive", bar, percent)
      } else {
        position = unknown_position % width
        bar = repeat(".", position) ">" repeat(".", width - position - 1)
        line = sprintf("[%s]  --%% Downloading release archive", bar)
      }

      if (diagnostics == 1) {
        safe_elapsed = elapsed > 0 ? elapsed : 1
        speed = downloaded / safe_elapsed
        eta = total > 0 && speed > 0 ? eta_text((total - downloaded) / speed) : "--:--"
        total_text = total > 0 ? human(total) : "unknown"
        line = sprintf("%s | %s / %s | %s/s | ETA %s", line, human(downloaded), total_text, human(speed), eta)
      }
      print line
    }
  '
}

render_download_progress() {
  downloaded_bytes="$1"
  total_bytes="$2"
  elapsed_seconds="$3"
  unknown_position="$4"
  progress_text="$(format_download_progress "$downloaded_bytes" "$total_bytes" "$elapsed_seconds" "$verify" "$unknown_position")"
  printf '\r\033[2K%s' "$progress_text" > /dev/tty
}

finish_download_progress() {
  downloaded_bytes="$1"
  total_bytes="$2"
  elapsed_seconds="$3"
  progress_text="$(format_download_progress "$downloaded_bytes" "$total_bytes" "$elapsed_seconds" "$verify" 0)"
  printf '\r\033[2K%s\n' "$progress_text" > /dev/tty
}

clear_download_progress() {
  [ "$progress_enabled" -eq 1 ] || return 0
  printf '\r\033[2K' > /dev/tty 2>/dev/null || :
}

wait_for_download() {
  destination="$1"
  headers_path="$2"
  started_at="$3"
  total_bytes="$4"
  unknown_position=0

  if [ "$progress_enabled" -eq 1 ]; then
    while kill -0 "$download_pid" 2>/dev/null; do
      [ "$total_bytes" -gt 0 ] || total_bytes="$(download_content_length "$headers_path")"
      downloaded_bytes=0
      [ ! -f "$destination" ] || downloaded_bytes="$(wc -c < "$destination")"
      elapsed_seconds=0
      [ "$verify" -eq 0 ] || elapsed_seconds="$(($(date +%s) - started_at))"
      render_download_progress "$downloaded_bytes" "$total_bytes" "$elapsed_seconds" "$unknown_position"
      unknown_position="$((unknown_position + 1))"
      sleep 0.2
    done
  fi

  if wait "$download_pid"; then
    download_status=0
  else
    download_status=$?
  fi
  download_pid=""
  return "$download_status"
}

download_archive() {
  url="$1"
  destination="$2"
  progress_stop
  headers_path="$tmpdir/download.headers"
  stderr_path="$tmpdir/download.stderr"
  stats_path="$tmpdir/download.stats"
  rm -f "$headers_path" "$stderr_path" "$stats_path"
  download_started_at="$(date +%s)"

  if command -v curl >/dev/null 2>&1; then
    total_bytes=0
    if curl --fail --location --head --silent \
      --retry "$download_retry_count" \
      --connect-timeout "$download_connect_timeout_seconds" \
      --dump-header "$headers_path" \
      --output /dev/null \
      "$url"; then
      total_bytes="$(download_content_length "$headers_path")"
    fi

    curl_stats_format='__CLAUDE_RS_DOWNLOAD_STATS__%{http_code}\t%{size_download}\t%{speed_download}\t%{time_total}'
    curl --fail --location \
      --retry "$download_retry_count" \
      --connect-timeout "$download_connect_timeout_seconds" \
      --speed-limit "$download_low_speed_bytes_per_second" \
      --speed-time "$download_low_speed_time_seconds" \
      --silent --show-error \
      --dump-header "$headers_path" \
      --stderr "$stderr_path" \
      --output "$destination" \
      --write-out "$curl_stats_format" \
      "$url" > "$stats_path" &
    download_pid=$!

    if wait_for_download "$destination" "$headers_path" "$download_started_at" "$total_bytes"; then
      download_status=0
    else
      download_status=$?
    fi
    if [ "$download_status" -ne 0 ]; then
      clear_download_progress
      while IFS= read -r download_detail || [ -n "$download_detail" ]; do
        warn_detail "$download_detail"
      done < "$stderr_path"
      return "$download_status"
    fi

    download_finished_at="$(date +%s)"
    download_elapsed="$((download_finished_at - download_started_at))"
    downloaded_bytes="$(wc -c < "$destination")"
    [ "$total_bytes" -gt 0 ] || total_bytes="$downloaded_bytes"
    [ "$progress_enabled" -eq 0 ] || finish_download_progress "$downloaded_bytes" "$total_bytes" "$download_elapsed"
    curl_stats="$(cat "$stats_path")"
    write_download_diagnostic "$curl_stats" "Download"
    return 0
  fi

  if command -v wget >/dev/null 2>&1; then
    wget -q \
      --timeout="$download_connect_timeout_seconds" \
      --tries="$((download_retry_count + 1))" \
      -O "$destination" "$url" 2>"$stderr_path" &
    download_pid=$!

    if wait_for_download "$destination" "$headers_path" "$download_started_at" 0; then
      download_status=0
    else
      download_status=$?
    fi
    if [ "$download_status" -ne 0 ]; then
      clear_download_progress
      while IFS= read -r download_detail || [ -n "$download_detail" ]; do
        warn_detail "$download_detail"
      done < "$stderr_path"
      return "$download_status"
    fi

    download_finished_at="$(date +%s)"
    download_elapsed="$((download_finished_at - download_started_at))"
    [ "$download_elapsed" -gt 0 ] || download_elapsed=1
    downloaded_bytes="$(wc -c < "$destination")"
    [ "$progress_enabled" -eq 0 ] || finish_download_progress "$downloaded_bytes" "$downloaded_bytes" "$download_elapsed"
    if [ "$verify" -eq 1 ]; then
      download_speed="$((downloaded_bytes / download_elapsed))"
      info "  Download: $(format_download_bytes "$downloaded_bytes") in ${download_elapsed}s ($(format_download_bytes "$download_speed")/s, HTTP unavailable)"
    fi
    return 0
  fi

  die "required command not found: curl or wget"
}

stop_download_process() {
  if [ -n "${download_pid:-}" ]; then
    kill "$download_pid" 2>/dev/null || :
    wait "$download_pid" 2>/dev/null || :
    download_pid=""
  fi
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
        warn_detail "unsafe archive path: $entry"
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
  die "another claude-rs installer appears to be running: $lock_dir (remove this directory if no installer is running)"
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
  # The script install is already complete at this point; a failed npm
  # removal must not fail the install.
  if npm uninstall -g "$root_package" >/dev/null 2>&1; then
    ok "Removed npm install"
  else
    warn "could not remove npm install. Remove manually: npm uninstall -g $root_package"
  fi
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

  if [ "$keep_npm" -eq 0 ] && [ "$yes" -eq 0 ] &&
    confirm_default_no "Remove the npm install so only this installer owns \`claude-rs\` on PATH?"
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

script_install_version() {
  app="$1"
  is_script_install_dir "$app" || return 1
  sed -n 's/.*"version"[ 	]*:[ 	]*"\([^"]*\)".*/\1/p' "$app/package.json" | sed -n '1p'
}

release_version() {
  selected_tag="$1"
  printf '%s\n' "${selected_tag#v}"
}

approve_same_version_reinstall() {
  selected_version="$1"
  if [ "$update" -eq 1 ]; then
    return 1
  fi
  if [ "$yes" -eq 1 ]; then
    return 0
  fi
  confirm_default_no "claude-rs $selected_version is already installed at $install_dir. Reinstall the same version?"
}

guard_same_version_before_download() {
  selected_version="$1"
  is_script_install_dir "$install_dir" || return 0

  installed_version="$(script_install_version "$install_dir" || true)"
  if [ -z "$installed_version" ]; then
    warn "could not determine the version of the existing script install; continuing with installation"
    return 0
  fi
  [ "$installed_version" = "$selected_version" ] || return 0

  if approve_same_version_reinstall "$selected_version"; then
    same_version_reinstall_approved=1
    ok "Reinstalling claude-rs $selected_version"
    return 0
  fi

  ok "claude-rs $selected_version is already installed; no changes made"
  exit 0
}

stop_if_selected_version_became_installed() {
  selected_version="$1"
  [ "$same_version_reinstall_approved" -eq 0 ] || return 0
  is_script_install_dir "$install_dir" || return 0

  installed_version="$(script_install_version "$install_dir" || true)"
  [ -n "$installed_version" ] || return 0
  [ "$installed_version" = "$selected_version" ] || return 0

  progress_stop
  ok "claude-rs $selected_version was installed by another installer; no changes made"
  exit 0
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

zsh_profile_file() {
  printf '%s\n' "${ZDOTDIR:-$HOME}/.zprofile"
}

zsh_rc_file() {
  printf '%s\n' "${ZDOTDIR:-$HOME}/.zshrc"
}

# Cover login and interactive shells. A guarded managed block prevents a
# duplicate prepend when a login profile also sources its shell's rc file.
profile_targets() {
  printf '%s\n' "$HOME/.profile"
  if [ -f "$HOME/.bash_profile" ]; then
    printf '%s\n' "$HOME/.bash_profile"
  elif [ -f "$HOME/.bash_login" ]; then
    printf '%s\n' "$HOME/.bash_login"
  fi
  case "${SHELL:-}" in
    */bash) printf '%s\n' "$HOME/.bashrc" ;;
    *) [ ! -f "$HOME/.bashrc" ] || printf '%s\n' "$HOME/.bashrc" ;;
  esac
  zprofile="$(zsh_profile_file)"
  zshrc="$(zsh_rc_file)"
  case "${SHELL:-}" in
    */zsh)
      printf '%s\n' "$zprofile"
      printf '%s\n' "$zshrc"
      ;;
    *)
      if [ -f "$zprofile" ]; then
        printf '%s\n' "$zprofile"
      fi
      if [ -f "$zshrc" ]; then
        printf '%s\n' "$zshrc"
      fi
      ;;
  esac
}

remove_managed_path_block() {
  profile="$1"
  [ -f "$profile" ] || return 0
  tmp_profile="$profile.tmp.$$"
  awk '
    /^# claude-rs PATH start$/ { skip = 1; next }
    /^# claude-rs PATH end$/ { skip = 0; next }
    skip != 1 { print }
  ' "$profile" > "$tmp_profile" && mv "$tmp_profile" "$profile"
}

remove_managed_path_blocks() {
  remove_managed_path_block "$HOME/.profile"
  remove_managed_path_block "$HOME/.bash_profile"
  remove_managed_path_block "$HOME/.bash_login"
  remove_managed_path_block "$HOME/.bashrc"
  remove_managed_path_block "$(zsh_profile_file)"
  remove_managed_path_block "$(zsh_rc_file)"
}

uninstall_script_install() {
  install_parent="$(dirname "$install_dir")"
  mkdir -p "$install_parent"
  lock_dir="$(acquire_lock "$install_parent")"

  remove_launcher_if_owned
  remove_managed_path_blocks

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

managed_path_lines() {
  quoted_bin_dir="'$(printf '%s' "$bin_dir" | sed "s/'/'\\\\''/g")'"
  printf "case \"\$PATH:\" in\n"
  printf '  %s:*) ;;\n' "$quoted_bin_dir"
  printf "  *) export PATH=%s:\"\$PATH\" ;;\n" "$quoted_bin_dir"
  printf 'esac\n'
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
    path_updated=1
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
  elif confirm_default_no "Add $bin_dir to PATH in your shell profile?"; then
    should_modify=1
  fi

  if [ "$should_modify" -eq 1 ]; then
    remove_managed_path_blocks
    profile_targets | while IFS= read -r profile_file; do
      {
        printf '\n# claude-rs PATH start\n'
        managed_path_lines
        printf '# claude-rs PATH end\n'
      } >> "$profile_file"
    done
    path_updated=1
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

if [ "$update" -eq 1 ] && ! is_script_install_dir "$install_dir"; then
  die "--update requires an existing claude-rs script install: $install_dir"
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
need_cmd cat
need_cmd date
need_cmd wc

tmpdir="$(mktemp -d "${TMPDIR:-/tmp}/claude-rs-install.XXXXXX")"
same_version_reinstall_approved=0
cleanup() {
  stop_download_process
  progress_stop
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
warn_missing_claude_cli

progress_start "Resolving release"
tag="$(resolve_tag "$release")"
selected_version="$(release_version "$tag")"
archive_name="$(archive_name_for_target "$target")"
base_url="https://github.com/$repo_slug/releases/download/$tag"
checksum_file="$tmpdir/SHA256SUMS"
archive_file="$tmpdir/$archive_name"

progress_done "Release $tag selected"

guard_same_version_before_download "$selected_version"

progress_start "Downloading release archive"
download "$base_url/SHA256SUMS" "$checksum_file" || die "could not download SHA256SUMS for $tag"
if ! download_archive "$base_url/$archive_name" "$archive_file"; then
  die_unavailable
fi
progress_done "Downloaded release archive"

progress_start "Verifying release archive"
expected_sha="$(checksum_for_archive "$checksum_file" "$archive_name")"
[ -n "$expected_sha" ] || die "SHA256SUMS does not contain dist-install/$archive_name"
actual_sha="$(sha256_file "$archive_file")"
[ "$actual_sha" = "$expected_sha" ] || die "checksum mismatch for $archive_name"
progress_done "Verified release archive integrity"

progress_start "Installing files"
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
stop_if_selected_version_became_installed "$selected_version"
replace_app_dir "$extracted_app" "$install_dir"
if [ "$update" -eq 0 ]; then
  write_launcher
fi
progress_done "Installed files"

if [ "$update" -eq 0 ]; then
  maybe_update_path
else
  path_updated=1
  ok "Preserved existing launcher and PATH configuration"
fi
PATH="$bin_dir:$PATH"
export PATH

progress_start "Verifying installed command"
version_output="$("$install_dir/$binary_name" --version)" ||
  die "installed claude-rs did not run successfully"
[ -n "$version_output" ] || die "installed claude-rs did not print a version"
"$install_dir/$binary_name" --help >/dev/null ||
  die "installed claude-rs help check failed"
progress_done "Verified $version_output"

if [ "$verify" -eq 1 ]; then
  progress_start "Running runtime diagnostics"
  doctor_output="$("$install_dir/$binary_name" doctor --strict 2>&1)" || {
    [ -z "$doctor_output" ] || info "$doctor_output"
    die "runtime diagnostics failed"
  }
  progress_done "Runtime diagnostics passed"
fi

# Only offer to remove an existing npm install after the script install has
# fully succeeded, so a failed install never leaves the user without claude-rs.
if [ "$update" -eq 0 ]; then
  resolve_npm_install_choice
fi

resolved="$(command -v claude-rs || true)"
launcher="$bin_dir/claude-rs"
if [ "$resolved" != "$launcher" ]; then
  warn "claude-rs resolves to $resolved instead of $launcher"
fi
warn_other_claude_rs_commands

info ""
if [ "$update" -eq 1 ]; then
  info "claude-rs is updated. Start claude-rs again to use ${tag#v}."
else
  info "claude-rs is installed."
  if [ "$run_after_install" -eq 1 ] || { [ "$yes" -eq 0 ] && confirm_default_no "Start claude-rs now?"; }; then
    launch_installed
  else
    if [ "$path_updated" -eq 1 ]; then
      info "Run in a new shell: claude-rs"
    else
      info "Run directly: $install_dir/$binary_name"
    fi
  fi
fi
