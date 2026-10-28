//! Guided init (Spec 06 §7): templates, layout, worktrees, bindings, adoption.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

struct Env {
    tmp: TempDir,
    hub: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let hub = tmp.path().join("hub");
        Self { tmp, hub }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.tmp.path().join(rel)
    }

    /// A git repository with one empty commit.
    fn repo(&self, rel: &str) -> PathBuf {
        let dir = self.path(rel);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["commit", "-q", "--allow-empty", "-m", "init"]);
        dir
    }

    fn run_with(&self, cwd: &Path, hub: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_airlock"))
            .args(args)
            .current_dir(cwd)
            .env("AIRLOCK_HOME", hub)
            .env("HOME", self.path("home"))
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env_remove("AIRLOCK_PROJECT")
            .env_remove("AIRLOCK_SESSION")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn ok(&self, cwd: &Path, args: &[&str]) -> String {
        let out = self.run_with(cwd, &self.hub, args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn fail(&self, cwd: &Path, args: &[&str]) -> String {
        let out = self.run_with(cwd, &self.hub, args);
        assert!(!out.status.success(), "{args:?} should fail");
        String::from_utf8(out.stderr).unwrap()
    }

    fn json(&self, cwd: &Path, args: &[&str]) -> Value {
        let mut args = args.to_vec();
        args.push("--json");
        serde_json::from_str(&self.ok(cwd, &args)).unwrap()
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn actions(report: &Value) -> Vec<String> {
    report["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            format!(
                "{} {}",
                a["action"].as_str().unwrap(),
                a["detail"].as_str().unwrap()
            )
        })
        .collect()
}

#[test]
fn clean_room_template_sets_up_worktree_and_bindings() {
    let env = Env::new();
    let main = env.repo("game");
    std::fs::write(main.join("wip.txt"), "unrelated").unwrap();
    git(&main, &["add", "wip.txt"]);

    assert!(
        env.fail(&main, &["init", "--template", "clean-room"])
            .contains("pass --yes")
    );
    let report = env.json(&main, &["init", "--template", "clean-room", "--yes"]);
    assert_eq!(report["template"], "clean-room");
    let done = actions(&report);
    assert!(
        done.iter().any(|a| a == "committed .airlock, airlock.toml"),
        "{done:?}"
    );
    assert!(
        done.iter()
            .any(|a| a == "worktree created on branch engine"),
        "{done:?}"
    );
    assert!(
        done.iter()
            .any(|a| a == "bound role engine, session engine@game-engine (locked)"),
        "{done:?}"
    );

    // Only marker and layout were committed; the unrelated staged file stays staged.
    assert_eq!(
        git(&main, &["show", "--name-only", "--format=", "HEAD"]).trim(),
        ".airlock\nairlock.toml"
    );
    assert_eq!(git(&main, &["status", "--porcelain"]).trim(), "A  wip.txt");

    let engine = env.path("game-engine");
    assert!(engine.join(".airlock").is_file() && engine.join("airlock.toml").is_file());
    assert_eq!(env.json(&engine, &["use"])["role"], "engine");
    assert_eq!(env.json(&main, &["use"])["role"], "lead");
    let roles = env.json(&main, &["role", "list"]);
    assert!(
        roles["engine"]["instructions"]
            .as_str()
            .unwrap()
            .contains("Never read analysis material")
    );

    // Running again changes nothing.
    let again = env.json(&main, &["init"]);
    assert!(
        actions(&again)
            .iter()
            .all(|a| a.starts_with("bound already")),
        "{again}"
    );
    assert!(
        env.fail(&main, &["init", "--template", "parallel", "--yes"])
            .contains("already exists")
    );
}

#[test]
fn templates_need_a_repository_with_a_commit() {
    let env = Env::new();
    let plain = env.path("plain");
    std::fs::create_dir_all(&plain).unwrap();
    assert!(
        env.fail(&plain, &["init", "--template", "clean-room", "--yes"])
            .contains("needs a git repository")
    );

    let empty = env.path("empty");
    std::fs::create_dir_all(&empty).unwrap();
    git(&empty, &["init", "-q", "-b", "main"]);
    assert!(
        env.fail(&empty, &["init", "--template", "parallel", "--yes"])
            .contains("at least one commit")
    );

    assert_eq!(
        env.run_with(
            &plain,
            &env.hub,
            &["init", "--template", "parallel", "--agents", "0"]
        )
        .status
        .code(),
        Some(2)
    );
}

#[test]
fn parallel_template_gives_each_agent_a_worktree() {
    let env = Env::new();
    let main = env.repo("web");
    env.ok(
        &main,
        &["init", "--template", "parallel", "--agents", "2", "--yes"],
    );
    for n in [1, 2] {
        let dir = env.path(&format!("web-dev-{n}"));
        let binding = env.json(&dir, &["use"]);
        assert_eq!(binding["session"], format!("dev@web-dev-{n}"));
        assert_eq!(
            git(&dir, &["branch", "--show-current"]).trim(),
            format!("dev-{n}")
        );
    }
    assert_eq!(env.json(&main, &["use"])["role"], "lead");
}

#[test]
fn single_template_works_without_git() {
    let env = Env::new();
    let dir = env.path("notes");
    std::fs::create_dir_all(&dir).unwrap();
    assert!(
        env.fail(&dir, &["init", "--template", "single"])
            .contains("pass --yes")
    );
    let report = env.json(&dir, &["init", "--template", "single", "--yes"]);
    assert!(actions(&report).iter().all(|a| !a.starts_with("committed")));
    assert!(dir.join("airlock.toml").is_file());
    assert_eq!(env.json(&dir, &["use"])["role"], "dev");
}

#[test]
fn plain_init_does_not_commit() {
    let env = Env::new();
    let main = env.repo("plain");
    env.ok(&main, &["init"]);
    assert_eq!(git(&main, &["status", "--porcelain"]).trim(), "?? .airlock");
    assert!(!main.join("airlock.toml").exists());
}

#[test]
fn template_applies_to_an_existing_project_and_worktree() {
    let env = Env::new();
    let main = env.repo("shr");
    env.ok(&main, &["init"]);
    git(&main, &["add", ".airlock"]);
    git(&main, &["commit", "-q", "-m", "marker"]);
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "engine",
            env.path("shr-engine").to_str().unwrap(),
        ],
    );
    env.ok(&main, &["role", "set", "qa", "--description", "custom"]);

    let report = env.json(&main, &["init", "--template", "clean-room", "--yes"]);
    assert_eq!(report["created"], false);
    let done = actions(&report);
    assert!(
        done.iter().any(|a| a == "committed airlock.toml"),
        "{done:?}"
    );
    assert!(
        !done.iter().any(|a| a.starts_with("worktree")),
        "existing worktree reused: {done:?}"
    );
    assert_eq!(env.json(&env.path("shr-engine"), &["use"])["locked"], true);
    assert_eq!(
        env.json(&main, &["role", "list"])["qa"]["description"],
        "custom",
        "existing roles kept"
    );
}

#[test]
fn fresh_clone_recreates_worktrees_from_the_layout() {
    let env = Env::new();
    let origin = env.repo("origin/game");
    env.ok(&origin, &["init", "--template", "clean-room", "--yes"]);
    git(
        &env.path("origin/game-engine"),
        &["commit", "-q", "--allow-empty", "-m", "engine work"],
    );
    // A branch whose last segment matches must not hide `engine`.
    git(&origin, &["branch", "feature/engine"]);

    // Another machine: its own hub and a clone in a different directory.
    std::fs::create_dir_all(env.path("m2")).unwrap();
    git(
        &env.path("m2"),
        &["clone", "-q", origin.to_str().unwrap(), "game"],
    );
    let clone = env.path("m2/game");
    let hub2 = env.path("hub2");
    let out = env.run_with(&clone, &hub2, &["init", "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["adopted"], true);
    let done = actions(&report);
    assert!(
        done.iter()
            .any(|a| a == "worktree created on branch engine"),
        "{done:?}"
    );

    let engine = env.path("m2/game-engine");
    assert_eq!(
        git(&engine, &["log", "-1", "--format=%s"]).trim(),
        "engine work",
        "tracks origin/engine"
    );
    let use_out = env.run_with(&engine, &hub2, &["use", "--json"]);
    let binding: Value = serde_json::from_slice(&use_out.stdout).unwrap();
    assert_eq!(
        (binding["role"].as_str(), binding["locked"].as_bool()),
        (Some("engine"), Some(true))
    );
}

#[test]
fn foreign_directories_are_not_bound() {
    let env = Env::new();
    let main = env.repo("app");
    std::fs::create_dir_all(env.path("app-engine")).unwrap();
    let report = env.json(&main, &["init", "--template", "clean-room", "--yes"]);
    assert!(
        actions(&report)
            .iter()
            .any(|a| a.starts_with("skipped exists but is not a worktree")),
        "{report}"
    );
    assert_eq!(
        env.json(
            &env.path("app-engine"),
            &["--project", "app", "task", "list"]
        ),
        Value::Array(vec![])
    );
}
