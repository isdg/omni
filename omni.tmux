#!/usr/bin/env bash
# omni — tmux plugin entry point.
#
# Three fzf-backed pickers + scrollback capture, all served by the `omni` Rust
# binary (see src/). Bindings:
#     prefix b  -> fuzzy-jump to any window across all sessions   (omni windows)
#     prefix a  -> fuzzy-search the on-screen content of windows   (omni content)
#     prefix P  -> capture current pane's scrollback -> less       (omni capture)
#     prefix j  -> capture current pane's scrollback -> nvim
#     prefix J  -> same as j, but plain text (no colors)
#
# Install via TPM (~/.tmux.conf):
#     set -g @plugin 'isdg/omni'
# The binary is built by bootstrap (`cargo install --path .`); resolved from
# PATH, falling back to the crate's release build for a dev checkout.
CURRENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OMNI="$(command -v omni || echo "$CURRENT_DIR/target/release/omni")"

# Self-heal: on a fresh TPM clone the binary won't exist yet. Build it once in
# the background so `set -g @plugin 'isdg/omni'` works without extra steps; the
# bindings below already point at the release path, so they start working the
# moment the build finishes. (bootstrap also builds it deterministically.)
if [ ! -x "$OMNI" ]; then
    if command -v cargo >/dev/null 2>&1; then
        tmux run-shell -b "cd '$CURRENT_DIR' && cargo build --release >/dev/null 2>&1 && tmux display-message 'omni: built — bindings ready'"
    else
        tmux display-message 'omni: install rust/cargo to build the binary, then reload tmux'
    fi
fi

tmux bind-key b display-popup -E -w 90% -h 75% "$OMNI windows"
tmux bind-key a display-popup -E -w 90% -h 80% "$OMNI content"
tmux bind-key P run-shell "$OMNI capture --pager less"
tmux bind-key j run-shell "$OMNI capture --pager nvim"
tmux bind-key J run-shell "$OMNI capture --pager plain"
