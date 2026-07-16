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

use anyhow::{Context, Result};
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
    Windows {
        /// Print the window list to stdout instead of launching fzf. Used by the
        /// picker's ctrl-x binding to refresh the list after killing a window.
        #[arg(long)]
        list: bool,
    },
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
        /// Capture this window/pane target instead of the current pane, first
        /// switching to it. Used by the picker's ctrl-j binding.
        #[arg(long)]
        target: Option<String>,
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
        Cmd::Windows { list } => windows(list),
        Cmd::Content { history } => content(history),
        Cmd::Capture { pager, target } => capture(pager, target),
    }
}

/// Rows sorted most-recently-active first: `#{window_activity}` (epoch of last
/// activity) is prefixed as a numeric sort key, then stripped before fzf.
/// `--tiebreak=index` keeps that recency order when match scores tie.
fn windows(list: bool) -> Result<()> {
    let input = window_list()?;

    // ctrl-x kills the highlighted window, then reloads via `omni windows --list`
    // so the row disappears without leaving the picker. `--list` prints exactly
    // the same rows this fn feeds fzf, so ordering/columns stay identical.
    if list {
        println!("{input}");
        return Ok(());
    }

    let exe = std::env::current_exe()
        .context("cannot resolve own path")?
        .to_string_lossy()
        .into_owned();
    // {{1}} -> literal {1} for fzf = first whitespace field = session:index.
    let kill = format!("--bind=ctrl-x:execute-silent(tmux kill-window -t {{1}})+reload({exe} windows --list)");
    // ctrl-j captures the highlighted window's pane just like prefix j: switch to
    // it, then open its scrollback in nvim. +abort leaves the picker afterward.
    let capture = format!("--bind=ctrl-j:execute-silent({exe} capture --pager nvim --target {{1}})+abort");

    if let Some(sel) = tmux::pick(
        &[
            "--reverse",
            "--tiebreak=index",
            "--prompt=window> ",
            "--header=enter jump · ctrl-x kill · ctrl-j capture",
            "--preview=tmux capture-pane -ep -t {1} | tail -n \"${FZF_PREVIEW_LINES:-40}\"",
            "--preview-window=down:55%",
            &kill,
            &capture,
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

/// The recency-ordered window rows fed to the picker (and re-emitted by
/// `windows --list` after a ctrl-x kill). Field 1 is `session:index`.
fn window_list() -> Result<String> {
    let raw = tmux::query([
        "list-windows", "-a", "-F",
        "#{window_activity} #{session_name}:#{window_index}  #{window_name}  \
         #{pane_title}  [#{window_panes}p #{pane_current_command}]  #{pane_current_path}",
    ])?;
    Ok(strip_activity_sort(&raw))
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
///
/// `target` (ctrl-j in the window picker) captures that window's active pane and
/// switches to it first, so the capture opens in its session exactly as pressing
/// prefix j after jumping there would. Without it, the current pane is used.
fn capture(pager: Pager, target: Option<String>) -> Result<()> {
    // Enter the picked window first; the new capture window then lands in its
    // session and reads its scrollback below via the same `-t` target.
    if let Some(t) = &target {
        tmux::run(["switch-client", "-t", t])?;
    }
    // `-t <target>` steers every read at the picked pane; empty = current pane.
    let tflag: &[&str] = match &target {
        Some(t) => &["-t", t.as_str()],
        None => &[],
    };

    let dm = |fmt: &str| {
        let args: Vec<&str> = ["display-message", "-p"]
            .into_iter()
            .chain(tflag.iter().copied())
            .chain([fmt])
            .collect();
        tmux::query(args)
    };

    let disp = dm("#{history_size} #{scroll_position}")?;
    let mut it = disp.split_whitespace();
    let hist: i64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let scroll: Option<i64> = it.next().and_then(|s| s.parse().ok());

    // In copy-mode, scroll_position is lines-from-bottom; the top visible line is
    // (history_size + 1 - scroll_position). Otherwise jump to the end.
    let pos = match scroll {
        Some(sp) if sp > 0 => format!("normal! {}Gzt", (hist + 1 - sp).max(1)),
        _ => "normal! G".to_string(),
    };

    let cwd = dm("#{pane_current_path}")?;
    let pane_id = dm("#{pane_id}")?;

    // plain mode drops color escapes (-p instead of -pe).
    let cap_flag = match pager {
        Pager::Plain => "-p",
        _ => "-pe",
    };
    let cap_args: Vec<&str> = ["capture-pane", cap_flag, "-S", "-"]
        .into_iter()
        .chain(tflag.iter().copied())
        .collect();
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
