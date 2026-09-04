//! omni — fzf-backed tmux navigation + scrollback capture.
//!
//!   omni windows   fuzzy-jump to any window across all sessions   (prefix b)
//!   omni content   fuzzy-search this session: screen + scrollback  (prefix a)
//!                  --all: every session, not just this one         (prefix A)
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
use std::sync::OnceLock;

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
        /// Only windows with an alarm on them, each row led by its state:
        /// [!] bell, [~] stopped, [*] running, [.] armed but not yet tripped.
        /// The full list is what the picker shows without this; here the
        /// question is what you set an alarm on and whether it has gone off.
        #[arg(long)]
        alerts: bool,
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
    /// Fuzzy-search this session's windows — screen and scrollback — then switch.
    Content {
        /// Search every session's windows, not just the one you are in.
        ///
        /// `history` is kept as an alias because that is the flag baked into the
        /// `prefix A` binding of any tmux server started before the rename; a
        /// running server holds its bindings in memory, so dropping the old name
        /// would break that key until the config was reloaded.
        #[arg(long, alias = "history")]
        all: bool,
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
        Cmd::Windows { list, alerts } => windows(list, alerts),
        Cmd::Sort { toggle, header } => {
            let mode = if toggle { toggle_order() } else { order_mode() };
            if header {
                println!("{}", windows_header());
            } else {
                println!("{}", order_label(mode));
            }
            Ok(())
        }
        Cmd::Content { all } => content(all),
        Cmd::Peek { target } => peek(&target),
        Cmd::Kill { target } => kill_window(&target),
        Cmd::Capture { pager, target } => capture(pager, target),
    }
}

/// Rows sorted most-recently-active first: `#{window_activity}` (epoch of last
/// activity) is prefixed as a numeric sort key, then stripped before fzf.
/// `--tiebreak=index` keeps that recency order when match scores tie.
fn windows(list: bool, alerts: bool) -> Result<()> {
    let input = window_list(alerts)?;

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
    // fzf's {N} is the Nth whitespace field of the row. That is the target in
    // the plain list, but --alerts prepends the state marker, so every binding
    // below has to reach one field further along — getting this wrong is silent:
    // `omni peek [*]` just fails and the preview pane stays blank.
    let tgt = if alerts { "{2}" } else { "{1}" };
    // `omni kill` guards the last-window case (would kill the session) with a
    // warning popup instead; the reload then refreshes the (maybe unchanged) list.
    // `mode` rides on every self-invocation below: without it ctrl-x and ctrl-g
    // would reload the *full* list from inside the alerts view, dropping the
    // filter and the state column on the first keystroke.
    let mode = if alerts { " --alerts" } else { "" };
    let kill = format!("--bind=ctrl-x:execute-silent({exe} kill {tgt})+reload({exe} windows --list{mode})");
    // ctrl-j captures the highlighted window's pane just like prefix j: switch to
    // it, then open its scrollback in nvim. +abort leaves the picker afterward.
    let capture = format!("--bind=ctrl-j:execute-silent({exe} capture --pager nvim --target {tgt})+abort");
    // `omni peek` renders the pane: shells bottom-aligned (recent output), but
    // alternate-screen TUIs (k9s/htop/less/nvim) top-down, since they paint from
    // the top and leave the bottom blank — a plain `tail` would show emptiness.
    let preview = format!("--preview={exe} peek {tgt}");
    // ctrl-g flips recency <-> session order. The mode is persisted, so the
    // reload it triggers (a fresh `omni windows --list`) comes back in the new
    // order, and transform-header re-renders the label so it names what you are
    // looking at rather than a fixed action.
    let order = format!(
        "--bind=ctrl-g:execute-silent({exe} sort --toggle)+reload({exe} windows --list{mode})+transform-list-label({exe} sort --header)"
    );
    let header = format!("--list-label={}", if alerts { alerts_header() } else { windows_header() });

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
        // {1} in fzf = first whitespace field, which in alerts mode is the state
        // marker rather than the target — so take the last field that looks like
        // one instead of blindly taking the first.
        if let Some(target) = sel.split_whitespace().find(|f| f.contains(':')) {
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
fn window_list(alerts: bool) -> Result<String> {
    // Columns are TAB-separated here and padded below. They used to be joined
    // with literal double spaces, which cannot be re-split reliably (a pane_title
    // may contain anything, including two spaces) and so could never be aligned.
    //
    // The alarm fields ride along at the END, past every displayed column, so
    // strip_sort_keys keeps working on the front of the row and only mark_alerts
    // has to know they exist. They cost nothing when unused: tmux fills them in
    // the same call either way.
    let raw = tmux::query([
        "list-windows", "-a", "-F",
        "#{window_activity} #{session_last_attached} #{session_name}:#{window_index}\t#{window_name}\t\
         #{pane_title}\t[#{window_panes}p #{pane_current_command}]\t#{pane_current_path}\t\
         #{window_bell_flag}#{window_silence_flag}#{window_activity_flag}\t\
         #{monitor-activity}#{?#{monitor-silence},1,0}",
    ])?;
    let body = strip_sort_keys(&raw, order_mode());
    Ok(align_columns(&if alerts { mark_alerts(&body) } else { drop_alarm_cols(&body) }))
}

/// The two trailing alarm columns are for mark_alerts, not for the eye.
fn drop_alarm_cols(body: &str) -> String {
    body.lines()
        .map(|l| {
            let mut cols: Vec<&str> = l.split('\t').collect();
            cols.truncate(cols.len().saturating_sub(2));
            cols.join("\t")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Keep only windows carrying an alarm, and lead each row with its state.
///
/// tmux raises the three flags itself — bell on a \a byte, activity on the first
/// byte of output under monitor-activity, silence once monitor-silence seconds
/// pass with none — and clears them when the window is selected. So this reads
/// the aftermath rather than watching anything.
///
/// Silence outranks activity because both flags stay raised once set: a window
/// that has stopped should say so rather than report the burst before it.
///
/// A window whose monitors are armed but which has raised nothing is `[.]`,
/// waiting. That state is not a tmux flag but an inference from the options, and
/// it is the one the status line cannot show — with no flag raised there is
/// nothing for its styling to hook onto, so an armed window looks exactly like
/// an unarmed one there. Which is most of why this mode is worth having.
fn mark_alerts(body: &str) -> String {
    body.lines()
        .filter_map(|l| {
            let mut cols: Vec<&str> = l.split('\t').collect();
            let armed = cols.pop()?;
            let flags = cols.pop()?;
            let mut f = flags.chars();
            let state = match (f.next(), f.next(), f.next()) {
                (Some('1'), _, _) => "[!]",
                (_, Some('1'), _) => "[~]",
                (_, _, Some('1')) => "[*]",
                _ if armed.contains('1') => "[.]",
                _ => return None,
            };
            Some(std::iter::once(state).chain(cols).collect::<Vec<_>>().join("\t"))
        })
        .collect::<Vec<_>>()
        .join("\n")
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
/// Every window is captured to the start of its scrollback (`-S -`); the preview
/// uses the same range so its line numbers stay aligned with {2}.
///
/// Enter switches to the window AND lands on the line — see jump_to_line. The
/// lineno rode along for the preview only and used to be dropped on selection,
/// which left you in the right window hunting for the row you had just picked.
///
/// **Scope is the only difference between the two keys.** `prefix a` searches
/// this session, `prefix A` (`--all`) every session, and both read the full
/// scrollback.
///
/// `a` used to search the viewport alone, which was fine while it also spanned
/// the server — but once it narrowed to one session the two limits multiplied and
/// it stopped being able to find anything: four windows' visible screens came to
/// 97 rows, against 105k for `A`. A session is a workspace (one repo, one
/// worktree, one machine), so the useful question there is not "what is on screen
/// in this session" but "what have I seen in this session" — the same question
/// `A` asks, over the part of the server you are actually working in. Hence one
/// capture range and one knob.
fn content(all: bool) -> Result<()> {
    // One call answers both questions: which session to search, and which window
    // to put first. Session names cannot contain ':' (tmux rejects it), so the
    // split is unambiguous.
    let cur = current_window();
    let sess = cur.split_once(':').map(|(s, _)| s).unwrap_or_default();

    const FMT: &str = "#{window_activity} #{session_last_attached} #{session_name}:#{window_index}";
    // For the session scope, `-t` is not decoration: a bare `list-windows`
    // resolves "current session" through `$TMUX_PANE`, which a popup inherits
    // from the client's environment and which can point into a *different*
    // session — that silently searches the wrong session's windows.
    // `#{client_session}` is resolved from the client rather than a pane target,
    // so it always names the session you are actually looking at.
    let ws = if all {
        tmux::query(["list-windows", "-a", "-F", FMT])?
    } else if sess.is_empty() {
        anyhow::bail!("cannot tell which session this client is on")
    } else {
        tmux::query(["list-windows", "-t", sess, "-F", FMT])?
    };
    // Content search always reads recency-first: it is "what did I just see?".
    // And the most recent thing you saw is the window you are on, which
    // `#{window_activity}` alone does not say — hence current_first. The second
    // sort key still earns its keep for `--all`, which spans sessions; within one
    // session it is constant, so activity ties there fall back to tmux's window
    // order, which is what you would guess anyway.
    let ordered = current_first(&strip_sort_keys(&ws, Order::Recency), &cur);

    // Capture from the start of scrollback; the preview must use the same range
    // or {2} lands on the wrong line.
    let cap = ["capture-pane", "-ep", "-S", "-", "-t"];
    let preview = "--preview=tmux capture-pane -ep -S - -t {1} | awk -v n={2} 'NR==n{print \"\\033[7m\" $0 \"\\033[0m\"; next}{print}'";

    let mut input = String::new();
    for t in ordered.lines() {
        let args: Vec<&str> = cap.iter().copied().chain([t]).collect();
        let pane = tmux::query(args).unwrap_or_default();
        input.push_str(&content_rows(t, &pane));
    }

    // Name the session on the border when that is the scope: a narrowed picker
    // is otherwise indistinguishable from a wide one, and "why is my other
    // window not in here?" is the first question it raises. `--all` spans every
    // session, so there is no one session to name and its label is the plain one
    // `prefix A` has always had.
    let label = if all {
        "--list-label= content · enter jump ".to_string()
    } else {
        format!("--list-label= content · {sess} · enter jump ")
    };

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
            &label,
            preview,
            // The +{2}-/2 offset centres the matched line in the preview; it has
            // to ride along with the new border-top, not be replaced by it.
            "--preview-window=down,55%,border-top,+{2}-/2",
        ],
        input,
    )? {
        let mut fields = sel.split('\t');
        if let Some(target) = fields.next() {
            tmux::run(["switch-client", "-t", target])?;
            if let Some(n) = fields.next().and_then(|s| s.trim().parse::<i64>().ok()) {
                jump_to_line(target, n)?;
            }
        }
    }
    Ok(())
}

/// One window's picker rows — `target<TAB>lineno<TAB>content` per captured line,
/// with the blank ones dropped.
///
/// A capture ends with the viewport, which is always `pane_height` lines, so
/// every window contributes its unused tail; a prompt that pads itself with a
/// blank line adds more throughout. On a real 32-window server that is 11,085 of
/// 116,670 rows — and it was 587 of 1531 back when only the viewport was read,
/// which is where this was first noticed. They cannot be matched (there is
/// nothing for fzf to score) and there is no reason to put the cursor on one, so
/// all they did was inflate the count and space the rows you actually read apart
/// by a screenful of nothing.
///
/// The lineno stays the line's position in the *capture*, not its position in
/// this list. The preview centres on it (`awk NR==n`) and jump_to_line converts
/// it into a copy-mode row, so renumbering after a dropped blank would put both
/// on the wrong line — which is why the filter comes after `enumerate` and never
/// before it. The preview keeps its blanks for the same reason: it is a picture
/// of the pane, and a pane has blank lines on it.
fn content_rows(target: &str, pane: &str) -> String {
    let mut out = String::new();
    for (i, line) in pane.lines().enumerate() {
        if is_blank(line) {
            continue;
        }
        out.push_str(&format!("{target}\t{}\t{line}\n", i + 1));
    }
    out
}

/// A captured line with nothing visible on it.
///
/// The cheap test first, since in practice nearly every blank row arrives as a
/// plain empty string. The regex is for the other case: `capture-pane -e` writes
/// out the pane's colour state as it changes, so a blank row can come through as
/// a couple of SGR sequences and no text — blank on screen, but not `""`.
fn is_blank(line: &str) -> bool {
    if line.trim().is_empty() {
        return true;
    }
    static ESC: OnceLock<regex::Regex> = OnceLock::new();
    let re = ESC.get_or_init(|| {
        regex::Regex::new(r"\x1b\[[0-9;:?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b.")
            .expect("static regex")
    });
    re.replace_all(line, "").trim().is_empty()
}

/// The `session:index` the client is looking at — the window the picker's popup
/// is drawn over. Read as one format so it is the client's *current* window and
/// not whatever pane a stale `$TMUX_PANE` in the popup's environment points at;
/// tmux resolves the client first, which is what makes this safe.
///
/// Empty when tmux cannot say, which current_first reads as "no current window".
fn current_window() -> String {
    tmux::query(["display-message", "-p", "#{client_session}:#{window_index}"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Hoist the current window to the front of the content order.
///
/// `#{window_activity}` is an *output* timestamp, not an attention one: a pane
/// running Claude Code or k9s restamps it every second, while the window you are
/// actually sitting in — nvim, a shell at a prompt — holds a constant one. So
/// recency ranks the chatty windows above the one in front of you: on a real
/// 32-window server the current window came sixth, behind five `claude`/`k9s`
/// windows. And because `--tiebreak=index` settles a score tie in favour of the
/// earlier row, a query matching text that also exists in one of those windows
/// jumped there instead — the content you were looking at losing to an identical
/// line somewhere else.
///
/// Searching what you can see has to land where you are, so the current window
/// goes first and everything else keeps its recency order.
///
/// A `cur` that is not in the list is dropped rather than prepended: tmux may
/// have said nothing, or the window may have gone between the two calls, and an
/// invented target would just be a row whose capture is empty.
fn current_first(ordered: &str, cur: &str) -> String {
    let mut rows: Vec<&str> = ordered.lines().collect();
    if let Some(i) = rows.iter().position(|l| *l == cur) {
        let row = rows.remove(i);
        rows.insert(0, row);
    }
    rows.join("\n")
}

/// Put the picked line under the cursor: copy-mode on the window's active pane,
/// scrolled so the line is on screen and the cursor on it.
///
/// copy-mode is the only way tmux can point at a line — a live pane has no
/// cursor to spare — and it is what the pane is for afterwards anyway: read the
/// line in place, `v`+motion then `y` to copy part of it, `q` to drop out.
///
/// The cursor is moved and nothing else. This used to end with `select-line`, to
/// reproduce the reverse-video row the preview showed, and that was the wrong
/// trade: it hands you copy-mode with a live line-wise selection, so the first
/// motion you make drags the selection with it instead of just moving, and
/// getting back to a clean cursor means finding `clear-selection` (Escape, in
/// the default vi table) rather than simply moving. A selection is something you
/// start on purpose; landing on a line is not that.
///
/// The pane target is the same `session:index` the row was captured from, so
/// tmux resolves it to that window's active pane, exactly as `capture` does.
fn jump_to_line(target: &str, n: i64) -> Result<()> {
    let disp = tmux::query([
        "display-message", "-p", "-t", target,
        "#{history_size} #{pane_height}",
    ])?;
    let mut it = disp.split_whitespace();
    let hist: i64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let height: i64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let (oy, row) = copy_position(hist, height, n);

    tmux::run(["copy-mode", "-t", target])?;
    // goto-line takes the scroll offset in lines-from-the-bottom, not a line
    // number, and it does NOT clamp a negative — window_copy_goto_line sends
    // anything < 0 to the top of the history instead, which is the wrong end of
    // the buffer. copy_position clamps, so nothing negative gets here.
    tmux::run(["send-keys", "-t", target, "-X", "goto-line", &oy.to_string()])?;
    // Scrolling moves the view, not the cursor, so place the cursor separately:
    // top-line pins it to the first visible row, then walk down to the hit. One
    // scroll plus at most a screenful of steps — reaching the line by cursor-up
    // alone would mean tens of thousands of steps through a deep scrollback.
    tmux::run(["send-keys", "-t", target, "-X", "top-line"])?;
    if row > 0 {
        tmux::run(["send-keys", "-t", target, "-X", "-N", &row.to_string(), "cursor-down"])?;
    }
    Ok(())
}

/// Where copy-mode has to sit for capture line `n` to be under the cursor:
/// `(scroll offset from the bottom, screen row from the top)`.
///
/// `capture-pane -S -` lays out `hist` scrollback lines and then the `height`
/// visible ones, so with the view scrolled `oy` lines off the bottom the top row
/// shows line `hist + 1 - oy` — invert that and the row of a line is
/// `line - hist - 1 + oy`.
///
/// Both clamps are load-bearing rather than defensive tidiness: `oy` past the
/// ends of the buffer is what goto-line mishandles, and a `row` from a capture
/// that no longer matches the pane (it scrolled, or cleared, between the capture
/// and Enter) would otherwise become a `send-keys -N <huge>` cursor walk.
/// Every row now comes from a `-S -` capture, so `n` is a position in the whole
/// buffer and there is no viewport-relative case to convert — the second half of
/// this function used to exist only for the viewport-only `prefix a`, which no
/// longer has a caller.
///
/// The hit is centred, so the lines either side of it come with it. A hit that
/// was already on screen gets moved too, which is the one thing lost with the
/// viewport mode: it used to leave the view alone so the line stayed exactly
/// where the picker showed it. The clamp still pins the last screenful, so the
/// bottom of the buffer does not scroll into empty space.
fn copy_position(hist: i64, height: i64, n: i64) -> (i64, i64) {
    let line = n;
    let centre = height / 2;
    let oy = (hist + 1 - line + centre).clamp(0, hist.max(0));
    let row = (line - hist - 1 + oy).clamp(0, (height - 1).max(0));
    (oy, row)
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

/// The alerts view's header. Same keys, different subject — and it names the
/// markers, since [.] for "armed, nothing yet" is not guessable.
pub fn alerts_header() -> String {
    format!(
        " alerts · {} · [!] bell [~] stopped [*] running [.] waiting · enter jump ",
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
    fn alerts_keep_only_alarmed_rows_and_lead_with_state() {
        // flags column is bell/silence/activity; armed is monitor-a/monitor-s.
        let body = "\
a:1\tzsh\ttitle\t[1p zsh]\t/p\t100\t00\n\
a:2\tzsh\ttitle\t[1p zsh]\t/p\t010\t01\n\
a:3\tzsh\ttitle\t[1p zsh]\t/p\t001\t10\n\
a:4\tzsh\ttitle\t[1p zsh]\t/p\t000\t01\n\
a:5\tzsh\ttitle\t[1p zsh]\t/p\t000\t00";
        let out = mark_alerts(body);
        let states: Vec<&str> = out.lines().map(|l| l.split('\t').next().unwrap()).collect();
        // a:5 is neither flagged nor armed and is gone; the rest lead with state.
        assert_eq!(states, ["[!]", "[~]", "[*]", "[.]"]);
        // The displayed columns survive untouched, alarm columns stripped.
        assert_eq!(out.lines().next().unwrap(), "[!]\ta:1\tzsh\ttitle\t[1p zsh]\t/p");
    }

    #[test]
    fn silence_outranks_activity_when_both_flags_stand() {
        // Both raised: the window has stopped, which is the newer fact.
        let body = "a:1\tzsh\tt\t[1p zsh]\t/p\t011\t11";
        assert!(mark_alerts(body).starts_with("[~]"));
    }

    #[test]
    fn plain_mode_drops_the_alarm_columns() {
        let body = "a:1\tzsh\tt\t[1p zsh]\t/p\t000\t00";
        assert_eq!(drop_alarm_cols(body), "a:1\tzsh\tt\t[1p zsh]\t/p");
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

    // The numbers below were read off a real pane (tmux 3.6a, hist 278,
    // height 24, `seq 1 300` in the scrollback): position copy-mode this way and
    // #{copy_cursor_line} is the content of capture line n, for every n.
    #[test]
    fn a_hit_is_centred_and_the_row_follows_the_scroll() {
        assert_eq!(copy_position(278, 24, 151), (140, 12));
        assert_eq!(copy_position(278, 24, 100), (191, 12));
    }

    #[test]
    fn both_ends_clamp_the_scroll_and_spend_the_rest_on_the_row() {
        // Top of the history: there is nothing left to scroll, so the row
        // absorbs what centring asked for.
        assert_eq!(copy_position(278, 24, 2), (278, 1));
        assert_eq!(copy_position(278, 24, 13), (278, 12));
        // Last screenful: same at the other end — oy bottoms out at 0 and the
        // cursor walks down instead. These are the rows a viewport hit lands on
        // now that there is no viewport-only mode to leave the view alone.
        assert_eq!(copy_position(278, 24, 295), (0, 16));
        assert_eq!(copy_position(278, 24, 302), (0, 23));
    }

    #[test]
    fn a_line_past_the_capture_cannot_become_a_giant_cursor_walk() {
        // Stale capture (the pane scrolled or cleared before Enter): land on the
        // last row rather than sending `-N 9998` cursor-down.
        assert_eq!(copy_position(0, 24, 9_999), (0, 23));
        // And an empty pane has no row to land on, but must not go negative.
        assert_eq!(copy_position(0, 0, 1), (0, 0));
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
    fn blank_rows_go_but_the_line_numbers_stay() {
        // The invariant that matters: the lineno is the line's place in the
        // capture, so the preview's `awk NR==n` and jump_to_line's copy-mode row
        // still point at it. Dropping line 2 must not renumber line 4 to 3.
        let pane = "alpha\n\n   \nbravo\n\ncharlie";
        assert_eq!(
            content_rows("s:1", pane),
            "s:1\t1\talpha\ns:1\t4\tbravo\ns:1\t6\tcharlie\n"
        );
    }

    #[test]
    fn a_row_of_nothing_but_escapes_is_blank_too() {
        // capture-pane -e re-emits the pane's colour state, so a blank row can
        // arrive as SGR sequences and no text. Coloured *text* obviously stays.
        assert!(is_blank("\x1b[39m\x1b[49m"));
        assert!(is_blank("\x1b[0m   \x1b[K"));
        assert!(!is_blank("\x1b[31mred\x1b[0m"));
        let pane = "\x1b[39m\x1b[49m\n\x1b[31mred\x1b[0m";
        assert_eq!(content_rows("s:1", pane), "s:1\t2\t\x1b[31mred\x1b[0m\n");
    }

    #[test]
    fn an_all_blank_pane_contributes_no_rows() {
        // An untouched window is not worth a single row, let alone a screenful.
        assert_eq!(content_rows("s:1", "\n\n\n"), "");
        assert_eq!(content_rows("s:1", ""), "");
    }

    #[test]
    fn the_window_you_are_on_leads_the_content_order() {
        // The real shape of the bug: every window ahead of the current one is a
        // Claude/k9s pane restamping its activity every second, so "recency"
        // buried the window in front of you and an identical line in one of them
        // won the tiebreak.
        let ordered = "rc:2\ncosmos-main:2\nmisc:2\nagents:1\nagents:2\nbtw:1";
        assert_eq!(
            current_first(ordered, "agents:2"),
            "agents:2\nrc:2\ncosmos-main:2\nmisc:2\nagents:1\nbtw:1"
        );
    }

    #[test]
    fn an_unknown_current_window_is_never_invented() {
        // tmux said nothing, or the window went between the two calls: leave the
        // order alone rather than adding a target that captures nothing.
        let ordered = "rc:2\nbtw:1";
        assert_eq!(current_first(ordered, ""), ordered);
        assert_eq!(current_first(ordered, "gone:9"), ordered);
        assert_eq!(current_first("", "rc:2"), "");
    }

    #[test]
    fn a_current_window_already_first_stays_put() {
        // Idempotent, so the hoist cannot reorder what recency already got right.
        let ordered = "rc:2\nbtw:1";
        assert_eq!(current_first(ordered, "rc:2"), ordered);
    }

    #[test]
    fn hoisting_matches_on_the_whole_target_not_a_prefix() {
        // `agents:1` must not be mistaken for `agents:12`, and a session whose
        // name is a prefix of another must not be either.
        let ordered = "agents:12\nagents:1\ncosmos:1\ncosmos-main:1";
        assert_eq!(
            current_first(ordered, "agents:1"),
            "agents:1\nagents:12\ncosmos:1\ncosmos-main:1"
        );
        assert_eq!(
            current_first(ordered, "cosmos:1"),
            "cosmos:1\nagents:12\nagents:1\ncosmos-main:1"
        );
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
