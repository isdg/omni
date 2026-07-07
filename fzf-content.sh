#!/usr/bin/env bash
# Fuzzy-search the visible content of every tmux window, then switch to the
# matching window. Bound to `prefix a` by omni.tmux (run inside display-popup).
#
# Each capture-pane line is prefixed with "session:index<TAB>lineno<TAB>" so fzf
# can match on the content (--with-nth=3.. hides the target + lineno columns)
# while still letting us recover the target from the selected line. The preview
# shows the live pane, scrolled so the matched line ({2}) is centered
# (--preview-window +{2}-/2) and reverse-video highlighted.
#
# Visible screen only by default (fast). For scrollback too, add `-S -500` (or
# `-S -` for full history) to the capture-pane call below.
set -euo pipefail

sel=$(
  tmux list-windows -a -F '#{session_name}:#{window_index}' | while read -r t; do
    tmux capture-pane -ep -t "$t" 2>/dev/null | awk -v t="$t" '{print t"\t"NR"\t"$0}'
  done | fzf --reverse --delimiter='	' --with-nth=3.. \
            --prompt='content> ' \
            --preview 'tmux capture-pane -ep -t {1} | awk -v n={2} '\''NR==n{print "\033[7m" $0 "\033[0m"; next}{print}'\''' \
            --preview-window=down:55%:+{2}-/2
) || exit 0

[ -n "$sel" ] && tmux switch-client -t "${sel%%	*}"
