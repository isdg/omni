# omni

Three fzf-backed pickers over tmux panes/windows: jump to any window, grep
the on-screen content of every window, or open the current pane's
scrollback in nvim/less. Pure tmux + fzf — no server, no config.

## Install

Via [TPM](https://github.com/tmux-plugins/tpm), add to `~/.tmux.conf`:

```tmux
set -g @plugin 'isdg/omni'
```

Then `prefix + I` to fetch it. Or load it directly:

```tmux
run-shell '~/.tmux/plugins/omni/omni.tmux'
```

Requires **fzf** on PATH and tmux 3.2+.

## Keys

| Key | Does |
|---|---|
| `prefix W` | fuzzy-jump to any window across all sessions (fzf popup, live preview) |
| `prefix a` | fuzzy-search the on-screen *content* of every window, jump to the match |
| `prefix P` | capture current pane's scrollback into a new window, open in `less` |
| `prefix j` | capture current pane's scrollback into a new window, open in `nvim` (colors preserved via [baleia.nvim](https://github.com/m00qek/baleia.nvim), if installed) |

`prefix w` (choose-tree) is left untouched — `W` is the fzf-powered
alternative, not a replacement.

## Files

- `omni.tmux` — entry point; binds the four keys above.
- `window-picker.sh` — `prefix W`.
- `fzf-content.sh` — `prefix a`.
- `pane-capture.sh` — `prefix P` / `prefix j` (mode selected by argv[1]).
