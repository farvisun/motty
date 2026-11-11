//! Pull requests through the forge's own CLI, logged in already: `gh` for
//! GitHub, `glab` for GitLab (merge requests). Motty holds no tokens. The
//! Git tab shows the branch's request and its checks, makes one (title and
//! body written by the coding agent, after the repository's template), and
//! merges it.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Forge {
    GitHub,
    GitLab,
}

impl Forge {
    /// What the forge calls it.
    pub fn noun(self) -> &'static str {
        match self {
            Forge::GitHub => "Pull Request",
            Forge::GitLab => "Merge Request",
        }
    }

    fn program(self) -> &'static str {
        match self {
            Forge::GitHub => "gh",
            Forge::GitLab => "glab",
        }
    }

    fn cli(self) -> Option<PathBuf> {
        crate::agents::locate(self.program())
    }
}

/// The forge for a remote URL, when its CLI is installed: GitHub for
/// github.com, else GitLab (self-hosted ones too) when `glab` is there.
pub fn forge_for(remote: &str) -> Option<Forge> {
    let forge = forge_of(remote);
    forge.cli().map(|_| forge)
}

fn forge_of(remote: &str) -> Forge {
    if remote.contains("github.com") || remote.contains("github:") {
        Forge::GitHub
    } else {
        Forge::GitLab
    }
}

/// How a request's checks stand.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Checks {
    pub passed: u32,
    pub failed: u32,
    pub pending: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pr {
    pub number: u64,
    pub title: String,
    pub url: String,
    /// `open`, `draft`, `merged` or `closed`.
    pub state: String,
    pub checks: Checks,
}

impl Pr {
    pub fn is_open(&self) -> bool {
        matches!(self.state.as_str(), "open" | "draft")
    }
}

fn run(program: &Path, dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .current_dir(dir)
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1")
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err.trim().lines().last().unwrap_or("failed").to_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    run(Path::new("git"), dir, args).map(|s| s.trim().to_owned())
}

/// A GitHub `statusCheckRollup` tallied.
fn github_checks(rollup: &Value) -> Checks {
    let mut checks = Checks::default();
    for check in rollup.as_array().into_iter().flatten() {
        // Check runs have a status and a conclusion; commit statuses a state.
        let outcome = check["conclusion"]
            .as_str()
            .filter(|c| !c.is_empty())
            .or_else(|| check["state"].as_str())
            .unwrap_or("")
            .to_ascii_uppercase();
        match outcome.as_str() {
            "SUCCESS" | "NEUTRAL" | "SKIPPED" => checks.passed += 1,
            "FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED" => {
                checks.failed += 1
            }
            _ => checks.pending += 1,
        }
    }
    checks
}

/// `gh pr view --json …` read.
pub fn parse_github(json: &str) -> Option<Pr> {
    let v: Value = serde_json::from_str(json).ok()?;
    let state = match (
        v["state"].as_str()?,
        v["isDraft"].as_bool().unwrap_or(false),
    ) {
        ("OPEN", true) => "draft",
        ("OPEN", false) => "open",
        ("MERGED", _) => "merged",
        _ => "closed",
    };
    Some(Pr {
        number: v["number"].as_u64()?,
        title: v["title"].as_str()?.to_owned(),
        url: v["url"].as_str()?.to_owned(),
        state: state.to_owned(),
        checks: github_checks(&v["statusCheckRollup"]),
    })
}

/// `glab mr view -F json` read; its pipeline counts as one check.
pub fn parse_gitlab(json: &str) -> Option<Pr> {
    let v: Value = serde_json::from_str(json).ok()?;
    let draft = v["draft"].as_bool().or(v["work_in_progress"].as_bool());
    let state = match (v["state"].as_str()?, draft.unwrap_or(false)) {
        ("opened", true) => "draft",
        ("opened", false) => "open",
        ("merged", _) => "merged",
        _ => "closed",
    };
    let mut checks = Checks::default();
    match v["head_pipeline"]["status"].as_str() {
        Some("success") => checks.passed = 1,
        Some("failed" | "canceled") => checks.failed = 1,
        Some(_) => checks.pending = 1,
        None => {}
    }
    Some(Pr {
        number: v["iid"].as_u64()?,
        title: v["title"].as_str()?.to_owned(),
        url: v["web_url"].as_str()?.to_owned(),
        state: state.to_owned(),
        checks,
    })
}

/// The branch's request, `None` when it has none.
pub fn status(forge: Forge, dir: &Path) -> Result<Option<Pr>, String> {
    let cli = forge.cli().ok_or("not installed")?;
    let out = match forge {
        Forge::GitHub => run(
            &cli,
            dir,
            &[
                "pr",
                "view",
                "--json",
                "number,title,url,state,isDraft,statusCheckRollup",
            ],
        ),
        Forge::GitLab => run(&cli, dir, &["mr", "view", "-F", "json"]),
    };
    match out {
        Ok(json) => Ok(match forge {
            Forge::GitHub => parse_github(&json),
            Forge::GitLab => parse_gitlab(&json),
        }),
        // No request for the branch is no error.
        Err(e)
            if e.to_lowercase().contains("no pull requests found")
                || e.to_lowercase().contains("no open merge request")
                || e.to_lowercase().contains("not found") =>
        {
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// The branch a request would go into: the remote's default branch.
pub fn base_branch(dir: &Path) -> String {
    git(
        dir,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .ok()
    .and_then(|r| r.strip_prefix("origin/").map(str::to_owned))
    .unwrap_or_else(|| "main".to_owned())
}

/// The repository's request template, when it has one.
pub fn template(dir: &Path) -> Option<String> {
    let root = git(dir, &["rev-parse", "--show-toplevel"]).ok()?;
    let root = Path::new(&root);
    [
        ".github/pull_request_template.md",
        ".github/PULL_REQUEST_TEMPLATE.md",
        "docs/pull_request_template.md",
        "pull_request_template.md",
        ".gitlab/merge_request_templates/Default.md",
        ".gitlab/merge_request_templates/default.md",
    ]
    .iter()
    .find_map(|p| std::fs::read_to_string(root.join(p)).ok())
}

/// The prompt for a request's title and body.
pub fn prompt(noun: &str, base: &str, log: &str, diff: &str, template: Option<&str>) -> String {
    let template = template.map_or_else(String::new, |t| {
        format!("\n\nFill in the repository's template for the body:\n{t}")
    });
    format!(
        "Write the title and description of a {noun} that merges this branch into \
         {base}. Reply with the title on the first line (under 72 characters, in the \
         style of the commit subjects), then a blank line, then the description in \
         Markdown: what changed and why, and how it was tested when the commits say. \
         No code fences around the reply, no commentary, and no AI attribution or \
         \"generated by\" lines.{template}\n\nCommits:\n{log}\n\nDiff:\n{}",
        crate::agents::ai::clip(diff, crate::agents::ai::MAX_DIFF)
    )
}

/// A reply's first line as the title, the rest as the body.
pub fn split_reply(reply: &str) -> (String, String) {
    let reply = reply.trim();
    let (title, body) = reply.split_once('\n').unwrap_or((reply, ""));
    let title = title.trim().trim_start_matches('#').trim().to_owned();
    (title, body.trim().to_owned())
}

/// Title and body for this branch, written by the coding agent.
pub fn write(
    writer: &crate::agents::ai::Writer,
    forge: Forge,
    dir: &Path,
) -> Result<(String, String), String> {
    let base = base_branch(dir);
    let range = format!("origin/{base}...HEAD");
    let log = git(
        dir,
        &["log", &format!("origin/{base}..HEAD"), "--format=- %s%n%b"],
    )?;
    if log.trim().is_empty() {
        return Err(format!("No commits ahead of {base}"));
    }
    let diff = git(dir, &["diff", "--no-color", &range])?;
    let text = prompt(forge.noun(), &base, &log, &diff, template(dir).as_deref());
    let reply = crate::agents::ai::write(writer, dir, &text)?;
    let (title, body) = split_reply(&reply);
    if title.is_empty() {
        return Err("The agent wrote no title".into());
    }
    Ok((title, body))
}

/// Pushes the branch and opens the request; its URL.
pub fn create(forge: Forge, dir: &Path, title: &str, body: &str) -> Result<String, String> {
    let cli = forge.cli().ok_or("not installed")?;
    git(dir, &["push", "-u", "origin", "HEAD"])?;
    let base = base_branch(dir);
    let out = match forge {
        Forge::GitHub => run(
            &cli,
            dir,
            &[
                "pr", "create", "--base", &base, "--title", title, "--body", body,
            ],
        )?,
        Forge::GitLab => run(
            &cli,
            dir,
            &[
                "mr",
                "create",
                "--target-branch",
                &base,
                "--title",
                title,
                "--description",
                body,
                "--yes",
            ],
        )?,
    };
    Ok(out
        .lines()
        .rev()
        .find(|l| l.contains("://"))
        .unwrap_or_default()
        .trim()
        .to_owned())
}

/// Merges the branch's request: on GitHub the way the repository allows
/// (a merge commit, else squash, else rebase).
pub fn merge(forge: Forge, dir: &Path) -> Result<(), String> {
    let cli = forge.cli().ok_or("not installed")?;
    match forge {
        Forge::GitHub => {
            let allowed = run(
                &cli,
                dir,
                &[
                    "repo",
                    "view",
                    "--json",
                    "mergeCommitAllowed,squashMergeAllowed,rebaseMergeAllowed",
                ],
            )?;
            let v: Value = serde_json::from_str(&allowed).map_err(|e| e.to_string())?;
            let how = [
                ("mergeCommitAllowed", "--merge"),
                ("squashMergeAllowed", "--squash"),
                ("rebaseMergeAllowed", "--rebase"),
            ]
            .into_iter()
            .find(|(key, _)| v[key].as_bool() == Some(true))
            .map_or("--merge", |(_, flag)| flag);
            run(&cli, dir, &["pr", "merge", how])?;
        }
        Forge::GitLab => {
            run(&cli, dir, &["mr", "merge", "--yes"])?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forges_by_remote() {
        assert_eq!(forge_of("git@github.com:me/x.git"), Forge::GitHub);
        assert_eq!(
            forge_of("https://repo.example.org/team/x.git"),
            Forge::GitLab
        );
    }

    #[test]
    fn reads_github() {
        let json = r#"{"number":14,"title":"feat: x","url":"https://github.com/a/b/pull/14",
            "state":"OPEN","isDraft":false,"statusCheckRollup":[
              {"__typename":"CheckRun","status":"COMPLETED","conclusion":"SUCCESS"},
              {"__typename":"CheckRun","status":"COMPLETED","conclusion":"FAILURE"},
              {"__typename":"CheckRun","status":"IN_PROGRESS","conclusion":""},
              {"__typename":"StatusContext","state":"SUCCESS"}]}"#;
        let pr = parse_github(json).unwrap();
        assert_eq!((pr.number, pr.state.as_str()), (14, "open"));
        assert_eq!(
            pr.checks,
            Checks {
                passed: 2,
                failed: 1,
                pending: 1
            }
        );
        assert!(pr.is_open());
    }

    #[test]
    fn reads_gitlab() {
        let json = r#"{"iid":7,"title":"Fix y","web_url":"https://gl/x/-/merge_requests/7",
            "state":"merged","draft":false,"head_pipeline":{"status":"success"}}"#;
        let pr = parse_gitlab(json).unwrap();
        assert_eq!((pr.number, pr.state.as_str()), (7, "merged"));
        assert_eq!(pr.checks.passed, 1);
        assert!(!pr.is_open());
    }

    #[test]
    fn splits_title_and_body() {
        let (t, b) = split_reply("# feat: add x\n\n## Summary\n- one\n");
        assert_eq!(t, "feat: add x");
        assert_eq!(b, "## Summary\n- one");
        assert_eq!(split_reply("only a title").1, "");
    }

    #[test]
    fn prompts_with_the_template() {
        let p = prompt("Pull Request", "main", "- feat: x", "diff", Some("## Why"));
        assert!(p.contains("into main") && p.contains("## Why") && p.contains("- feat: x"));
    }
}
