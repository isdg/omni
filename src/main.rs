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
        Cmd::Content { history } => content(history),
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
/// `history` extends the capture back through scrollback (`-S -`); the preview
/// uses the same range so its line numbers stay aligned with {2}.
///
/// Enter switches to the window AND lands on the line — see jump_to_line. The
/// lineno rode along for the preview only and used to be dropped on selection,
/// which left you in the right window hunting for the row you had just picked.
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
        let mut fields = sel.split('\t');
        if let Some(target) = fields.next() {
            tmux::run(["switch-client", "-t", target])?;
            if let Some(n) = fields.next().and_then(|s| s.trim().parse::<i64>().ok()) {
                jump_to_line(target, n, history)?;
            }
        }
    }
    Ok(())
}

/// Put the picked line under the cursor: copy-mode on the window's active pane,
/// scrolled so the line is on screen, cursor on it, the line selected.
///
/// copy-mode is the only way tmux can point at a line — a live pane has no
/// cursor to spare — and it is what the pane is for afterwards anyway: read the
/// line in place, or `y` it. Escape clears the selection and leaves you free to
/// move; `q` drops out. `select-line` is there because a bare block cursor mid
/// row is nearly invisible; it reproduces the reverse-video row the preview
/// showed, so the hit looks the same before and after Enter.
///
/// The pane target is the same `session:index` the row was captured from, so
/// tmux resolves it to that window's active pane, exactly as `capture` does.
fn jump_to_line(target: &str, n: i64, history: bool) -> Result<()> {
    let disp = tmux::query([
        "display-message", "-p", "-t", target,
        "#{history_size} #{pane_height}",
    ])?;
    let mut it = disp.split_whitespace();
    let hist: i64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let height: i64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let (oy, row) = copy_position(hist, height, n, history);

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
    tmux::run(["send-keys", "-t", target, "-X", "select-line"])
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
fn copy_position(hist: i64, height: i64, n: i64, history: bool) -> (i64, i64) {
    // Without --history the rows came from the viewport alone, where line n is
    // screen row n - 1; with it they came from the whole buffer. Convert the
    // first into the second so one formula covers both.
    let line = if history { n } else { hist + n };
    // A history hit needs the view moved, and centring it shows the lines either
    // side. A viewport hit is already on screen: centring would scroll the very
    // screen the picker just showed you, so leave it (the clamp below turns this
    // 0 into oy = 0) and the line stays exactly where you saw it.
    let centre = if history { height / 2 } else { 0 };
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
    fn a_history_hit_is_centred_and_the_row_follows_the_scroll() {
        assert_eq!(copy_position(278, 24, 151, true), (140, 12));
        assert_eq!(copy_position(278, 24, 100, true), (191, 12));
    }

    #[test]
    fn a_viewport_hit_stays_where_the_picker_showed_it() {
        // No --history: the view must not move, so the row is the screen row.
        assert_eq!(copy_position(278, 24, 1, false), (0, 0));
        assert_eq!(copy_position(278, 24, 5, false), (0, 4));
        assert_eq!(copy_position(278, 24, 24, false), (0, 23));
    }

    #[test]
    fn both_ends_clamp_the_scroll_and_spend_the_rest_on_the_row() {
        // Top of the history: there is nothing left to scroll, so the row
        // absorbs what centring asked for.
        assert_eq!(copy_position(278, 24, 2, true), (278, 1));
        assert_eq!(copy_position(278, 24, 13, true), (278, 12));
        // Last screenful: same at the other end — oy bottoms out at 0 and the
        // cursor walks down instead.
        assert_eq!(copy_position(278, 24, 295, true), (0, 16));
        assert_eq!(copy_position(278, 24, 302, true), (0, 23));
    }

    #[test]
    fn a_line_past_the_capture_cannot_become_a_giant_cursor_walk() {
        // Stale capture (the pane scrolled or cleared before Enter): land on the
        // last row rather than sending `-N 9998` cursor-down.
        assert_eq!(copy_position(0, 24, 9_999, true), (0, 23));
        // And an empty pane has no row to land on, but must not go negative.
        assert_eq!(copy_position(0, 0, 1, true), (0, 0));
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
