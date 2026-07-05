#!/usr/bin/env bash
# Fuzzy-jump to any window across all sessions. Bound to `prefix W` in
# .tmux.conf (run inside display-popup). Leaves the default prefix w
# (choose-tree) intact.
#
# Each row carries choose-tree-like info -- session:index, name, pane count +
# running command, path -- plus #{pane_title} (Claude Code sets it to the
# conversation topic), which disambiguates same-dir windows that otherwise
# all read as "claude". fzf preview ({1} = session:index) shows live
# contents; cut the first token and switch-client to it.
set -euo pipefail

tmux list-windows -a -F '#{session_name}:#{window_index}  #{window_name}  #{pane_title}  [#{window_panes}p #{pane_current_command}]  #{pane_current_path}' \
  | fzf --reverse --prompt='window> ' \
      --preview 'tmux capture-pane -ep -t {1}' --preview-window=right:55% \
  | cut -d' ' -f1 | xargs -r tmux switch-client -t
