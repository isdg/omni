#!/usr/bin/env bash
# Fuzzy-jump to any window across all sessions. Bound to `prefix b` by
# omni.tmux (run inside display-popup). Leaves the default prefix w
# (choose-tree) intact.
#
# Each row carries choose-tree-like info -- session:index, name, pane count +
# running command, path -- plus #{pane_title} (Claude Code sets it to the
# conversation topic), which disambiguates same-dir windows that otherwise
# all read as "claude". fzf preview ({1} = session:index) shows live
# contents; cut the first token and switch-client to it.
#
# Rows are sorted most-recently-active first (#{window_activity} is the epoch
# of each window's last activity): we prefix it as a sort key, sort -rn, then
# strip it back off before fzf. --tiebreak=index keeps that recency order as
# the tiebreaker when fuzzy-match scores are equal.
set -euo pipefail

tmux list-windows -a -F '#{window_activity} #{session_name}:#{window_index}  #{window_name}  #{pane_title}  [#{window_panes}p #{pane_current_command}]  #{pane_current_path}' \
  | sort -rn | cut -d' ' -f2- \
  | fzf --reverse --tiebreak=index --prompt='window> ' \
      --preview 'tmux capture-pane -ep -t {1}' --preview-window=down:55% \
  | cut -d' ' -f1 | xargs -r tmux switch-client -t
