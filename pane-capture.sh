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

# Match nvim's initial view to the pane's current scroll position. The full
# capture below is history_size + pane_height lines; in copy-mode
# #{scroll_position} is how many lines we're scrolled up from the bottom, so the
# top visible line is (history_size + 1 - scroll_position). Not in copy-mode ->
# scroll_position is empty -> jump to the end (G) as before.
read -r hist sp <<<"$(tmux display-message -p '#{history_size} #{scroll_position}')"
if [ -n "${sp:-}" ] && [ "${sp:-0}" -gt 0 ]; then
    top=$(( hist + 1 - sp ))
    [ "$top" -lt 1 ] && top=1
    pos="normal! ${top}Gzt"
else
    pos='normal! G'
fi

# plain: capture without escape sequences (drop the -e flag) for colorless text.
if [ "$mode" = plain ]; then
    tmux capture-pane -p -S - | strip_osc8 > "$f"
    tmux new-window "nvim -n -c 'set number nowrap' -c '${pos}' '$f'"
    exit 0
fi

tmux capture-pane -pe -S - | strip_osc8 > "$f"

if [ "$mode" = less ]; then
    tmux new-window "less -RN +G '$f'"
else
    tmux new-window "nvim -n -c 'set number nowrap' \
        -c 'lua pcall(function() require([[baleia]]).setup().once(0) end)' \
        -c '${pos}' '$f'"
fi
