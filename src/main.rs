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
    /// Show or flip the window picker's order: recency (default) or session.
    /// The choice persists, so ctrl-g's reload comes back in the new order.
    Sort {
        /// Flip to the other order and print the new one.
        #[arg(long)]
        toggle: bool,
        /// Print the picker's full header line instead of just the mode.
        #[arg(long)]
        header: bool,
    },
    /// Fuzzy-search the visible content of every window, then switch.
    Content {
        /// Also search each window's scrollback history, not just the viewport.
        #[arg(long)]
        history: bool,
    },
    /// Render a pane for the picker preview (bottom-aligned shell / top TUI).
    Peek {
        /// The `session:index` window target (fzf field 1) to preview.
        target: String,
    },
    /// Kill a window, warning instead when it's the last one in its session.
    Kill {
        /// The `session:index` window target (fzf field 1) to kill.
        target: String,
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
        Cmd::Sort { toggle, header } => {
            let mode = if toggle { toggle_order() } else { order_mode() };
            if header {
                println!("{}", windows_header());
            } else {
                println!("{}", order_label(mode));
            }
            Ok(())
        }
        Cmd::Content { history } => content(history),
        Cmd::Peek { target } => peek(&target),
        Cmd::Kill { target } => kill_window(&target),
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
    // `omni kill` guards the last-window case (would kill the session) with a
    // warning popup instead; the reload then refreshes the (maybe unchanged) list.
    let kill = format!("--bind=ctrl-x:execute-silent({exe} kill {{1}})+reload({exe} windows --list)");
    // ctrl-j captures the highlighted window's pane just like prefix j: switch to
    // it, then open its scrollback in nvim. +abort leaves the picker afterward.
    let capture = format!("--bind=ctrl-j:execute-silent({exe} capture --pager nvim --target {{1}})+abort");
    // `omni peek` renders the pane: shells bottom-aligned (recent output), but
    // alternate-screen TUIs (k9s/htop/less/nvim) top-down, since they paint from
    // the top and leave the bottom blank — a plain `tail` would show emptiness.
    let preview = format!("--preview={exe} peek {{1}}");
    // ctrl-g flips recency <-> session order. The mode is persisted, so the
    // reload it triggers (a fresh `omni windows --list`) comes back in the new
    // order, and transform-header re-renders the label so it names what you are
    // looking at rather than a fixed action.
    let order = format!(
        "--bind=ctrl-g:execute-silent({exe} sort --toggle)+reload({exe} windows --list)+transform-list-label({exe} sort --header)"
    );
    let header = format!("--list-label={}", windows_header());

    if let Some(sel) = tmux::pick(
        &[
            "--tiebreak=index",
            &header,
            &order,
            &preview,
            "--preview-window=down,55%,border-top",
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

/// Render a pane's visible screen for the picker preview. A normal shell is
/// bottom-aligned to the preview height (like `tail`) so the newest output and
/// prompt show. An alternate-screen TUI (k9s, htop, less, nvim — `alternate_on`)
/// is shown top-down as painted: those fill from the top and leave the bottom
/// blank, so a tail would slice the content off and preview an empty screen.
fn peek(target: &str) -> Result<()> {
    let raw = tmux::query_bytes(["capture-pane", "-ep", "-t", target])?;
    let alt = tmux::query(["display-message", "-p", "-t", target, "#{alternate_on}"])?;

    let out = if alt.trim() == "1" {
        raw
    } else {
        // Keep only the last FZF_PREVIEW_LINES lines (fzf exports the preview
        // height); operate on bytes so SGR color escapes survive intact.
        let n: usize = std::env::var("FZF_PREVIEW_LINES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(40);
        let lines: Vec<&[u8]> = raw.split(|&b| b == b'\n').collect();
        let start = lines.len().saturating_sub(n);
        let mut out = Vec::new();
        for (i, line) in lines[start..].iter().enumerate() {
            if i > 0 {
                out.push(b'\n');
            }
            out.extend_from_slice(line);
        }
        out
    };
    std::io::stdout().write_all(&out)?;
    Ok(())
}

/// Kill `target`, unless it's the only window in its session — killing that
/// would take the whole session down, so instead pop up a warning and leave it.
fn kill_window(target: &str) -> Result<()> {
    let count: i64 = tmux::query(["display-message", "-p", "-t", target, "#{session_windows}"])?
        .trim()
        .parse()
        .unwrap_or(0);

    if count > 1 {
        return tmux::run(["kill-window", "-t", target]);
    }

    // Stacked warning popup (tmux ≥3.2) over the picker; `read` holds it open
    // until Enter, then it closes and control returns to the list.
    let body = format!(
        "printf '\\n  \\033[1;33m{target}\\033[0m is the only window in its session.\\n  \
         Killing it would kill the session — left untouched.\\n\\n  Press Enter to dismiss…'; \
         read -r _ </dev/tty"
    );
    tmux::run([
        "display-popup", "-E",
        "-T", " won't kill last window ",
        "-w", "60%", "-h", "30%",
        "sh", "-c", &body,
    ])
}

/// The recency-ordered window rows fed to the picker (and re-emitted by
/// `windows --list` after a ctrl-x kill). Field 1 is `session:index`.
fn window_list() -> Result<String> {
    // Columns are TAB-separated here and padded below. They used to be joined
    // with literal double spaces, which cannot be re-split reliably (a pane_title
    // may contain anything, including two spaces) and so could never be aligned.
    let raw = tmux::query([
        "list-windows", "-a", "-F",
        "#{window_activity} #{session_last_attached} #{session_name}:#{window_index}\t#{window_name}\t\
         #{pane_title}\t[#{window_panes}p #{pane_current_command}]\t#{pane_current_path}",
    ])?;
    Ok(align_columns(&strip_sort_keys(&raw, order_mode())))
}

/// Pad TAB-separated columns to a common width and join them with two spaces.
///
/// fzf reprints a row verbatim and lays a literal tab on the next 8-column stop,
/// so alignment has to be baked in rather than left to the terminal. The LAST
/// column is never padded: it runs free to the edge, and padding it would add
/// trailing whitespace to every row.
///
/// Width is counted in chars, not display cells, so a CJK or emoji pane_title
/// still nudges its row — the same approximation orchbus makes, and wrong only
/// for rows that already look unusual.
fn align_columns(body: &str) -> String {
    let rows: Vec<Vec<&str>> = body
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.split('\t').collect())
        .collect();
    if rows.is_empty() {
        return String::new();
    }
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut w = vec![0usize; cols];
    for r in &rows {
        for (i, cell) in r.iter().enumerate() {
            if i + 1 < cols {
                w[i] = w[i].max(cell.chars().count());
            }
        }
    }
    rows.iter()
        .map(|r| {
            r.iter()
                .enumerate()
                .map(|(i, cell)| {
                    if i + 1 < r.len() {
                        let pad = w[i].saturating_sub(cell.chars().count());
                        format!("{cell}{}", " ".repeat(pad))
                    } else {
                        cell.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("  ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Each capture line becomes `session:index<TAB>lineno<TAB>content` so fzf can
/// match on content (`--with-nth=3..` hides the target + lineno) while still
/// recovering the target from field 1. Preview centers the matched line ({2}).
///
/// `history` extends the capture back through scrollback (`-S -`); the preview
/// uses the same range so its line numbers stay aligned with {2}.
fn content(history: bool) -> Result<()> {
    let ws = tmux::query([
        "list-windows", "-a", "-F",
        "#{window_activity} #{session_last_attached} #{session_name}:#{window_index}",
    ])?;
    // Content search always reads recency-first: it is "what did I just see?".
    let ordered = strip_sort_keys(&ws, Order::Recency);

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
            "--tiebreak=index",
            "--delimiter=\t",
            "--with-nth=3..",
            // The rows come from `capture-pane -ep`, so they carry the panes' own
            // colour. Without this fzf prints the escapes as literal text and
            // matches against them too — a query spanning a colour change would
            // silently fail. With it, rows look like the screen they came from.
            "--ansi",
            "--list-label= content · enter jump ",
            preview,
            // The +{2}-/2 offset centres the matched line in the preview; it has
            // to ride along with the new border-top, not be replaced by it.
            "--preview-window=down,55%,border-top,+{2}-/2",
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

    let pos = format!("normal! {}Gzt", top_line(hist, scroll));

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

    // No `set nowrap`/`set number`: the capture window should read like the
    // editor you already configured. Forcing nowrap silently clipped the tail of
    // every full-width line, because the number gutter narrows the text area
    // below the pane width the content was captured at.
    let shell = match pager {
        Pager::Plain => format!("nvim -n -c '{pos}' '{path}'"),
        Pager::Less => format!("less -RN +G '{path}'"),
        Pager::Nvim => format!(
            "nvim -n \
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

/// The buffer line a capture should open on: the pane's first visible line, so
/// the new window shows what the pane shows.
///
/// `capture-pane -S -` lays out `history_size` scrollback lines and then the
/// visible screen, so that line is `history_size + 1` — less `scroll_position`
/// (lines-from-bottom) when copy-mode has scrolled up.
///
/// Anchoring the *top* matters even when the pane isn't scrolled. Capture used to
/// fall back to `G`, which parks the cursor on the last line and relies on
/// 'nowrap' for that final screenful to match the pane; once lines wrap, a
/// screenful of display rows covers fewer buffer lines, so the bottom stops being
/// a reliable anchor and the pane's own view scrolls off the top.
fn top_line(hist: i64, scroll: Option<i64>) -> i64 {
    let top = match scroll {
        // scroll_position is 0 (or absent) unless copy-mode has scrolled up.
        Some(sp) if sp > 0 => hist + 1 - sp,
        _ => hist + 1,
    };
    top.max(1)
}

/// Split lines of `"<activity> <rest>"`, sort by activity descending, and return
/// the `<rest>` lines joined — the recency-ordered input for fzf.
/// Which order the window picker is showing. Persisted, because ctrl-g reloads
/// through a fresh `omni windows --list` process — a mode held inside fzf would
/// not survive the reload it triggers.
#[derive(Clone, Copy, PartialEq)]
pub enum Order {
    /// Most recently active first. The default: it answers "where was I?".
    Recency,
    /// tmux's own order — session name, then window index. Stable and
    /// predictable, which is what you want when you know the name you are after.
    Session,
}

pub fn order_label(o: Order) -> &'static str {
    match o {
        Order::Recency => "recency",
        Order::Session => "session",
    }
}

/// The window picker's one header line: active order first, then the keys.
pub fn windows_header() -> String {
    format!(
        " {} · enter jump · ctrl-g order · ctrl-x kill · ctrl-j capture ",
        order_label(order_mode())
    )
}

fn order_path() -> String {
    let tmp = std::env::var("TMPDIR")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/tmp".into());
    format!("{}/omni.order", tmp.trim_end_matches('/'))
}

pub fn order_mode() -> Order {
    match std::fs::read_to_string(order_path()).as_deref().map(str::trim) {
        Ok("session") => Order::Session,
        _ => Order::Recency,
    }
}

pub fn toggle_order() -> Order {
    let next = match order_mode() {
        Order::Recency => Order::Session,
        Order::Session => Order::Recency,
    };
    let _ = std::fs::write(order_path(), order_label(next));
    next
}

/// Strip the two leading numeric sort keys, ordering rows by `mode`.
///
/// Two keys, not one, because `#{window_activity}` alone ties constantly: any
/// pane running an animated TUI (a Claude Code spinner, k9s) bumps its activity
/// every second, so a dozen windows share the same timestamp and the sort — being
/// stable — silently degenerates to tmux's listing order. The session's
/// last-attached time breaks those ties, so windows in the session you were
/// actually in come first.
fn strip_sort_keys(raw: &str, mode: Order) -> String {
    let mut rows: Vec<(i64, i64, &str)> = raw
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let (a, rest) = l.split_once(' ').unwrap_or(("0", l));
            let (b, rest) = rest.split_once(' ').unwrap_or(("0", rest));
            (a.parse().unwrap_or(0), b.parse().unwrap_or(0), rest)
        })
        .collect();
    // Session order is tmux's own listing order, so leave it alone; only recency
    // reorders. Both drop the keys.
    if mode == Order::Recency {
        rows.sort_by(|x, y| y.0.cmp(&x.0).then_with(|| y.1.cmp(&x.1)));
    }
    rows.into_iter()
        .map(|(_, _, r)| r)
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
    fn top_line_follows_pane_view() {
        // Not scrolled: first line of the visible screen, not the end of the file.
        assert_eq!(top_line(398, None), 399);
        assert_eq!(top_line(398, Some(0)), 399);
        // Scrolled up 90 lines in copy-mode.
        assert_eq!(top_line(398, Some(90)), 309);
        // Scrolled to the very top, and past it — never below line 1.
        assert_eq!(top_line(398, Some(398)), 1);
        assert_eq!(top_line(398, Some(500)), 1);
        // Empty history: the whole capture is the visible screen.
        assert_eq!(top_line(0, None), 1);
    }

    #[test]
    fn align_pads_every_column_but_the_last() {
        let body = "a\tlong-window\tx\nbbbb\tw\tyy";
        let out = align_columns(body);
        // col0 padded to 4 ("bbbb"), col1 to 11 ("long-window"), col2 free.
        assert_eq!(out, "a     long-window  x\nbbbb  w            yy");
        // no row ends in whitespace
        for line in out.lines() {
            assert_eq!(line, line.trim_end(), "trailing space on: {line:?}");
        }
    }

    #[test]
    fn align_keeps_first_field_a_clean_target_for_fzf() {
        // fzf's {1} is the first WHITESPACE field; padding must not glue the
        // pane target to the next column.
        let out = align_columns("s:1\tzsh\tp\nlonger-session:12\tnvim\tq");
        let first = out.lines().next().unwrap().split_whitespace().next().unwrap();
        assert_eq!(first, "s:1");
    }

    #[test]
    fn align_survives_a_row_with_fewer_columns() {
        let out = align_columns("a\tb\tc\nsolo");
        assert_eq!(out.lines().count(), 2);
        assert!(out.lines().any(|l| l == "solo"));
    }

    #[test]
    fn recency_is_descending_and_strips_both_keys() {
        let raw = "100 1 alpha\n300 1 gamma\n200 1 beta\n";
        assert_eq!(strip_sort_keys(raw, Order::Recency), "gamma\nbeta\nalpha");
    }

    #[test]
    fn session_last_attached_breaks_an_activity_tie() {
        // The case that matters in practice: animated panes (a Claude spinner,
        // k9s) all stamp the same activity second, so the first key ties and the
        // session you were last in has to decide.
        let raw = "500 10 old-session\n500 90 recent-session\n500 50 mid-session\n";
        assert_eq!(
            strip_sort_keys(raw, Order::Recency),
            "recent-session\nmid-session\nold-session"
        );
    }

    #[test]
    fn session_order_keeps_tmux_listing_order_and_still_strips_keys() {
        let raw = "100 1 alpha\n300 9 gamma\n200 5 beta\n";
        assert_eq!(strip_sort_keys(raw, Order::Session), "alpha\ngamma\nbeta");
    }

    #[test]
    fn a_missing_second_key_degrades_without_eating_the_row() {
        // Defensive: a one-key line must still yield its content, not swallow it.
        assert_eq!(strip_sort_keys("100 alpha\n", Order::Recency), "alpha");
    }
}
