Output "/workspace/claude-code-rust/assets/demo.gif"
Require claude-rs
Set Shell "bash"
Set FontFamily "DejaVu Sans Mono"
Set FontSize 18
Set Width 1200
Set Height 600
Set Padding 6
Set Margin 3
Set MarginFill "#8b949e"
Set Framerate 30
Set TypingSpeed 45ms
Set CursorBlink false
Set WaitTimeout 90s
Set Theme "GitHub Dark"

Env TERM "xterm-256color"

Hide
Type "clear && claude-rs --no-update-check --bridge-script /opt/claude-rs/agent-sdk/dist/bridge.js -C /workspace/claude-code-rust"
Enter
Wait+Screen /Type a message/
Show

Sleep 1s
Type "What problem does Claude Code Rust solve?"
Sleep 400ms
Enter
Wait+Screen /Agent SDK workflow/
Sleep 3s

Type "Amazing :) "
Sleep 3s

Hide
Ctrl+C
Sleep 500ms
