//! Thin wrappers around the `git` command for setup and checks (Spec 06 §5, §7).
//! Airlock never uses git to locate its state.

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Runs git in `dir`; `None` if git is unavailable or `dir` is not in a
/// repository, `Err(stderr)` if the command failed.
pub fn query(dir: &Path, args: &[&str]) -> Option<Result<String, String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if output.status.success() {
        Some(Ok(String::from_utf8_lossy(&output.stdout).into_owned()))
    } else if stderr.contains("not a git repository") {
        None
    } else {
        Some(Err(stderr))
    }
}

/// Runs git in `dir` and fails with git's message if it does not succeed.
pub fn run(dir: &Path, args: &[&str]) -> Result<String> {
    match query(dir, args) {
        Some(Ok(stdout)) => Ok(stdout),
        Some(Err(stderr)) => bail!("git {} failed: {}", args.join(" "), stderr.trim()),
        None => bail!(
            "{} is not inside a git repository (or git is not installed)",
            dir.display()
        ),
    }
}

/// All worktrees of the repository containing `dir`; the main worktree comes first.
pub fn worktrees(dir: &Path) -> Option<Vec<PathBuf>> {
    let text = query(dir, &["worktree", "list", "--porcelain"])?.ok()?;
    Some(
        text.lines()
            .filter_map(|l| l.strip_prefix("worktree "))
            .map(PathBuf::from)
            .collect(),
    )
}

pub fn has_commit(dir: &Path) -> bool {
    matches!(
        query(dir, &["rev-parse", "--verify", "--quiet", "HEAD"]),
        Some(Ok(_))
    )
}

pub fn branch_exists(dir: &Path, branch: &str) -> bool {
    let reference = format!("refs/heads/{branch}");
    matches!(
        query(dir, &["rev-parse", "--verify", "--quiet", &reference]),
        Some(Ok(_))
    )
}

/// Whether exactly one remote has a branch of this name; `git worktree add <path> <branch>`
/// then creates a local tracking branch from it.
pub fn remote_branch_exists(dir: &Path, branch: &str) -> bool {
    let Some(Ok(refs)) = query(
        dir,
        &["for-each-ref", "--format=%(refname)", "refs/remotes"],
    ) else {
        return false;
    };
    // refs/remotes/<remote>/<branch>: compare the whole branch part, so `origin/feature/engine`
    // does not count as `engine`.
    refs.lines()
        .filter_map(|r| r.strip_prefix("refs/remotes/")?.split_once('/'))
        .filter(|(_, name)| *name == branch)
        .count()
        == 1
}
