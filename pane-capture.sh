#!/usr/bin/env bash
# Capture the current pane's scrollback into a new window, opened in nvim
# (default), less, or plain. Filters the capture (below) to strip OSC 8
# hyperlinks so they don't show as literal "]8;…" artifacts.
#   pane-capture.sh        -> nvim, colors preserved (bound to prefix j)
#   pane-capture.sh less   -> less, colors preserved (bound to prefix P)
#   pane-capture.sh plain  -> nvim, no colors: plain text only (bound to prefix J)
set -euo pipefail

# Remove OSC 8 hyperlink sequences (ESC ]8;… ESC\ … ESC ]8;; ESC\) — e.g. Claude
# Code's clickable file paths, which otherwise show as literal "]8;…" text since
# baleia (nvim) and less only handle SGR color escapes, not OSC 8.
strip_osc8() {
    perl -pe 's/\e\]8;.*?(?:\a|\e\\)//g'
}

mode="${1:-nvim}"
f="$(mktemp -t tmux-pane.XXXXXX)"

# plain: capture without escape sequences (drop the -e flag) for colorless text.
if [ "$mode" = plain ]; then
    tmux capture-pane -p -S - | strip_osc8 > "$f"
    tmux new-window "nvim -n -c 'set number nowrap' -c 'normal G' '$f'"
    exit 0
fi

tmux capture-pane -pe -S - | strip_osc8 > "$f"

if [ "$mode" = less ]; then
    tmux new-window "less -RN +G '$f'"
else
    tmux new-window "nvim -n -c 'set number nowrap' \
        -c 'lua pcall(function() require([[baleia]]).setup().once(0) end)' \
        -c 'normal G' '$f'"
fi
