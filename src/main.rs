//! omni — fzf-backed tmux navigation + scrollback capture.
//!
//!   omni windows   fuzzy-jump to any window across all sessions   (prefix b)
//!   omni content   fuzzy-search on-screen text of every window     (prefix a)
//!                  add --history to also search scrollback         (prefix A)
//!   omni capture   capture this pane's scrollback into a new window (prefix j/J/P)
//!
//! The `.tmux` bindings are one-liners that call these; the per-prompt env
//! snapshot that `capture` consumes stays in zsh (see zsh/omni.zsh).

mod env;
mod tmux;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use regex::bytes::Regex;
use std::io::Write;

#[derive(Parser)]
#[command(name = "omni", about = "fzf-backed tmux navigation + capture")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Fuzzy-jump to any window across all sessions.
    Windows,
    /// Fuzzy-search the visible content of every window, then switch.
    Content {
        /// Also search each window's scrollback history, not just the viewport.
        #[arg(long)]
        history: bool,
    },
    /// Capture the current pane's scrollback into a new window.
    Capture {
        /// Viewer for the captured text.
        #[arg(long, value_enum, default_value_t = Pager::Nvim)]
        pager: Pager,
    },
}

#[derive(Copy, Clone, ValueEnum)]
enum Pager {
    /// nvim, colors preserved via baleia (prefix j)
    Nvim,
    /// less, colors preserved (prefix P)
    Less,
    /// nvim, no colors — plain text only (prefix J)
    Plain,
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Windows => windows(),
        Cmd::Content { history } => content(history),
        Cmd::Capture { pager } => capture(pager),
    }
}

/// Rows sorted most-recently-active first: `#{window_activity}` (epoch of last
/// activity) is prefixed as a numeric sort key, then stripped before fzf.
/// `--tiebreak=index` keeps that recency order when match scores tie.
fn windows() -> Result<()> {
    let raw = tmux::query([
        "list-windows", "-a", "-F",
        "#{window_activity} #{session_name}:#{window_index}  #{window_name}  \
         #{pane_title}  [#{window_panes}p #{pane_current_command}]  #{pane_current_path}",
    ])?;
    let input = strip_activity_sort(&raw);

    if let Some(sel) = tmux::pick(
        &[
            "--reverse",
            "--tiebreak=index",
            "--prompt=window> ",
            "--preview=tmux capture-pane -ep -t {1}",
            "--preview-window=down:55%",
        ],
        input,
    )? {
        // {1} in fzf = first whitespace field = session:index.
        if let Some(target) = sel.split_whitespace().next() {
            tmux::run(["switch-client", "-t", target])?;
        }
    }
    Ok(())
}

/// Each capture line becomes `session:index<TAB>lineno<TAB>content` so fzf can
/// match on content (`--with-nth=3..` hides the target + lineno) while still
/// recovering the target from field 1. Preview centers the matched line ({2}).
///
/// `history` extends the capture back through scrollback (`-S -`); the preview
/// uses the same range so its line numbers stay aligned with {2}.
fn content(history: bool) -> Result<()> {
    let ws = tmux::query([
        "list-windows", "-a", "-F", "#{window_activity} #{session_name}:#{window_index}",
    ])?;
    // strip_activity_sort yields recency order with empty lines already dropped.
    let ordered = strip_activity_sort(&ws);

    // With history, start capture at the beginning of scrollback (-S -); the
    // preview command below must match so {2} lands on the right line.
    let cap: &[&str] = if history {
        &["capture-pane", "-ep", "-S", "-", "-t"]
    } else {
        &["capture-pane", "-ep", "-t"]
    };
    let preview = if history {
        "--preview=tmux capture-pane -ep -S - -t {1} | awk -v n={2} 'NR==n{print \"\\033[7m\" $0 \"\\033[0m\"; next}{print}'"
    } else {
        "--preview=tmux capture-pane -ep -t {1} | awk -v n={2} 'NR==n{print \"\\033[7m\" $0 \"\\033[0m\"; next}{print}'"
    };

    let mut input = String::new();
    for t in ordered.lines() {
        let args: Vec<&str> = cap.iter().copied().chain([t]).collect();
        let pane = tmux::query(args).unwrap_or_default();
        for (i, line) in pane.lines().enumerate() {
            input.push_str(&format!("{t}\t{}\t{line}\n", i + 1));
        }
    }

    if let Some(sel) = tmux::pick(
        &[
            "--reverse",
            "--tiebreak=index",
            "--delimiter=\t",
            "--with-nth=3..",
            "--prompt=content> ",
            preview,
            "--preview-window=down:55%:+{2}-/2",
        ],
        input,
    )? {
        if let Some(target) = sel.split('\t').next() {
            tmux::run(["switch-client", "-t", target])?;
        }
    }
    Ok(())
}

/// Capture scrollback, strip OSC-8 hyperlinks, re-apply the pane's exported env,
/// and open the result in nvim/less at the pane's current scroll position.
fn capture(pager: Pager) -> Result<()> {
    let disp = tmux::query(["display-message", "-p", "#{history_size} #{scroll_position}"])?;
    let mut it = disp.split_whitespace();
    let hist: i64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let scroll: Option<i64> = it.next().and_then(|s| s.parse().ok());

    // In copy-mode, scroll_position is lines-from-bottom; the top visible line is
    // (history_size + 1 - scroll_position). Otherwise jump to the end.
    let pos = match scroll {
        Some(sp) if sp > 0 => format!("normal! {}Gzt", (hist + 1 - sp).max(1)),
        _ => "normal! G".to_string(),
    };

    let cwd = tmux::query(["display-message", "-p", "#{pane_current_path}"])?;
    let pane_id = tmux::query(["display-message", "-p", "#{pane_id}"])?;

    // plain mode drops color escapes (-p instead of -pe).
    let cap_args: &[&str] = match pager {
        Pager::Plain => &["capture-pane", "-p", "-S", "-"],
        _ => &["capture-pane", "-pe", "-S", "-"],
    };
    let raw = tmux::query_bytes(cap_args)?;
    let cleaned = strip_osc8(&raw);

    let path = std::env::temp_dir().join(format!("tmux-pane.{}", std::process::id()));
    std::fs::File::create(&path)?.write_all(&cleaned)?;
    let path = path.to_string_lossy().into_owned();

    let shell = match pager {
        Pager::Plain => format!("nvim -n -c 'set number nowrap' -c '{pos}' '{path}'"),
        Pager::Less => format!("less -RN +G '{path}'"),
        Pager::Nvim => format!(
            "nvim -n -c 'set number nowrap' \
             -c 'lua pcall(function() require([[baleia]]).setup().once(0) end)' \
             -c '{pos}' '{path}'"
        ),
    };

    let mut args: Vec<String> = vec!["new-window".into(), "-c".into(), cwd];
    for kv in env::records(&pane_id) {
        args.push("-e".into());
        args.push(kv);
    }
    args.push(shell);
    tmux::run(&args)
}

/// Split lines of `"<activity> <rest>"`, sort by activity descending, and return
/// the `<rest>` lines joined — the recency-ordered input for fzf.
fn strip_activity_sort(raw: &str) -> String {
    let mut rows: Vec<(i64, &str)> = raw
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let (a, rest) = l.split_once(' ').unwrap_or(("0", l));
            (a.parse::<i64>().unwrap_or(0), rest)
        })
        .collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    rows.into_iter()
        .map(|(_, r)| r)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Remove OSC-8 hyperlink sequences (ESC ]8; … BEL|ESC\) so viewers that only
/// understand SGR color escapes don't render literal "]8;…" artifacts.
fn strip_osc8(bytes: &[u8]) -> Vec<u8> {
    let re = Regex::new(r"\x1b\]8;.*?(?:\x07|\x1b\\)").unwrap();
    re.replace_all(bytes, &b""[..]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc8_stripped_both_terminators_sgr_kept() {
        // BEL-terminated and ESC\-terminated hyperlinks, with an SGR color that
        // must survive (viewers understand SGR, not OSC-8).
        let input = b"\x1b]8;;file:///a\x07link\x1b]8;;\x07 \x1b[31mred\x1b[0m \x1b]8;;http://x\x1b\\y\x1b]8;;\x1b\\";
        let out = strip_osc8(input);
        assert_eq!(out, b"link \x1b[31mred\x1b[0m y".to_vec());
    }

    #[test]
    fn osc8_noop_when_absent() {
        assert_eq!(strip_osc8(b"plain text"), b"plain text".to_vec());
    }

    #[test]
    fn activity_sort_is_descending_and_strips_key() {
        let raw = "100 alpha\n300 gamma\n200 beta\n";
        assert_eq!(strip_activity_sort(raw), "gamma\nbeta\nalpha");
    }
}
