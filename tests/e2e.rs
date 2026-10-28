//! End-to-end: the real binary against a temporary hub (Spec 02).

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

struct Env {
    _tmp: TempDir,
    hub: PathBuf,
    work: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let hub = tmp.path().join("hub");
        let work = tmp.path().join("work");
        std::fs::create_dir_all(work.join("project")).unwrap();
        Self {
            _tmp: tmp,
            hub,
            work,
        }
    }

    fn dir(&self, name: &str) -> PathBuf {
        let dir = self.work.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn cmd(&self, cwd: &Path, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_airlock"));
        cmd.args(args)
            .current_dir(cwd)
            .env("AIRLOCK_HOME", &self.hub)
            .env_remove("AIRLOCK_PROJECT")
            .env_remove("AIRLOCK_SESSION");
        cmd
    }

    fn run(&self, cwd: &Path, args: &[&str]) -> Output {
        self.cmd(cwd, args).output().expect("run airlock")
    }

    fn ok(&self, cwd: &Path, args: &[&str]) -> String {
        let out = self.run(cwd, args);
        assert!(
            out.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn fail(&self, cwd: &Path, args: &[&str]) -> String {
        let out = self.run(cwd, args);
        assert_eq!(out.status.code(), Some(1), "{args:?} should fail");
        String::from_utf8(out.stderr).unwrap()
    }

    fn json(&self, cwd: &Path, args: &[&str]) -> Value {
        let mut args = args.to_vec();
        args.push("--json");
        serde_json::from_str(&self.ok(cwd, &args)).expect("valid JSON")
    }
}

fn add(env: &Env, cwd: &Path, id: &str, extra: &[&str]) {
    let mut args = vec![
        "task", "add", id, "Title", "--role", "engine", "--do", "x", "--verify", "y",
    ];
    args.extend_from_slice(extra);
    env.ok(cwd, &args);
}

#[test]
fn init_writes_marker_and_is_idempotent() {
    let env = Env::new();
    let project = env.dir("project");
    let created = env.json(&project, &["init"]);
    assert_eq!(created["created"], true);
    assert_eq!(created["name"], "project");

    let marker: Value =
        serde_json::from_str(&std::fs::read_to_string(project.join(".airlock")).unwrap()).unwrap();
    assert_eq!(marker["project_id"], created["project_id"]);

    let again = env.json(&project, &["init"]);
    assert_eq!(again["created"], false);
    assert_eq!(again["project_id"], created["project_id"]);

    let nested = project.join("src");
    std::fs::create_dir_all(&nested).unwrap();
    assert!(env.fail(&nested, &["init"]).contains("already inside"));
}

#[test]
fn init_adopts_a_marker_from_another_machine() {
    let env = Env::new();
    let clone = env.dir("clone");
    std::fs::write(
        clone.join(".airlock"),
        r#"{"project_id": "11111111-2222-4333-8444-555555555555"}"#,
    )
    .unwrap();
    std::fs::create_dir_all(clone.join("docs")).unwrap();
    std::fs::write(clone.join("docs/.airlock-state.json"), "{}").unwrap();

    assert!(
        env.fail(&clone, &["task", "list"])
            .contains("run `airlock init` there")
    );
    let out = env.ok(&clone, &["init", "--name", "game"]);
    assert!(
        out.contains("airlock import"),
        "hints at the snapshot: {out}"
    );
    let again = env.json(&clone, &["init"]);
    assert_eq!(again["project_id"], "11111111-2222-4333-8444-555555555555");
}

#[test]
fn worktrees_share_state_and_project_flag_works_anywhere() {
    let env = Env::new();
    let main = env.dir("main");
    env.ok(&main, &["init", "--name", "game"]);
    let worktree = env.dir("game-engine");
    std::fs::copy(main.join(".airlock"), worktree.join(".airlock")).unwrap();

    add(&env, &main, "E-1", &[]);
    env.ok(
        &worktree,
        &["task", "claim", "E-1", "--session", "engine-1"],
    );
    let task = env.json(&main, &["task", "show", "E-1"]);
    assert_eq!(task["claimed_by"], "engine-1");
    assert_eq!(task["status"], "in_progress");

    let outside = env.dir("elsewhere");
    assert!(
        env.fail(&outside, &["task", "list"])
            .contains("not inside an Airlock project")
    );
    let listed = env.json(&outside, &["--project", "game", "task", "list"]);
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert!(
        env.fail(&outside, &["--project", "nope", "task", "list"])
            .contains("no project")
    );
}

#[test]
fn task_flow_with_json_output() {
    let env = Env::new();
    let project = env.dir("project");
    env.ok(&project, &["init"]);
    add(&env, &project, "L-1", &[]);
    add(
        &env,
        &project,
        "E-1",
        &["--blocked-by", "L-1", "--priority", "1"],
    );
    add(&env, &project, "E-2", &["--priority", "5"]);

    let list = env.json(&project, &["task", "list", "--status", "blocked"]);
    assert_eq!(list[0]["id"], "E-1");
    assert_eq!(
        env.json(&project, &["task", "next", "--role", "engine"])["id"],
        "E-2"
    );

    env.ok(&project, &["--session", "s1", "task", "claim", "L-1"]);
    let done = env.json(&project, &["--session", "s1", "task", "done", "L-1"]);
    assert_eq!(done["now_ready"], serde_json::json!(["E-1"]));

    assert!(
        env.fail(&project, &["task", "next", "--role", "nope"])
            .contains("unknown role")
    );
    assert!(
        env.fail(&project, &["sync"])
            .contains("not implemented yet")
    );
}

#[test]
fn session_defaults_to_environment() {
    let env = Env::new();
    let project = env.dir("project");
    env.ok(&project, &["init"]);
    add(&env, &project, "E-1", &[]);

    let out = env
        .cmd(&project, &["task", "claim", "E-1"])
        .env("AIRLOCK_SESSION", "agent-7")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        env.json(&project, &["task", "show", "E-1"])["claimed_by"],
        "agent-7"
    );
}

#[test]
fn concurrent_claims_from_separate_processes_have_one_winner() {
    let env = Env::new();
    let project = env.dir("project");
    env.ok(&project, &["init"]);
    add(&env, &project, "E-1", &[]);

    let children: Vec<_> = (0..8)
        .map(|i| {
            env.cmd(
                &project,
                &["task", "claim", "E-1", "--session", &format!("s{i}")],
            )
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
        })
        .collect();
    let outputs: Vec<Output> = children
        .into_iter()
        .map(|c| c.wait_with_output().unwrap())
        .collect();

    assert_eq!(outputs.iter().filter(|o| o.status.success()).count(), 1);
    for loser in outputs.iter().filter(|o| !o.status.success()) {
        let stderr = String::from_utf8_lossy(&loser.stderr);
        assert!(
            stderr.contains("claimed by"),
            "unexpected failure: {stderr}"
        );
    }
}
