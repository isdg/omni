#!/usr/bin/env bash
# Fuzzy-search the visible content of every tmux window, then switch to the
# matching window. Bound to `prefix a` in .tmux.conf (run inside display-popup).
#
# Each capture-pane line is prefixed with "session:index<TAB>" so fzf can match
# on the content (--with-nth=2.. hides the target column) while still letting us
# recover the target from the selected line. Preview shows the live pane.
#
# Visible screen only by default (fast). For scrollback too, add `-S -500` (or
# `-S -` for full history) to the capture-pane call below.
set -euo pipefail

sel=$(
  tmux list-windows -a -F '#{session_name}:#{window_index}' | while read -r t; do
    tmux capture-pane -ep -t "$t" 2>/dev/null | sed "s|^|${t}	|"
  done | fzf --reverse --delimiter='	' --with-nth=2.. \
            --prompt='content> ' \
            --preview 'tmux capture-pane -ep -t {1}' \
            --preview-window=right:55%
) || exit 0

[ -n "$sel" ] && tmux switch-client -t "${sel%%	*}"
