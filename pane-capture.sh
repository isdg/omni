#!/usr/bin/env bash
# Capture the current pane's scrollback (with colors) into a new window, opened
# in nvim (default) or less. Strips OSC 8 hyperlink sequences — e.g. Claude
# Code's clickable file paths — which would otherwise show as literal "]8;…"
# artifacts, since baleia (nvim) and less only handle SGR color escapes.
#   pane-capture.sh        -> nvim  (bound to prefix j)
#   pane-capture.sh less   -> less  (bound to prefix P)
set -euo pipefail

mode="${1:-nvim}"
f="$(mktemp -t tmux-pane.XXXXXX)"
tmux capture-pane -pe -S - \
  | perl -pe 's/\e\]8;.*?(?:\a|\e\\)//g' \
  > "$f"

if [ "$mode" = less ]; then
    tmux new-window "less -RN +G '$f'"
else
    tmux new-window "nvim -n -c 'set number nowrap' \
        -c 'lua pcall(function() require([[baleia]]).setup().once(0) end)' \
        -c 'normal G' '$f'"
fi
