#!/usr/bin/env bash
# omni — tmux plugin entry point.
#
# Three fzf-backed pickers + scrollback capture, all served by the `omni` Rust
# binary (see src/). Bindings:
#     prefix b  -> fuzzy-jump to any window across all sessions   (omni windows)
#     prefix a  -> fuzzy-search the on-screen content of windows   (omni content)
#     prefix A  -> same, but also search each window's scrollback   (omni content --history)
#     prefix P  -> capture current pane's scrollback -> less       (omni capture)
#     prefix j  -> capture current pane's scrollback -> nvim
#     prefix J  -> same as j, but plain text (no colors)
#
# Install via TPM (~/.tmux.conf):
#     set -g @plugin 'isdg/omni'
CURRENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OMNI="$(command -v omni || echo "$HOME/.cargo/bin/omni")"

# Self-heal build. Rebuild when the binary is MISSING (fresh clone) or STALE —
# i.e. any source file is newer than the installed binary, which is exactly what
# `prefix U` (TPM update) produces after it pulls new source. `cargo install
# --force` reinstalls to ~/.cargo/bin so the update actually takes effect. Runs
# in the background so tmux start never blocks; bindings work once it finishes.
if [ ! -x "$OMNI" ] || \
   [ -n "$(find "$CURRENT_DIR/src" "$CURRENT_DIR/Cargo.toml" -newer "$OMNI" -print -quit 2>/dev/null)" ]; then
    if command -v cargo >/dev/null 2>&1; then
        tmux run-shell -b "cd '$CURRENT_DIR' && cargo install --path . --force >/dev/null 2>&1 && tmux display-message 'omni: (re)built — bindings ready'"
    else
        tmux display-message 'omni: install rust/cargo to build the binary, then reload tmux'
    fi
fi

tmux bind-key b display-popup -E -w 90% -h 75% "$OMNI windows"
tmux bind-key a display-popup -E -w 90% -h 80% "$OMNI content"
tmux bind-key A display-popup -E -w 90% -h 80% "$OMNI content --history"
tmux bind-key P run-shell "$OMNI capture --pager less"
tmux bind-key j run-shell "$OMNI capture --pager nvim"
tmux bind-key J run-shell "$OMNI capture --pager plain"
