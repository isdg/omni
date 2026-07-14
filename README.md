# omni

Three fzf-backed pickers over tmux panes/windows: jump to any window, grep
the on-screen content of every window, or open the current pane's
scrollback in nvim/less. A small Rust binary drives tmux + fzf — no server,
no config.

## Install

Via [TPM](https://github.com/tmux-plugins/tpm), add to `~/.tmux.conf`:

```tmux
set -g @plugin 'isdg/omni'
```

Then `prefix + I` to fetch it. Or load it directly:

```tmux
run-shell '~/.tmux/plugins/omni/omni.tmux'
```

Requires **fzf** on PATH and tmux 3.2+. The `omni` binary is built on first
load: `omni.tmux` runs `cargo install` in the background if the binary is
missing or its source is newer than the installed copy (so `prefix U` updates
rebuild automatically). Needs **rust/cargo** for that build.

## Keys

| Key | Does |
|---|---|
| `prefix b` | fuzzy-jump to any window across all sessions, most-recently-active first (fzf popup, live preview) |
| `prefix a` | fuzzy-search the on-screen *content* of every window, jump to the match |
| `prefix P` | capture current pane's scrollback into a new window, open in `less` |
| `prefix j` | capture current pane's scrollback into a new window, open in `nvim` (colors preserved via [baleia.nvim](https://github.com/m00qek/baleia.nvim), if installed) |
| `prefix J` | same as `j`, but strips colors — plain text in `nvim` |

`prefix w` (choose-tree) is left untouched — `b` is the fzf-powered
alternative, not a replacement.

## Files

- `omni.tmux` — entry point; resolves/builds the binary and binds the keys.
- `src/main.rs` — CLI: `omni windows` (`prefix b`), `omni content` (`prefix a`),
  `omni capture --pager nvim|less|plain` (`prefix j`/`P`/`J`).
- `src/tmux.rs` — tmux + fzf helpers.
- `src/env.rs` — reads the per-pane exported-env snapshot (see below) so a
  captured pane's venv/direnv/exported vars carry into the new window.

The env snapshot is written by a zsh `precmd` hook that stays in the shell (it
runs every prompt); `omni capture` re-applies it via `new-window -e`. Records
live at `$XDG_CACHE_HOME/omni/env/<pane-id>`, NUL-delimited `NAME=VALUE`.
