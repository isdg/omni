//! Thin wrappers around the `tmux` and `fzf` CLIs — the two external tools omni
//! orchestrates. Everything omni does is: ask tmux for pane/window data, pipe it
//! through fzf, and act on the selection.

use anyhow::{Context, Result};
use std::ffi::OsStr;
use std::io::Write;
use std::process::{Command, Stdio};

/// Run `tmux <args>` and return stdout as a String (lossy UTF-8), trimmed of a
/// trailing newline. Used for `display-message` / `list-*` queries.
pub fn query<I, S>(args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let out = Command::new("tmux")
        .args(args)
        .output()
        .context("failed to run tmux (is it on PATH?)")?;
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    if s.ends_with('\n') {
        s.pop();
    }
    Ok(s)
}

/// Run `tmux <args>` for raw byte output — capture-pane content may contain
/// escape sequences and non-UTF-8 bytes we must preserve verbatim.
pub fn query_bytes<I, S>(args: I) -> Result<Vec<u8>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let out = Command::new("tmux")
        .args(args)
        .output()
        .context("failed to run tmux")?;
    Ok(out.stdout)
}

/// Run `tmux <args>` for its side effect (switch-client, new-window, …).
pub fn run<I, S>(args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new("tmux")
        .args(args)
        .status()
        .context("failed to run tmux")?;
    Ok(())
}

/// Pipe `input` into `fzf <args>` and return the selected line, or `None` if the
/// user aborted (Esc / empty). A writer thread feeds stdin so a large list can't
/// deadlock against fzf's rendering.
pub fn pick(args: &[&str], input: String) -> Result<Option<String>> {
    let mut child = Command::new("fzf")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to spawn fzf (is it installed?)")?;

    let mut stdin = child.stdin.take().expect("piped stdin");
    let writer = std::thread::spawn(move || {
        // Ignore write errors: fzf may exit (Esc) before consuming all input,
        // closing the pipe — a broken-pipe here is expected, not a failure.
        let _ = stdin.write_all(input.as_bytes());
    });

    let out = child.wait_with_output().context("fzf wait failed")?;
    let _ = writer.join();

    if !out.status.success() {
        return Ok(None); // 130 = Esc, 1 = no match
    }
    let sel = String::from_utf8_lossy(&out.stdout)
        .trim_end_matches('\n')
        .to_string();
    Ok(if sel.is_empty() { None } else { Some(sel) })
}
