# Recordings

Records `assets/demo.gif` for the README, using [VHS](https://github.com/charmbracelet/vhs) in Docker.

## Usage

Needs a running Docker daemon. Run from the repository root:

```bash
docker compose -f recordings/compose.yml build
docker compose -f recordings/compose.yml run --rm demo
```

The build context is the repository root, so the current working tree is recorded. The GIF is written straight to `assets/demo.gif` through the bind mount.

## Files

- `demo.tape` — the VHS script: terminal size, theme, and the typed input.
- `compose.yml` — mounts that keep runs reproducible: scripted `CLAUDE.md`, `main` as `.git/HEAD`, pre-accepted trust dialog, and the host `~/.claude/.credentials.json` for auth.
- `Dockerfile` — builds `claude-rs`, the bridge, and the Bun runtime on top of the VHS image.
- `inspection/` — screenshots kept from earlier tuning passes.

## Notes

- The Rust stage pins `rust:1.88-bookworm` and ignores `rust-toolchain.toml`, so the recording builds at MSRV, not at the pinned 1.89.
- `demo.tape` waits on the `Type a message` placeholder and on `Agent SDK workflow` from the scripted answer in `CLAUDE.md`; changing either text breaks the recording.
- Check the result visually after UI changes — `Set Width`/`Set Height` in the tape may need adjusting.
