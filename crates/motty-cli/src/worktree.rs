//! `motty worktree`: git worktrees for running agents side by side. New
//! ones go in the repository's `.worktrees/` folder (kept out of git with
//! `info/exclude`) and open in a new tab. `.motty/worktree.toml` in the
//! main checkout can name a `setup` script, run in each new worktree, and a
//! `teardown` script, run before one is removed; both run only once the
//! user approved the file as it is (see `motty_theme::trust`).

use std::io::{BufRead as _, IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::{Context as _, bail};
use clap::Subcommand;
use serde::{Deserialize, Serialize};

use crate::output::Ctx;

/// The folder in the main checkout that holds new worktrees.
pub const DIR: &str = ".worktrees";
/// The scripts' file, in the main checkout.
pub const SCRIPTS: &str = ".motty/worktree.toml";

#[derive(Subcommand)]
pub enum WorktreeCmd {
    /// The repository's worktrees.
    List,
    /// A worktree for a branch (made from --from, else HEAD, when it doesn't
    /// exist yet) in `.worktrees/`, opened in a new tab that runs the setup
    /// script.
    New {
        branch: String,
        /// Where a new branch starts.
        #[arg(long, value_name = "REF")]
        from: Option<String>,
        /// Don't run the setup script.
        #[arg(long)]
        no_setup: bool,
        /// Don't open a tab; run the setup script here.
        #[arg(long)]
        no_tab: bool,
    },
    /// Remove a worktree (by branch or path; default: the one this runs
    /// in) after its teardown script. Refused while it has uncommitted
    /// changes, unless --force. The branch is kept.
    Remove {
        target: Option<String>,
        #[arg(long)]
        force: bool,
        /// Don't run the teardown script.
        #[arg(long)]
        no_teardown: bool,
    },
    /// Show `.motty/worktree.toml`'s scripts and approve them as they are.
    Trust,
}

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Scripts {
    pub setup: Option<String>,
    pub teardown: Option<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub main: bool,
}

pub fn run(ctx: &Ctx, cmd: WorktreeCmd) -> anyhow::Result<ExitCode> {
    let here = std::env::current_dir()?;
    match cmd {
        WorktreeCmd::List => {
            let all = list(&here)?;
            ctx.print(&all, || {
                for w in &all {
                    let branch = w.branch.as_deref().unwrap_or("(detached)");
                    let mark = if w.main { " (main)" } else { "" };
                    println!("{}\t{branch}{mark}", w.path.display());
                }
            });
        }
        WorktreeCmd::New {
            branch,
            from,
            no_setup,
            no_tab,
        } => new(ctx, &here, &branch, from.as_deref(), no_setup, no_tab)?,
        WorktreeCmd::Remove {
            target,
            force,
            no_teardown,
        } => remove(ctx, &here, target.as_deref(), force, no_teardown)?,
        WorktreeCmd::Trust => {
            let root = main_root(&here)?;
            let file = root.join(SCRIPTS);
            let contents =
                std::fs::read(&file).with_context(|| format!("no {}", file.display()))?;
            parse_scripts(&contents)?;
            println!(
                "{}:\n{}",
                file.display(),
                String::from_utf8_lossy(&contents)
            );
            if ctx.yes || ask("Approve these scripts?")? {
                motty_theme::trust::trust(&file, &contents)?;
                ctx.note("Approved.");
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn git(dir: &Path, args: &[&str]) -> anyhow::Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .context("couldn't run git")?;
    if !out.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_owned())
}

/// `git worktree list --porcelain`, read.
pub fn parse_list(porcelain: &str) -> Vec<Worktree> {
    let mut all: Vec<Worktree> = Vec::new();
    for line in porcelain.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            all.push(Worktree {
                path: PathBuf::from(path),
                branch: None,
                main: all.is_empty(),
            });
        } else if let (Some(b), Some(last)) = (line.strip_prefix("branch "), all.last_mut()) {
            last.branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_owned());
        }
    }
    all
}

fn list(dir: &Path) -> anyhow::Result<Vec<Worktree>> {
    Ok(parse_list(&git(dir, &["worktree", "list", "--porcelain"])?))
}

/// The main checkout: the first worktree git lists.
fn main_root(dir: &Path) -> anyhow::Result<PathBuf> {
    list(dir)?
        .into_iter()
        .next()
        .map(|w| w.path)
        .context("not in a git repository")
}

/// The folder name for a branch: `feat/login` is `feat-login`.
pub fn folder_for(branch: &str) -> String {
    branch
        .chars()
        .map(|c| if c == '/' || c == '\\' { '-' } else { c })
        .collect()
}

/// Single-quoted for sh.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub fn parse_scripts(contents: &[u8]) -> anyhow::Result<Scripts> {
    let text = std::str::from_utf8(contents).context("worktree.toml isn't UTF-8")?;
    toml::from_str(text).with_context(|| format!("couldn't read {SCRIPTS}"))
}

/// The main checkout's scripts, when it has any; approval is asked for
/// here when needed. `None` when there are none or they weren't approved.
fn approved_scripts(ctx: &Ctx, root: &Path) -> anyhow::Result<Option<Scripts>> {
    let file = root.join(SCRIPTS);
    let Ok(contents) = std::fs::read(&file) else {
        return Ok(None);
    };
    let scripts = parse_scripts(&contents)?;
    if scripts.setup.is_none() && scripts.teardown.is_none() {
        return Ok(None);
    }
    if motty_theme::trust::is_trusted(&file, &contents) {
        return Ok(Some(scripts));
    }
    if !std::io::stdin().is_terminal() {
        eprintln!(
            "motty: {} isn't approved; skipping its scripts (approve with `motty worktree trust`)",
            file.display()
        );
        return Ok(None);
    }
    eprintln!("{} asks to run:", file.display());
    for (name, script) in [("setup", &scripts.setup), ("teardown", &scripts.teardown)] {
        if let Some(script) = script {
            eprintln!("  {name}: {script}");
        }
    }
    if ask("Run them now and when they're needed again, until the file changes?")? {
        motty_theme::trust::trust(&file, &contents)?;
        Ok(Some(scripts))
    } else {
        ctx.note("Skipping the scripts.");
        Ok(None)
    }
}

fn ask(question: &str) -> anyhow::Result<bool> {
    eprint!("{question} [y/N] ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes"))
}

/// Keeps `.worktrees/` out of `git status` without touching `.gitignore`.
fn exclude(root: &Path) -> anyhow::Result<()> {
    let common = git(
        root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let file = Path::new(&common).join("info").join("exclude");
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let line = format!("/{DIR}/");
    if text.lines().any(|l| l.trim() == line) {
        return Ok(());
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let sep = if text.is_empty() || text.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    std::fs::write(&file, format!("{text}{sep}{line}\n"))?;
    Ok(())
}

/// The command line that runs a script in a worktree, with its context in
/// the environment.
pub fn script_line(script: &str, root: &Path, worktree: &Path, branch: &str) -> String {
    format!(
        "env MOTTY_WORKTREE_ROOT={} MOTTY_WORKTREE={} MOTTY_WORKTREE_BRANCH={} sh -c {}",
        quote(&root.display().to_string()),
        quote(&worktree.display().to_string()),
        quote(branch),
        quote(script)
    )
}

fn run_script(line: &str, dir: &Path) -> anyhow::Result<()> {
    let status = Command::new("sh")
        .arg("-c")
        .arg(line)
        .current_dir(dir)
        .status()?;
    if !status.success() {
        bail!("the script failed ({status})");
    }
    Ok(())
}

fn new(
    ctx: &Ctx,
    here: &Path,
    branch: &str,
    from: Option<&str>,
    no_setup: bool,
    no_tab: bool,
) -> anyhow::Result<()> {
    let root = main_root(here)?;
    let path = root.join(DIR).join(folder_for(branch));
    if path.exists() {
        bail!("{} already exists", path.display());
    }
    exclude(&root)?;
    let target = path.display().to_string();
    let exists = git(
        &root,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok();
    if exists {
        git(&root, &["worktree", "add", &target, branch])?;
    } else {
        let mut args = vec!["worktree", "add", "-b", branch, &target];
        args.extend(from);
        git(&root, &args)?;
    }
    ctx.note(format!("Created {target} ({branch})"));
    let setup = if no_setup {
        None
    } else {
        approved_scripts(ctx, &root)?.and_then(|s| s.setup)
    };
    let line = setup.map(|s| script_line(&s, &root, &path, branch));
    if !no_tab && open_tab(ctx, &path, line.as_deref(), branch).is_ok() {
        return Ok(());
    }
    if let Some(line) = &line {
        run_script(line, &path)?;
    }
    if no_tab {
        ctx.note(format!("cd {}", quote(&target)));
    }
    Ok(())
}

fn open_tab(ctx: &Ctx, path: &Path, line: Option<&str>, branch: &str) -> anyhow::Result<()> {
    let mut client = ctx.connect()?;
    let params = motty_ipc::NewTabParams {
        cwd: Some(path.display().to_string()),
        select_window: crate::tree::own_window(&mut client)?,
        line: line.map(str::to_owned),
        title: Some(branch.to_owned()),
        ..Default::default()
    };
    let _: serde_json::Value = client.call(motty_ipc::method::NEW_TAB, &params)?;
    Ok(())
}

fn remove(
    ctx: &Ctx,
    here: &Path,
    target: Option<&str>,
    force: bool,
    no_teardown: bool,
) -> anyhow::Result<()> {
    let all = list(here)?;
    let root = all
        .first()
        .map(|w| w.path.clone())
        .context("not in a git repository")?;
    let wanted = match target {
        Some(t) => {
            let as_path = std::fs::canonicalize(t).ok();
            all.iter().find(|w| {
                w.branch.as_deref() == Some(t)
                    || as_path.as_ref() == std::fs::canonicalize(&w.path).ok().as_ref()
            })
        }
        None => {
            let top = git(here, &["rev-parse", "--show-toplevel"])?;
            all.iter().find(|w| w.path == Path::new(&top))
        }
    };
    let Some(worktree) = wanted else {
        bail!("no worktree {}", target.unwrap_or("here"));
    };
    if worktree.main {
        bail!(
            "{} is the main checkout, not a worktree",
            worktree.path.display()
        );
    }
    let path = worktree.path.clone();
    let branch = worktree.branch.clone().unwrap_or_default();
    if !force && !git(&path, &["status", "--porcelain"])?.is_empty() {
        bail!(
            "{} has uncommitted changes; commit them, or pass --force to lose them",
            path.display()
        );
    }
    if !no_teardown && let Some(teardown) = approved_scripts(ctx, &root)?.and_then(|s| s.teardown) {
        run_script(&script_line(&teardown, &root, &path, &branch), &path)?;
    }
    let target = path.display().to_string();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&target);
    git(&root, &args)?;
    ctx.note(format!("Removed {target}; the branch {branch} is kept"));
    if here.starts_with(&path) {
        ctx.note(format!(
            "This folder is gone: cd {}",
            quote(&root.display().to_string())
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_worktree_list() {
        let porcelain = "worktree /r\nHEAD 1\nbranch refs/heads/main\n\n\
                         worktree /r/.worktrees/feat-x\nHEAD 2\nbranch refs/heads/feat/x\n\n\
                         worktree /r/.worktrees/old\nHEAD 3\ndetached\n";
        let all = parse_list(porcelain);
        assert_eq!(all.len(), 3);
        assert!(all[0].main && !all[1].main);
        assert_eq!(all[1].branch.as_deref(), Some("feat/x"));
        assert_eq!(all[2].branch, None);
    }

    #[test]
    fn names_and_quotes() {
        assert_eq!(folder_for("feat/login"), "feat-login");
        assert_eq!(quote("it's"), "'it'\\''s'");
        let line = script_line("pnpm i", Path::new("/r"), Path::new("/r/.worktrees/x"), "x");
        assert_eq!(
            line,
            "env MOTTY_WORKTREE_ROOT='/r' MOTTY_WORKTREE='/r/.worktrees/x' \
             MOTTY_WORKTREE_BRANCH='x' sh -c 'pnpm i'"
        );
    }

    #[test]
    fn scripts_file() {
        let s = parse_scripts(b"setup = \"pnpm i\"\nteardown = \"docker compose down\"").unwrap();
        assert_eq!(s.setup.as_deref(), Some("pnpm i"));
        assert!(parse_scripts(b"setpu = \"x\"").is_err());
    }
}
