#!/usr/bin/env bash
# omni — tmux plugin entry point.
#
# Three fzf-backed pickers over your panes/windows, bound to:
#     prefix W  -> fuzzy-jump to any window across all sessions
#     prefix a  -> fuzzy-search the on-screen content of every window
#     prefix P  -> capture current pane's scrollback, open in less
#     prefix j  -> capture current pane's scrollback, open in nvim
#     prefix J  -> same as j, but without colors (plain text)
#
# Install via TPM (~/.tmux.conf):
#     set -g @plugin 'isdg/omni'
# or load directly:
#     run-shell '~/.tmux/plugins/omni/omni.tmux'
CURRENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

tmux bind-key W display-popup -E -w 90% -h 75% "$CURRENT_DIR/window-picker.sh"
tmux bind-key a display-popup -E -w 90% -h 80% "$CURRENT_DIR/fzf-content.sh"
tmux bind-key P run-shell "$CURRENT_DIR/pane-capture.sh less"
tmux bind-key j run-shell "$CURRENT_DIR/pane-capture.sh nvim"
tmux bind-key J run-shell "$CURRENT_DIR/pane-capture.sh plain"
