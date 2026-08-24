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

omni binds **no keys of its own** — it resolves and builds the binary, and the
key map stays in your `~/.tmux.conf`, where one file owns every binding and a
plugin update can never move a key underneath you. Paste this after the TPM
`run` line and adjust to taste:

```tmux
run-shell 'OMNI="$(command -v omni || echo "$HOME/.cargo/bin/omni")"; \
  tmux bind-key b display-popup -E -w 90% -h 100% "$OMNI windows"; \
  tmux bind-key a display-popup -E -w 90% -h 100% "$OMNI content"; \
  tmux bind-key A display-popup -E -w 90% -h 100% "$OMNI content --history"; \
  tmux bind-key P run-shell "$OMNI capture --pager less"; \
  tmux bind-key j run-shell "$OMNI capture --pager nvim"; \
  tmux bind-key J run-shell "$OMNI capture --pager plain"'
```

| Key | Does |
|---|---|
| `prefix b` | fuzzy-jump to any window across all sessions, most-recently-active first; `ctrl-g` toggles to session order (fzf popup, live preview) |
| `prefix a` | fuzzy-search the on-screen *content* of every window, jump to the match |
| `prefix A` | same as `a`, but searches each window's scrollback too |
| `prefix P` | capture current pane's scrollback into a new window, open in `less` |
| `prefix j` | capture current pane's scrollback into a new window, open in `nvim` (colors preserved via [baleia.nvim](https://github.com/m00qek/baleia.nvim), if installed) |
| `prefix J` | same as `j`, but strips colors — plain text in `nvim` |

### Window order

The window picker opens **recency first** and `ctrl-g` toggles to **session
order** (tmux's own: session name, then window index). The header names the
active one, and the choice persists.

Recency sorts on `#{window_activity}`, then on the session's last-attached time.
The second key is not decoration: any pane running an animated TUI — a Claude
Code spinner, k9s — restamps its activity every second, so a dozen windows tie on
the first key and a stable sort quietly degenerates into tmux's listing order.
Breaking the tie by the session you were last in is what makes recency mean
anything on a busy server.

Both pickers share one layout — list on top, the input line under it, preview
below, the shape of nvim's buffer picker — with fzf's chrome stripped to a single
pointer on the current row. It lives in `tmux::pick`, so every picker wears it and
a new one gets it for free.

`prefix w` (choose-tree) is left untouched — `b` is the fzf-powered
alternative, not a replacement.

## Files

- `omni.tmux` — entry point; resolves and builds the binary. Binds nothing.
- `src/main.rs` — CLI: `omni windows`, `omni content` (`--history` to include
  scrollback), `omni capture --pager nvim|less|plain`.
- `src/tmux.rs` — tmux + fzf helpers.
- `src/env.rs` — reads the per-pane exported-env snapshot (see below) so a
  captured pane's venv/direnv/exported vars carry into the new window.

The env snapshot is written by a zsh `precmd` hook that stays in the shell (it
runs every prompt); `omni capture` re-applies it via `new-window -e`. Records
live at `$XDG_CACHE_HOME/omni/env/<pane-id>`, NUL-delimited `NAME=VALUE`.
