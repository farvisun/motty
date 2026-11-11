//! Text written by the coding agent the user already has: commit messages
//! and pull request titles and bodies. Motty holds no API keys; it runs the
//! agent's one-shot mode (`claude -p`, `codex exec`, `opencode run`) with
//! the prompt as the last argument, or `git-ai-command` when set.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// One-shot modes, in the order they're looked for.
const DEFAULTS: [(&str, &[&str]); 3] = [
    ("claude", &["-p"]),
    ("codex", &["exec"]),
    ("opencode", &["run"]),
];

/// A diff is cut to this much, so the prompt stays a sane size.
pub const MAX_DIFF: usize = 60_000;
/// How long the agent may take.
const TIMEOUT: Duration = Duration::from_secs(120);

/// The program and its arguments, before the prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Writer {
    pub program: PathBuf,
    pub args: Vec<String>,
}

impl Writer {
    /// Its name, for buttons: `claude`.
    pub fn name(&self) -> String {
        self.program
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// `git-ai-command` split on spaces (its program found where agents get
/// installed), else the first agent installed.
pub fn writer(configured: Option<&str>) -> Option<Writer> {
    if let Some(line) = configured {
        let mut words = line.split_whitespace();
        let program = words.next()?;
        let path = if program.contains('/') {
            PathBuf::from(program)
        } else {
            super::locate(program)?
        };
        return Some(Writer {
            program: path,
            args: words.map(str::to_owned).collect(),
        });
    }
    DEFAULTS.iter().find_map(|(name, args)| {
        Some(Writer {
            program: super::locate(name)?,
            args: args.iter().map(|a| (*a).to_owned()).collect(),
        })
    })
}

/// `text` cut to `max` bytes at a line end, with a note when cut.
pub fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let cut = text[..end].rfind('\n').unwrap_or(end);
    format!(
        "{}\n[… diff cut: {} more bytes]",
        &text[..cut],
        text.len() - cut
    )
}

/// The prompt for a commit message about `diff`, in the style of `log`
/// (recent subjects).
pub fn commit_prompt(diff: &str, log: &str) -> String {
    format!(
        "Write a git commit message for the staged changes below.\n\
         Follow the style of the repository's recent commit subjects (for example \
         Conventional Commits when they use it). One short subject line in the \
         imperative, under 72 characters; add a blank line and a short body only when \
         the reason for the change isn't obvious. Reply with the message only: no \
         quotes, no code fences, no commentary.\n\n\
         Recent commit subjects:\n{log}\n\nStaged diff:\n{}",
        clip(diff, MAX_DIFF)
    )
}

/// Takes what an agent replied down to the text asked for: no code fence
/// around it and no blank lines at the ends.
pub fn tidy(reply: &str) -> String {
    let mut text = reply.trim();
    if let Some(inner) = text.strip_prefix("```") {
        // Drop the fence's language word and the closing fence.
        let inner = inner.split_once('\n').map_or("", |(_, rest)| rest);
        text = inner.trim_end().strip_suffix("```").unwrap_or(inner).trim();
    }
    text.trim_matches('"').trim().to_owned()
}

/// Runs the writer in `dir` with `prompt`; its reply, tidied.
pub fn write(writer: &Writer, dir: &Path, prompt: &str) -> Result<String, String> {
    let child = Command::new(&writer.program)
        .args(&writer.args)
        .arg(prompt)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't run {}: {e}", writer.name()))?;
    // Read while it runs (a full pipe would stall it), up to the timeout.
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    let out = match rx.recv_timeout(TIMEOUT) {
        Ok(out) => out.map_err(|e| e.to_string())?,
        Err(_) => {
            // SAFETY: plain kill(2) on the child we started.
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
            return Err(format!("{} took too long", writer.name()));
        }
    };
    let text = tidy(&String::from_utf8_lossy(&out.stdout));
    if !out.status.success() || text.is_empty() {
        let err = String::from_utf8_lossy(&out.stderr);
        let last = err
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        return Err(format!("{} didn't write anything. {last}", writer.name())
            .trim()
            .to_owned());
    }
    Ok(text)
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A commit message for what's staged in `dir`.
pub fn commit_message(writer: &Writer, dir: &Path) -> Result<String, String> {
    let diff = git(dir, &["diff", "--cached", "--no-color"])?;
    if diff.trim().is_empty() {
        return Err("Nothing is staged".into());
    }
    let log = git(dir, &["log", "-12", "--format=%s"]).unwrap_or_default();
    write(writer, dir, &commit_prompt(&diff, &log))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tidies_replies() {
        assert_eq!(tidy("\n  feat: add x \n"), "feat: add x");
        assert_eq!(tidy("```text\nfix: y\n\nbody\n```"), "fix: y\n\nbody");
        assert_eq!(tidy("\"docs: z\""), "docs: z");
    }

    #[test]
    fn clips_long_diffs_at_a_line() {
        let text = "aaaa\nbbbb\ncccc\n";
        assert_eq!(clip(text, 100), text);
        let cut = clip(text, 7);
        assert!(cut.starts_with("aaaa\n[… diff cut:"), "{cut}");
    }

    #[test]
    fn a_configured_writer_with_a_path() {
        let w = writer(Some("/usr/bin/env llm -m small")).unwrap();
        assert_eq!(w.program, PathBuf::from("/usr/bin/env"));
        assert_eq!(w.args, ["llm", "-m", "small"]);
        assert_eq!(w.name(), "env");
    }

    #[test]
    fn writes_with_the_prompt_last() {
        // `echo` stands in for an agent: it says back its arguments.
        let w = Writer {
            program: "/bin/echo".into(),
            args: vec!["reply:".into()],
        };
        let out = write(&w, Path::new("/"), "hello").unwrap();
        assert_eq!(out, "reply: hello");
    }
}
