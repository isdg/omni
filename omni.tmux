#!/usr/bin/env bash
# omni — tmux plugin entry point.
#
# Three fzf-backed pickers + scrollback capture, all served by the `omni` Rust
# binary (see src/).
#
# This entry point binds no keys. It resolves and self-heals the binary and
# leaves the key map to your own tmux.conf, so one file owns every binding and a
# plugin update can never move a key underneath you. README.md carries a block
# of suggested bindings to paste.
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
        tmux run-shell -b "cd '$CURRENT_DIR' && cargo install --path . --force >/dev/null 2>&1 && tmux display-message 'omni: (re)built — ready'"
    else
        tmux display-message 'omni: install rust/cargo to build the binary, then reload tmux'
    fi
fi
