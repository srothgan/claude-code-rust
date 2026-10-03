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
release_advisories_url="https://raw.githubusercontent.com/$repo_slug/main/scripts/install/release-advisories.json"
release_advisories_timeout_seconds=10

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

# Cyan marks information and interaction, magenta running work, green success.
bold=""
dim=""
gray=""
red=""
green=""
yellow=""
magenta=""
cyan=""
badge=""
reset=""
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  bold="$(printf '\033[1m')"
  dim="$(printf '\033[2m')"
  gray="$(printf '\033[90m')"
  red="$(printf '\033[31m')"
  green="$(printf '\033[32m')"
  yellow="$(printf '\033[33m')"
  magenta="$(printf '\033[35m')"
  cyan="$(printf '\033[36m')"
  badge="$(printf '\033[1;30;46m')"
  reset="$(printf '\033[0m')"
fi

# The installer draws one framed transcript: an intro line, one entry per step,
# and an outro line, joined by a gutter bar. Terminals without UTF-8 box drawing
# get an ASCII rendering of the same layout.
unicode=1
case "${LC_ALL:-${LC_CTYPE:-${LANG:-}}}" in
  C | POSIX) unicode=0 ;;
esac
[ "${TERM:-}" != "linux" ] || unicode=0
if [ "$unicode" -eq 1 ]; then
  s_bar_start="$(printf '\342\224\214')"
  s_bar="$(printf '\342\224\202')"
  s_bar_end="$(printf '\342\224\224')"
  s_bar_branch="$(printf '\342\224\234')"
  s_line="$(printf '\342\224\200')"
  s_corner_top="$(printf '\342\225\256')"
  s_corner_bottom="$(printf '\342\225\257')"
  s_step="$(printf '\342\227\207')"
  s_active="$(printf '\342\227\206')"
  s_info="$(printf '\342\227\217')"
  s_warn="$(printf '\342\226\262')"
  s_error="$(printf '\342\226\240')"
  s_fill="$(printf '\342\224\201')"
  s_empty="$(printf '\342\224\200')"
  progress_frames="$(printf '\342\227\222 \342\227\220 \342\227\223 \342\227\221')"
else
  s_bar_start="+"
  s_bar="|"
  s_bar_end="+"
  s_bar_branch="+"
  s_line="-"
  s_corner_top="+"
  s_corner_bottom="+"
  s_step="o"
  s_active="*"
  s_info="*"
  s_warn="!"
  s_error="x"
  s_fill="#"
  s_empty="-"
  progress_frames="$(printf '| / - \134')"
fi
tab_char="$(printf '\t')"
flow_label="Installation"
frame_open=0
interrupted=0
tmpdir=""
lock_dir=""

progress_enabled=0
progress_pid=""
progress_rendered_file=""
download_pid=""
saved_tty_state=""
cursor_hidden=0
prompt_state=""
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
      printf '\r\033[2K%s%s%s  %s' "$magenta" "$progress_frame" "$reset" "$progress_message" > /dev/tty
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

restore_tty() {
  if [ -n "$saved_tty_state" ]; then
    stty "$saved_tty_state" < /dev/tty 2>/dev/null || :
    saved_tty_state=""
  fi
  if [ "$cursor_hidden" -eq 1 ]; then
    printf '\033[?25h' > /dev/tty 2>/dev/null || :
    cursor_hidden=0
  fi
}

tildify() {
  case "$1" in
    "$HOME") printf '~\n' ;;
    "$HOME"/*) printf '~%s\n' "${1#"$HOME"}" ;;
    *) printf '%s\n' "$1" ;;
  esac
}

repeat_text() {
  repeat_count="$2"
  while [ "$repeat_count" -gt 0 ]; do
    printf '%s' "$1"
    repeat_count="$((repeat_count - 1))"
  done
}

gutter() {
  printf '%s%s%s\n' "$gray" "$s_bar" "$reset"
}

detail() {
  printf '%s%s%s  %s%s%s\n' "$gray" "$s_bar" "$reset" "$dim" "$1" "$reset"
}

# entry <color> <symbol> <message> [detail...]
entry() {
  entry_color="$1"
  entry_symbol="$2"
  entry_message="$3"
  shift 3
  progress_stop
  printf '%s%s%s  %s\n' "$entry_color" "$entry_symbol" "$reset" "$entry_message"
  for entry_detail in "$@"; do
    detail "$entry_detail"
  done
}

intro() {
  frame_open=1
  if [ -n "$badge" ]; then
    printf '%s%s%s  %s claude-rs %s %s%s%s\n' "$gray" "$s_bar_start" "$reset" "$badge" "$reset" "$dim" "$1" "$reset"
  else
    printf '%s  claude-rs %s\n' "$s_bar_start" "$1"
  fi
  gutter
}

outro() {
  progress_stop
  gutter
  printf '%s%s%s  %s%s%s\n' "$gray" "$s_bar_end" "$reset" "$bold" "$1" "$reset"
  frame_open=0
}

info() {
  entry "$cyan" "$s_info" "$@"
}

ok() {
  entry "$green" "$s_step" "$@"
}

warn() {
  entry "$yellow" "$s_warn" "$@" >&2
}

warn_detail() {
  progress_stop
  detail "$*" >&2
}

# The failing exit closes the frame through cleanup, so a die inside a command
# substitution or pipeline still ends the transcript exactly once.
die() {
  progress_stop
  printf '%s%s%s  %s\n' "$red" "$s_error" "$reset" "$*" >&2
  exit 1
}

# A titled box of follow-up instructions, attached to the gutter.
note() {
  note_title="$1"
  shift
  progress_stop
  note_width="${#note_title}"
  for note_line in "$@"; do
    [ "${#note_line}" -le "$note_width" ] || note_width="${#note_line}"
  done
  gutter
  printf '%s%s%s  %s %s%s%s%s\n' "$cyan" "$s_info" "$reset" "$note_title" "$gray" \
    "$(repeat_text "$s_line" "$((note_width + 1 - ${#note_title}))")" "$s_corner_top" "$reset"
  for note_line in "" "$@" ""; do
    printf '%s%s%s  %s%s%s%s  %s%s%s\n' "$gray" "$s_bar" "$reset" "$dim" "$note_line" "$reset" \
      "$(repeat_text " " "$((note_width - ${#note_line}))")" "$gray" "$s_bar" "$reset"
  done
  printf '%s%s%s%s%s\n' "$gray" "$s_bar_branch" "$(repeat_text "$s_line" "$((note_width + 4))")" \
    "$s_corner_bottom" "$reset"
}

terminal_columns() {
  columns="$(stty size 2>/dev/null < /dev/tty | sed 's/^[^ ]* //')" || columns=""
  case "$columns" in
    "" | *[!0-9]* | 0) columns=80 ;;
  esac
  printf '%s\n' "$columns"
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

confirm_answer_label() {
  if [ "$1" -eq 1 ]; then
    printf 'Yes\n'
  else
    printf 'No\n'
  fi
}

# Immediate y/n confirmation. Returns 1 only when single-key input is unavailable.
confirm_single_key() {
  confirm_question="$1"
  command -v stty >/dev/null 2>&1 || return 1
  command -v dd >/dev/null 2>&1 || return 1
  confirm_tty_state="$(stty -g < /dev/tty 2>/dev/null)" || return 1
  [ -n "$confirm_tty_state" ] || return 1
  stty -icanon -echo min 1 time 0 < /dev/tty 2>/dev/null || return 1
  saved_tty_state="$confirm_tty_state"
  cursor_hidden=1
  prompt_state="single_key"
  printf '\033[?25l%s%s%s  %s\n' "$cyan" "$s_active" "$reset" "$confirm_question" > /dev/tty
  printf '%s%s%s  %sy%s Yes / %sN%s No%s\n' "$cyan" "$s_bar" "$reset" "$cyan" "$gray" "$cyan" "$gray" "$reset" > /dev/tty
  printf '%s%s%s\n' "$cyan" "$s_bar_end" "$reset" > /dev/tty

  while :; do
    confirm_byte="$(dd bs=1 count=1 2>/dev/null < /dev/tty)" || {
      restore_tty
      prompt_state=""
      return 0
    }
    case "$confirm_byte" in
      y | Y)
        confirm_selected=1
        break
        ;;
      n | N)
        confirm_selected=0
        break
        ;;
      # Only the literal answer keys submit the question.
      *) continue ;;
    esac
  done

  confirm_columns="$(terminal_columns)"
  restore_tty
  prompt_state=""
  # Replace the active question with its answer.
  confirm_rows="$(((${#confirm_question} + 3 + confirm_columns - 1) / confirm_columns + (15 + confirm_columns - 1) / confirm_columns + 1))"
  printf '\033[%sA\r\033[J' "$confirm_rows" > /dev/tty
  info "$confirm_question" "$(confirm_answer_label "$confirm_selected")"
}

confirm_default_no() {
  prompt="$1"
  progress_stop
  can_prompt || return 1
  confirm_selected=0
  if [ "$progress_enabled" -eq 0 ] || ! confirm_single_key "$prompt"; then
    prompt_state="typed"
    printf '%s%s%s  %s %s[y/N]%s ' "$cyan" "$s_active" "$reset" "$prompt" "$dim" "$reset" > /dev/tty
    IFS= read -r answer < /dev/tty || answer=
    prompt_state=""
    case "$answer" in
      y | Y | yes | YES) confirm_selected=1 ;;
    esac
  fi
  [ "$confirm_selected" -eq 1 ]
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

warn_missing_claude_cli() {
  command -v claude >/dev/null 2>&1 && return 0
  warn "Claude Code CLI ('claude') not found on PATH" "Install it from https://claude.com/claude-code"
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

# Prints the --verify detail line for curl's tab-separated transfer stats.
format_download_diagnostic() {
  download_stats="${1#__CLAUDE_RS_DOWNLOAD_STATS__}"
  saved_ifs="$IFS"
  IFS="$tab_char"
  # shellcheck disable=SC2086 # curl's tab-separated fields are intentionally split.
  set -- $download_stats
  IFS="$saved_ifs"
  [ "$#" -eq 4 ] || return 0

  printf 'Download: %s in %ss (%s/s, HTTP %s)\n' \
    "$(format_download_bytes "$2")" "$4" "$(format_download_bytes "$3")" "$1"
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
      content_length = $2
    }
    END { print content_length + 0 }
  ' "$headers_path"
}

download_bar_width=20

# Prints "<lead>|<fill>|<percent>|<sizes>|<rate>" for the live download line.
# With an unknown total the filled cells slide across the bar instead.
download_progress_fields() {
  awk \
    -v downloaded="$1" \
    -v total="$2" \
    -v elapsed="$3" \
    -v tick="$4" \
    -v width="$download_bar_width" '
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
      if (total > 0) {
        percent = int((downloaded * 100) / total)
        if (percent < 0) percent = 0
        if (percent > 100) percent = 100
        lead = 0
        fill = int((percent * width) / 100)
        percent_text = sprintf("%3d%%", percent)
        sizes = human(downloaded) " / " human(total)
      } else {
        fill = 4
        lead = tick % (width - fill + 1)
        percent_text = ""
        sizes = human(downloaded)
      }

      safe_elapsed = elapsed > 0 ? elapsed : 1
      speed = downloaded / safe_elapsed
      eta = total > 0 && speed > 0 ? eta_text((total - downloaded) / speed) : "--:--"
      printf "%d|%d|%s|%s|%s/s  ETA %s\n", lead, fill, percent_text, sizes, human(speed), eta
    }
  '
}

# Trailing segments are dropped rather than wrapped when the terminal is narrow,
# because a wrapped line cannot be redrawn in place.
render_download_progress() {
  IFS='|' read -r bar_lead bar_fill bar_percent bar_sizes bar_rate <<EOF
$(download_progress_fields "$1" "$2" "$3" "$4")
EOF
  [ "$verify" -eq 1 ] || bar_rate=""
  bar_frame_index="$(($4 % 4))"
  # shellcheck disable=SC2086 # Frames are intentionally split on spaces.
  set -- $progress_frames
  shift "$bar_frame_index"

  bar_columns="$(terminal_columns)"
  bar_line="$magenta$1$reset  Downloading"
  bar_line_width=14
  if [ "$((bar_line_width + 2 + download_bar_width))" -lt "$bar_columns" ]; then
    bar_line="$bar_line  $gray$(repeat_text "$s_empty" "$bar_lead")$cyan$(repeat_text "$s_fill" "$bar_fill")"
    bar_line="$bar_line$gray$(repeat_text "$s_empty" "$((download_bar_width - bar_lead - bar_fill))")$reset"
    bar_line_width="$((bar_line_width + 2 + download_bar_width))"
  fi
  if [ -n "$bar_percent" ]; then
    bar_line="$bar_line $bar_percent"
    bar_line_width="$((bar_line_width + 1 + ${#bar_percent}))"
  fi
  for bar_segment in "$bar_sizes" "$bar_rate"; do
    [ -n "$bar_segment" ] || continue
    [ "$((bar_line_width + 2 + ${#bar_segment}))" -lt "$bar_columns" ] || break
    bar_line="$bar_line  $dim$bar_segment$reset"
    bar_line_width="$((bar_line_width + 2 + ${#bar_segment}))"
  done
  printf '\r\033[2K%s' "$bar_line" > /dev/tty
}

clear_download_progress() {
  [ "$progress_enabled" -eq 1 ] || return 0
  printf '\r\033[2K' > /dev/tty 2>/dev/null || :
}

# The live download line collapses into the one completed entry for the archive.
finish_download() {
  clear_download_progress
  if [ "$verify" -eq 1 ] && [ -n "$2" ]; then
    ok "Downloaded release archive ($(format_download_bytes "$1"))" "$2"
  else
    ok "Downloaded release archive ($(format_download_bytes "$1"))"
  fi
}

report_download_failure() {
  clear_download_progress
  while IFS= read -r download_detail || [ -n "$download_detail" ]; do
    warn_detail "$download_detail"
  done < "$1"
}

wait_for_download() {
  destination="$1"
  headers_path="$2"
  started_at="$3"
  total_bytes="$4"
  download_tick=0

  progress_stop
  if [ "$progress_enabled" -eq 1 ]; then
    while kill -0 "$download_pid" 2>/dev/null; do
      [ "$total_bytes" -gt 0 ] || total_bytes="$(download_content_length "$headers_path")"
      downloaded_bytes=0
      [ ! -f "$destination" ] || downloaded_bytes="$(wc -c < "$destination")"
      elapsed_seconds=0
      [ "$verify" -eq 0 ] || elapsed_seconds="$(($(date +%s) - started_at))"
      render_download_progress "$downloaded_bytes" "$total_bytes" "$elapsed_seconds" "$download_tick"
      download_tick="$((download_tick + 1))"
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
      report_download_failure "$stderr_path"
      return "$download_status"
    fi

    finish_download "$(wc -c < "$destination")" "$(format_download_diagnostic "$(cat "$stats_path")")"
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
      report_download_failure "$stderr_path"
      return "$download_status"
    fi

    download_elapsed="$(($(date +%s) - download_started_at))"
    [ "$download_elapsed" -gt 0 ] || download_elapsed=1
    downloaded_bytes="$(wc -c < "$destination")"
    download_speed="$((downloaded_bytes / download_elapsed))"
    finish_download "$downloaded_bytes" \
      "Download: $(format_download_bytes "$downloaded_bytes") in ${download_elapsed}s ($(format_download_bytes "$download_speed")/s, HTTP unavailable)"
    return 0
  fi

  die "required command not found: curl or wget"
}

stop_download_process() {
  if [ -n "$download_pid" ]; then
    kill "$download_pid" 2>/dev/null || :
    wait "$download_pid" 2>/dev/null || :
    download_pid=""
    clear_download_progress
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
        die "unsafe archive path: $entry"
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

launcher_contents() {
  printf '%s\n' '#!/bin/sh'
  printf "exec '%s' \"\$@\"\n" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

write_launcher() {
  mkdir -p "$bin_dir"
  launcher="$bin_dir/claude-rs"
  tmp_launcher="$launcher.tmp.$$"
  app_binary="$install_dir/$binary_name"
  launcher_contents "$app_binary" > "$tmp_launcher"
  chmod 755 "$tmp_launcher"
  mv "$tmp_launcher" "$launcher"
}

man_link_dir() {
  printf '%s/share/man/man1\n' "$(dirname "${1:-$bin_dir}")"
}

man_source_dir() {
  manual_install_dir="${1:-$install_dir}"
  printf '%s/%s/share/man/man1\n' "$(CDPATH='' cd "$(dirname "$manual_install_dir")" && pwd -P)" "$(basename "$manual_install_dir")"
}

install_man_pages() {
  remove_man_pages_if_owned
  manual_dir="$(man_source_dir)"
  [ -d "$manual_dir" ] || return 0
  link_dir="$(man_link_dir)"
  mkdir -p "$link_dir"
  for manual in "$manual_dir"/claude-rs*.1; do
    [ -f "$manual" ] || continue
    destination="$link_dir/$(basename "$manual")"
    if [ -e "$destination" ] || [ -L "$destination" ]; then
      warn "not replacing existing manual $destination"
      continue
    fi
    ln -s "$manual" "$destination"
  done
}

remove_man_pages_if_owned() {
  link_dir="$(man_link_dir "${2:-$bin_dir}")"
  source_dir="$(man_source_dir "${1:-$install_dir}")"
  for destination in "$link_dir"/claude-rs*.1; do
    [ -L "$destination" ] || continue
    target="$(readlink "$destination")"
    case "$target" in
      "$source_dir/"claude-rs*.1) rm -f "$destination" ;;
    esac
  done
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

  warn "Existing npm install found: $root_package $npm_package_version" "Location: $npm_root/$root_package"
  if [ "$remove_npm" -eq 1 ]; then
    remove_npm_install
    return
  fi

  if [ "$keep_npm" -eq 0 ] && [ "$yes" -eq 0 ] &&
    confirm_default_no "Uninstall this npm installation of claude-rs?"
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

# Release advisories are optional guidance. A single short attempt keeps an
# unreachable advisory file from delaying or failing the install.
fetch_release_advisories() {
  advisories_destination="$1"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL \
      --connect-timeout "$release_advisories_timeout_seconds" \
      --max-time "$release_advisories_timeout_seconds" \
      "$release_advisories_url" -o "$advisories_destination" 2>/dev/null
    return $?
  fi
  if command -v wget >/dev/null 2>&1; then
    wget -q -T "$release_advisories_timeout_seconds" -t 1 \
      -O "$advisories_destination" "$release_advisories_url" 2>/dev/null
    return $?
  fi
  return 1
}

# Prints the summary of every advisory whose inclusive start..end range contains
# the version. Only the MAJOR.MINOR.PATCH core is compared. Advisory values never
# contain quotes or backslashes (enforced by release-advisories.test.mjs), so
# fields can be extracted without a JSON parser.
matching_release_advisories() {
  awk -v version="$2" '
    function field(name,    pattern, text) {
      pattern = "\"" name "\"[ \t\r\n]*:[ \t\r\n]*\"[^\"]*\""
      if (!match($0, pattern)) return ""
      text = substr($0, RSTART, RLENGTH)
      sub(/^"[a-z]*"[ \t\r\n]*:[ \t\r\n]*"/, "", text)
      sub(/"$/, "", text)
      return text
    }
    function compare(left, right,    a, b, i) {
      split(left, a, ".")
      split(right, b, ".")
      for (i = 1; i <= 3; i++) {
        if (a[i] + 0 < b[i] + 0) return -1
        if (a[i] + 0 > b[i] + 0) return 1
      }
      return 0
    }
    BEGIN {
      RS = "}"
      plain = "^[0-9]+\\.[0-9]+\\.[0-9]+$"
      if (version !~ /^[0-9]+\.[0-9]+\.[0-9]+([-+].*)?$/) exit
      sub(/[-+].*$/, "", version)
    }
    {
      first = field("start")
      last = field("end")
      summary = field("summary")
      if (first !~ plain || last !~ plain || summary == "") next
      if (compare(first, version) <= 0 && compare(version, last) <= 0) print summary
    }
  ' "$1"
}

warn_release_advisories() {
  advisory_version="$1"
  advisories_file="$tmpdir/release-advisories.json"
  advisory_matches_file="$tmpdir/release-advisories.txt"
  fetch_release_advisories "$advisories_file" || return 0
  matching_release_advisories "$advisories_file" "$advisory_version" \
    > "$advisory_matches_file" 2>/dev/null || return 0
  while IFS= read -r advisory_summary; do
    warn "Known issue in claude-rs $advisory_version: $advisory_summary"
  done < "$advisory_matches_file"
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
    info "Reinstalling claude-rs $selected_version"
    return 0
  fi

  outro "claude-rs $selected_version is already installed; no changes made"
  exit 0
}

stop_if_selected_version_became_installed() {
  selected_version="$1"
  [ "$same_version_reinstall_approved" -eq 0 ] || return 0
  is_script_install_dir "$install_dir" || return 0

  installed_version="$(script_install_version "$install_dir" || true)"
  [ -n "$installed_version" ] || return 0
  [ "$installed_version" = "$selected_version" ] || return 0

  outro "claude-rs $selected_version was installed by another installer; no changes made"
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
  only_bin_dir="${2:-}"
  [ -f "$profile" ] || return 0
  tmp_profile="$profile.tmp.$$"
  managed_assignment=""
  legacy_assignment=""
  if [ -n "$only_bin_dir" ]; then
    managed_assignment="$(managed_path_lines "$only_bin_dir" | sed -n '/export PATH=/p')"
    legacy_assignment="$(manual_path_line "$only_bin_dir")"
  fi
  awk -v selected="$only_bin_dir" -v managed="$managed_assignment" -v legacy="$legacy_assignment" '
    /^# claude-rs PATH start$/ {
      inside = 1
      block = $0 "\n"
      remove = (selected == "")
      next
    }
    inside {
      block = block $0 "\n"
      if ($0 == managed || $0 == legacy) remove = 1
      if ($0 == "# claude-rs PATH end") {
        if (!remove) printf "%s", block
        inside = 0
        block = ""
      }
      next
    }
    { print }
    END { if (inside && !remove) printf "%s", block }
  ' "$profile" > "$tmp_profile" && mv "$tmp_profile" "$profile"
}

remove_managed_path_blocks() {
  remove_managed_path_block "$HOME/.profile" "${1:-}"
  remove_managed_path_block "$HOME/.bash_profile" "${1:-}"
  remove_managed_path_block "$HOME/.bash_login" "${1:-}"
  remove_managed_path_block "$HOME/.bashrc" "${1:-}"
  remove_managed_path_block "$(zsh_profile_file)" "${1:-}"
  remove_managed_path_block "$(zsh_rc_file)" "${1:-}"
}

uninstall_script_install() (
  uninstall_directory="${1:-$install_dir}"
  uninstall_parent="$(dirname "$uninstall_directory")"
  mkdir -p "$uninstall_parent" || return 1
  uninstall_lock="$(acquire_lock "$uninstall_parent")" || return 1
  trap 'rmdir "$uninstall_lock"' EXIT

  if [ -e "$uninstall_directory" ] && ! is_script_install_dir "$uninstall_directory"; then
    warn "not removing $uninstall_directory because it does not look like a claude-rs script install"
    return 0
  fi
  if [ "$#" -eq 0 ]; then
    remove_launcher_if_owned
    remove_man_pages_if_owned
    remove_managed_path_blocks "$bin_dir"
  else
    # Collect every owned launcher before deleting the app. The binary may
    # precede its launchers on PATH, and multiple launchers can point to it.
    uninstall_commands="$(commands_on_path)"
    while IFS= read -r uninstall_command; do
      [ -f "$uninstall_command" ] || continue
      [ "$uninstall_command" != "$uninstall_directory/claude-rs" ] || continue
      [ "$(script_install_directory_for_command "$uninstall_command" || true)" = "$uninstall_directory" ] || continue
      rm -f "$uninstall_command" || return 1
      ok "Removed launcher $uninstall_command"
      uninstall_bin_dir="$(dirname "$uninstall_command")"
      remove_man_pages_if_owned "$uninstall_directory" "$uninstall_bin_dir"
      remove_managed_path_blocks "$uninstall_bin_dir"
    done <<EOF
$uninstall_commands
EOF
  fi

  if [ -e "$uninstall_directory" ]; then
    rm -rf "$uninstall_directory" || return 1
    ok "Removed script install directory $uninstall_directory"
  fi
)

manual_path_line() {
  path_bin_dir="${1:-$bin_dir}"
  if [ "$path_bin_dir" = "$HOME/.local/bin" ]; then
    # shellcheck disable=SC2016
    printf '%s\n' 'export PATH="$HOME/.local/bin:$PATH"'
  else
    # shellcheck disable=SC2016
    printf 'export PATH="%s:$PATH"\n' "$path_bin_dir"
  fi
}

managed_path_lines() {
  quoted_bin_dir="'$(printf '%s' "${1:-$bin_dir}" | sed "s/'/'\\\\''/g")'"
  printf "case \"\$PATH:\" in\n"
  printf '  %s:*) ;;\n' "$quoted_bin_dir"
  printf "  *) export PATH=%s:\"\$PATH\" ;;\n" "$quoted_bin_dir"
  printf 'esac\n'
}

# Resolve physical directories and final symlinks without GNU-only readlink -f.
canonical_path() (
  path="$1"
  link_count=0
  while [ -L "$path" ]; do
    [ "$link_count" -lt 40 ] || return 1
    link_count="$((link_count + 1))"
    target="$(readlink "$path")" || return 1
    case "$target" in
      /*) path="$target" ;;
      *) path="$(dirname "$path")/$target" ;;
    esac
  done
  if [ -d "$path" ]; then
    CDPATH='' cd "$path" && pwd -P
  else
    directory="$(CDPATH='' cd "$(dirname "$path")" && pwd -P)" || return 1
    printf '%s/%s\n' "${directory%/}" "$(basename "$path")"
  fi
)

path_entries() {
  # The extra delimiter preserves an empty final entry (the current directory).
  printf '%s:' "$PATH" | awk -v RS=: '{ print $0 == "" ? "." : $0 }'
}

commands_on_path() {
  path_entries | while IFS= read -r directory; do
    candidate="$directory/claude-rs"
    if [ ! -f "$candidate" ] || [ ! -x "$candidate" ]; then
      continue
    fi
    canonical_path "$candidate"
  done | awk '!seen[$0]++'
}

path_has_bin_dir() {
  expected_bin_dir="$(canonical_path "$bin_dir")" || return 1
  path_entries | (
    while IFS= read -r directory; do
      candidate_directory="$(canonical_path "$directory" 2>/dev/null)" || continue
      [ "$candidate_directory" != "$expected_bin_dir" ] || exit 0
    done
    exit 1
  )
}

path_starts_with_bin_dir() {
  expected_bin_dir="$(canonical_path "$bin_dir")" || return 1
  first_directory="${PATH%%:*}"
  first_directory="$(canonical_path "${first_directory:-.}" 2>/dev/null)" || return 1
  [ "$first_directory" = "$expected_bin_dir" ]
}

warn_path_update_skipped() {
  warn "PATH update skipped" "Add this to your shell profile:" "  $(manual_path_line)"
}

maybe_update_path() {
  if path_starts_with_bin_dir; then
    path_updated=1
    ok "PATH already points to this script install"
    return
  fi
  [ "$no_modify_path" -eq 1 ] && {
    warn_path_update_skipped
    if path_has_bin_dir; then
      warn "$bin_dir is already on PATH but not first; another claude-rs may take precedence in new shells"
    fi
    return
  }

  should_modify=0
  if [ "$yes" -eq 1 ]; then
    should_modify=1
  elif confirm_default_no "Add $(tildify "$bin_dir") to PATH in your shell profile?"; then
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
    warn_path_update_skipped
  fi
}

install_directory_overlaps() {
  first_directory="$(canonical_path "$1")" || return 1
  second_directory="$(canonical_path "$2")" || return 1
  first_directory="${first_directory%/}/"
  second_directory="${second_directory%/}/"
  case "$first_directory" in "$second_directory"*) return 0 ;; esac
  case "$second_directory" in "$first_directory"*) return 0 ;; esac
  return 1
}

script_install_directory_for_command() (
  command_path="$1"
  app_directory="$(dirname "$command_path")"
  if is_script_install_dir "$app_directory"; then
    printf '%s\n' "$app_directory"
    return 0
  fi
  # Decode a launcher as data, then compare it with the format we generate.
  # Never execute a discovered script to determine what it owns.
  [ "$(dd bs=2 count=1 2>/dev/null < "$command_path")" = '#!' ] || return 1
  contents="$(cat "$command_path")"
  exec_line="$(printf '%s\n' "$contents" | sed -n '2p')"
  case "$exec_line" in
    "exec '"*"' \"\$@\"") ;;
    *) return 1 ;;
  esac
  encoded_path="${exec_line#exec \'}"
  encoded_path="${encoded_path%\' \"\$@\"}"
  app_binary="$(printf '%s' "$encoded_path" | sed "s/'\\\\''/'/g")"
  [ "$contents" = "$(launcher_contents "$app_binary")" ] || return 1
  app_binary="$(canonical_path "$app_binary")" || return 1
  [ "$(basename "$app_binary")" = "claude-rs" ] || return 1
  app_directory="$(dirname "$app_binary")"
  is_script_install_dir "$app_directory" || return 1
  printf '%s\n' "$app_directory"
)

warn_other_claude_rs_commands() {
  expected_launcher="$(canonical_path "$bin_dir/claude-rs")" || return 0
  expected_binary="$(canonical_path "$install_dir/claude-rs")" || return 0
  command_candidates="$(commands_on_path)"
  seen_script_installs=""
  resolved="$(command -v claude-rs || true)"
  if [ -n "$resolved" ] && [ ! -f "$resolved" ]; then
    warn "A shell alias or function named claude-rs takes precedence over the installed command" \
      "Check its definition with: command -V claude-rs"
  fi
  # Keep prompts in the main shell so signal cleanup sees their saved tty state.
  while IFS= read -r candidate; do
    # A previous cleanup can invalidate another command in this snapshot.
    if [ ! -f "$candidate" ] || [ ! -x "$candidate" ]; then
      continue
    fi
    if [ "$candidate" = "$expected_launcher" ] || [ "$candidate" = "$expected_binary" ]; then
      continue
    fi
    if candidate_directory="$(script_install_directory_for_command "$candidate")"; then
      [ "$candidate_directory/claude-rs" != "$expected_binary" ] || continue
      if printf '%s\n' "$seen_script_installs" | grep -F -x -e "$candidate_directory" >/dev/null; then
        continue
      fi
      seen_script_installs="${seen_script_installs}${candidate_directory}
"
      warn "Another script installation is also on PATH: $candidate" \
        "Current installation: $expected_launcher"
      if install_directory_overlaps "$candidate_directory" "$install_dir" ||
        install_directory_overlaps "$candidate_directory" "$bin_dir"; then
        warn "Cannot remove that copy automatically because its directory overlaps this installation"
      elif [ "$yes" -eq 0 ] && [ "$update" -eq 0 ] &&
        confirm_default_no "Uninstall this other script installation at $candidate_directory?"; then
        if ! uninstall_script_install "$candidate_directory"; then
          warn "Could not uninstall $candidate_directory; this installation is ready to use"
        fi
      fi
    else
      warn "Another claude-rs is also on PATH: $candidate" \
        "Current installation: $expected_launcher" \
        "Check PATH order or remove that copy using the tool that installed it."
    fi
  done <<EOF
$command_candidates
EOF
}

cleanup() {
  exit_status="$1"
  stop_download_process
  progress_stop
  restore_tty
  if [ "$frame_open" -eq 1 ] && [ "$interrupted" -eq 1 ]; then
    # Drop the unanswered prompt's closing corner, or the echoed ^C.
    case "$prompt_state" in
      single_key) printf '\033[1A\r\033[2K' > /dev/tty 2>/dev/null || : ;;
      typed) printf '\n' > /dev/tty 2>/dev/null || : ;;
      *) [ "$progress_enabled" -eq 0 ] || printf '\r\033[2K' > /dev/tty 2>/dev/null || : ;;
    esac
    printf '%s%s  %s cancelled%s\n' "$red" "$s_bar_end" "$flow_label" "$reset" >&2
  elif [ "$frame_open" -eq 1 ] && [ "$exit_status" -ne 0 ]; then
    printf '%s%s  %s failed%s\n' "$red" "$s_bar_end" "$flow_label" "$reset" >&2
  fi
  [ -z "$lock_dir" ] || rm -rf "$lock_dir"
  [ -z "$tmpdir" ] || rm -rf "$tmpdir"
}

# Exiting from the signal traps runs cleanup once and stops the installer, so
# Ctrl-C at a prompt is never read as an answer.
on_signal() {
  interrupted=1
  exit "$1"
}
trap 'cleanup "$?"' EXIT
trap 'on_signal 129' HUP
trap 'on_signal 130' INT
trap 'on_signal 143' TERM

if [ "$uninstall" -eq 1 ]; then
  flow_label="Uninstall"
  intro "uninstaller"
  need_cmd mkdir
  need_cmd rm
  need_cmd grep
  need_cmd awk
  need_cmd mv
  uninstall_script_install
  outro "Uninstall complete"
  exit 0
fi

if [ "$update" -eq 1 ]; then
  flow_label="Update"
  intro "updater"
  is_script_install_dir "$install_dir" ||
    die "--update requires an existing claude-rs script install: $install_dir"
else
  intro "installer"
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

info "$target_label detected"
info "Install location: $(tildify "$install_dir")"
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
warn_release_advisories "$selected_version"

progress_start "Downloading release archive"
download "$base_url/SHA256SUMS" "$checksum_file" || die "could not download SHA256SUMS for $tag"
download_archive "$base_url/$archive_name" "$archive_file" ||
  die "install script is currently not available for this release"

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
install_man_pages
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
    progress_stop
    [ -z "$doctor_output" ] || printf '%s\n' "$doctor_output" | while IFS= read -r doctor_line; do
      detail "$doctor_line"
    done
    die "runtime diagnostics failed"
  }
  progress_done "Runtime diagnostics passed"
fi

# Only offer to remove an existing npm install after the script install has
# fully succeeded, so a failed install never leaves the user without claude-rs.
# Release the completed installation's lock before optional cleanup of another
# copy, which can share the same parent directory.
rmdir "$lock_dir"
lock_dir=""
if [ "$update" -eq 0 ]; then
  resolve_npm_install_choice
fi

warn_other_claude_rs_commands

if [ "$update" -eq 1 ]; then
  outro "claude-rs is updated. Start claude-rs again to use $selected_version."
elif [ "$run_after_install" -eq 1 ] || { [ "$yes" -eq 0 ] && confirm_default_no "Start claude-rs now?"; }; then
  outro "claude-rs $selected_version is installed"
  launch_installed
else
  if [ "$path_updated" -eq 1 ]; then
    note "Next steps" "Start a new shell, then run:" "  claude-rs"
  else
    note "Next steps" "Run claude-rs directly:" "  $(tildify "$install_dir/$binary_name")"
  fi
  outro "claude-rs $selected_version is installed"
fi
