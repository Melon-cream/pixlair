# Pixlair

Pixlair is a small companion TUI that sits beside an interactive `codex` session. It exists to add a bit of charm to an otherwise plain terminal, not to improve productivity.
At the moment, it only supports Codex.

Japanese version: [README.md]()

## Features

- Avatar-only sidecar for interactive `codex`
- Built for `zellij` pane layouts
- Expression changes driven by Codex session events
- Compact Unicode avatar optimized for terminal display
- `--demo` mode for local preview and tuning
- External event feed support for manual testing

## Requirements

- Rust toolchain
- Unix-like terminal
- `zellij`
- `codex` available in `PATH`
- Interactive TTY

## Build

```bash
cargo build
```

## Usage

### Run with Codex

Start Pixlair from inside an attached `zellij` session:

```bash
cargo run -- --codex
```

Everything after `--codex` is passed directly to `codex`:

```bash
cargo run -- --codex resume --last
cargo run -- --codex --no-alt-screen
```

Behavior:

- Pixlair opens a side pane on the left
- `codex` runs in the current pane
- Pixlair watches the Codex session file under `~/.codex/sessions/...`
- The avatar updates as Codex thinks, uses tools, replies, or completes a task

### Demo Mode

```bash
cargo run -- --demo
```

This starts Pixlair without Codex and cycles through expressions automatically.

### No Arguments

Running without arguments shows help:

```bash
cargo run
```

## Status Mapping

Pixlair maps Codex session activity to avatar states such as:

- `thinking`
- `tool`
- `working`
- `success`
- `error`
- `input`

The exact mapping is based on session JSONL events such as reasoning, tool calls, assistant messages, and task completion.

## Controls

When running in the full TUI:

- `1` idle
- `2` input
- `3` thinking
- `4` working
- `5` success
- `6` error
- `7` sleeping
- `8` tool
- `d` toggle demo mode
- `h` toggle help
- `q` quit

## External Event Feed

For manual testing, Pixlair can also follow an external event file:

```bash
touch /tmp/pixlair.events
cargo run -- --events /tmp/pixlair.events
```

Example events:

```text
state=thinking message="Planning the change"
state=success badge=ok message="Patch applied"
```

```json
{"state":"tool","tool":"wrench","message":"Running formatter","badge":"run"}
```

## Limitations

- Intended for use inside an attached `zellij` session
- Terminal Unicode width handling may vary by terminal emulator
- Uses lightweight ANSI terminal control rather than an image renderer
- Interactive Codex tracking depends on session files being available under `~/.codex/sessions`

## License

MIT License. See `LICENSE.md`.
