//! Read back the pane shell's exported environment that the zsh precmd hook
//! (`~/.dotfiles/zsh/omni.zsh`) snapshots per prompt, so `omni capture` can
//! re-apply it to the new window via `new-window -e`.
//!
//! Contract with the writer (keep in sync): records live at
//! `$XDG_CACHE_HOME/omni/env/<pane-id-without-%>`, NUL-delimited `NAME=VALUE`.

/// Vars tied to the *source* pane/terminal — inheriting them would confuse the
/// new window, so they're never carried over.
const DENY: &[&str] = &[
    "TMUX",
    "TMUX_PANE",
    "TMUX_TMPDIR",
    "TERM",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "WINDOWID",
    "SHLVL",
    "PWD",
    "OLDPWD",
    "COLUMNS",
    "LINES",
    "_",
];

fn env_dir() -> String {
    let base = std::env::var("XDG_CACHE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}/.cache", std::env::var("HOME").unwrap_or_default()));
    format!("{base}/omni/env")
}

/// Returns `NAME=VALUE` records (denylisted names dropped) for the given pane id
/// (e.g. `%3`). Empty if no snapshot exists yet.
pub fn records(pane_id: &str) -> Vec<String> {
    let file = format!("{}/{}", env_dir(), pane_id.trim_start_matches('%'));
    match std::fs::read(&file) {
        Ok(bytes) => parse(&bytes),
        Err(_) => Vec::new(),
    }
}

/// Parse NUL-delimited `NAME=VALUE` records, dropping empty chunks and any name
/// on the denylist. Pure (no IO) so it can be unit-tested.
fn parse(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|&b| b == 0)
        .filter(|chunk| !chunk.is_empty())
        .filter_map(|chunk| {
            let kv = String::from_utf8_lossy(chunk);
            let name = kv.split('=').next().unwrap_or("");
            if name.is_empty() || DENY.contains(&name) {
                None
            } else {
                Some(kv.into_owned())
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_normal_vars_drops_denylisted() {
        let blob = b"PATH=/usr/bin\0TMUX=/tmp/x,1,0\0VIRTUAL_ENV=/venv\0PWD=/here\0";
        assert_eq!(parse(blob), vec!["PATH=/usr/bin", "VIRTUAL_ENV=/venv"]);
    }

    #[test]
    fn values_with_spaces_and_equals_round_trip() {
        let blob = b"MSG=a b c\0EXPR=x=y=z\0";
        assert_eq!(parse(blob), vec!["MSG=a b c", "EXPR=x=y=z"]);
    }

    #[test]
    fn empty_and_trailing_nul_are_ignored() {
        assert!(parse(b"").is_empty());
        assert_eq!(parse(b"A=1\0"), vec!["A=1"]);
    }
}
