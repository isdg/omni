# omni

Three fzf-backed pickers over tmux panes/windows: jump to any window, grep the
content of this session's windows (screen and scrollback), or open the current
pane's scrollback in nvim/less. A small Rust binary drives tmux + fzf — no
server, no config.

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
| `prefix a` | fuzzy-search the *content* of every window **in this session** — screen and scrollback — and jump to the matched **line** |
| `prefix A` | same as `a`, over **every session** rather than just this one |
| `prefix P` | capture current pane's scrollback into a new window, open in `less` |
| `prefix j` | capture current pane's scrollback into a new window, open in `nvim` (colors preserved via [baleia.nvim](https://github.com/m00qek/baleia.nvim), if installed) |
| `prefix J` | same as `j`, but strips colors — plain text in `nvim` |

### Scope is the only difference between the two keys

Both read the **full scrollback** of every window they cover. `prefix a` covers
this session, `prefix A` (`--all`) covers every session — and the picker's border
names the session when that is the scope, so it is never a guess.

`a` used to search the viewport alone, which worked while it also spanned the
server. Once it narrowed to one session the two limits multiplied and it stopped
being able to find anything: four windows' visible screens came to **97 rows**,
against **105k** for `A`. A session is a workspace — one repo, one worktree, one
machine — so the useful question there is not "what is on screen in this session"
but "what have I seen in this session", which is `A`'s question over the part of
the server you are working in. Same capture range, one knob. `prefix b` is the
other cross-session door.

The session is passed to `list-windows` explicitly, via `#{client_session}`. A
bare `list-windows` resolves "current session" through `$TMUX_PANE`, which a
popup inherits from the client's environment and which can point into a
*different* session — silently searching the wrong one.

`--history` still works as an alias for `--all`: a running tmux server holds its
bindings in memory, so a `prefix A` bound before the rename keeps working until
the config is reloaded.

### Content search lands on the line

Enter does not just switch to the window — it puts that window's pane in
copy-mode with the cursor on the line you picked. Only the cursor is moved:
nothing is selected, so the next motion just moves, and `v` starts a selection
when you actually want one. `y` copies it, `q` leaves copy-mode.

The hit is centred, with the lines either side of it for context. A hit that was
already on screen gets centred too, which is the one thing the viewport-only mode
did better — it left the view alone so the line stayed exactly where the picker
showed it.

**Blank lines are not rows.** A viewport is always `pane_height` lines, so every
window contributes its unused tail, and a prompt padded with a blank line adds
more in the middle — 587 of 1531 rows on a 32-window server. Nothing can match
them and there is nowhere useful to jump, so they are dropped from the list; the
line numbers stay the line's place in the *capture*, so the preview and the jump
still land exactly. The preview keeps its blanks: it is a picture of the pane.

**The window you are on comes first.** Content search is fed in recency order,
but `#{window_activity}` measures output, not attention: a pane running Claude
Code or k9s restamps it every second while the window you are sitting in — nvim,
a shell at a prompt — holds a constant one. On a 32-window server the current
window came *sixth*, behind five chattier ones, and since `--tiebreak=index`
settles a score tie in favour of the earlier row, searching for text you could
see on screen jumped you to an identical line somewhere else. So the current
window is hoisted to the front and the rest keep their recency order.

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
