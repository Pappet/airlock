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
}

#[test]
fn task_edit_changes_content_and_guards_started_tasks() {
    let env = Env::new();
    let project = env.dir("project");
    env.ok(&project, &["init"]);
    add(&env, &project, "L-1", &[]);
    add(&env, &project, "E-1", &["--touch", "src/old.rs"]);

    let edited = env.json(
        &project,
        &[
            "task",
            "edit",
            "E-1",
            "--title",
            "Renamed",
            "--touch",
            "src/new.rs",
            "--blocked-by",
            "L-1",
        ],
    );
    assert_eq!(edited["title"], "Renamed");
    assert_eq!(edited["touch"], serde_json::json!(["src/new.rs"]));
    assert_eq!(edited["status"], "blocked");
    assert!(
        env.fail(&project, &["task", "edit", "L-1", "--blocked-by", "E-1"])
            .contains("dependency cycle")
    );
    assert!(
        env.ok(&project, &["task", "edit", "E-1", "--clear-blocked-by"])
            .contains("Edited E-1")
    );
    assert_eq!(
        env.json(&project, &["task", "show", "E-1"])["status"],
        "ready"
    );

    assert!(
        env.fail(&project, &["task", "edit", "E-1"])
            .contains("nothing to change")
    );
    assert!(
        env.fail(&project, &["task", "edit", "E-1", "--role", "nope"])
            .contains("unknown role")
    );

    env.ok(&project, &["--session", "s1", "task", "claim", "E-1"]);
    assert!(
        env.fail(&project, &["task", "edit", "E-1", "--title", "x"])
            .contains("--force")
    );
    env.ok(
        &project,
        &["task", "edit", "E-1", "--title", "x", "--force"],
    );
}

#[test]
fn overlapping_work_is_handed_out_last_and_claims_ask_the_lead() {
    let env = Env::new();
    let project = env.dir("project");
    env.ok(&project, &["init"]);
    add(&env, &project, "E-1", &["--touch", "src/sim.rs"]);
    add(
        &env,
        &project,
        "E-2",
        &["--touch", "./src/sim.rs", "--priority", "9"],
    );
    add(&env, &project, "E-3", &["--touch", "src/other.rs"]);
    env.ok(&project, &["--session", "engine-2", "task", "claim", "E-1"]);

    // E-2 has the higher priority but touches the occupied src/sim.rs.
    let next = env.json(&project, &["task", "next", "--role", "engine"]);
    assert_eq!(next["id"], "E-3");
    assert!(next.get("overlaps").is_none(), "no warning for clean work");

    let clean = env.json(&project, &["--session", "engine-3", "task", "claim", "E-3"]);
    assert!(clean.get("overlaps").is_none() && clean.get("question_id").is_none());
    let out = env.run(&project, &["task", "next", "--role", "engine"]);
    assert!(out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("E-2"),
        "nothing clean left: E-2 anyway"
    );
    let warning = String::from_utf8_lossy(&out.stderr);
    assert!(
        warning.contains("warning: E-2") && warning.contains("src/sim.rs with E-1"),
        "{warning}"
    );

    let claimed = env.json(&project, &["--session", "engine-1", "task", "claim", "E-2"]);
    assert_eq!(claimed["overlaps"][0]["id"], "E-1");
    assert_eq!(
        claimed["overlaps"][0]["paths"],
        serde_json::json!(["src/sim.rs"])
    );
    let question_id = claimed["question_id"]
        .as_u64()
        .expect("question for the lead");
    let questions = env.json(&project, &["question", "list", "--status", "open"]);
    let q = &questions[0];
    assert_eq!(q["id"].as_u64(), Some(question_id));
    assert_eq!(
        (q["topic"].as_str(), q["asked_by"].as_str()),
        (Some("overlap:E-2"), Some("engine-1"))
    );
    let text = q["question"].as_str().unwrap();
    assert!(
        text.contains("engine-1 claimed E-2")
            && text.contains("src/sim.rs (E-1, in_progress by engine-2)")
            && text.contains("release E-2"),
        "{text}"
    );

    // Claiming again keeps the one open question.
    env.ok(
        &project,
        &["--session", "engine-1", "task", "release", "E-2"],
    );
    let out = env.run(&project, &["--session", "engine-1", "task", "claim", "E-2"]);
    let warning = String::from_utf8_lossy(&out.stderr);
    assert!(
        warning.contains(&format!("question #{question_id}")),
        "{warning}"
    );
    assert_eq!(
        env.json(&project, &["question", "list"])
            .as_array()
            .unwrap()
            .len(),
        1
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

#[test]
fn marker_ids_cannot_escape_the_hub() {
    let env = Env::new();
    let repo = env.dir("repo");
    std::fs::write(repo.join(".airlock"), r#"{"project_id": "../../escaped"}"#).unwrap();

    assert!(
        env.fail(&repo, &["init", "--name", "x"])
            .contains("invalid project id")
    );
    assert!(
        env.fail(&repo, &["task", "list"])
            .contains("invalid project id")
    );
    assert!(!env.work.parent().unwrap().join("escaped").exists());
    assert!(!env.hub.exists(), "nothing written to the hub");

    let outside = env.dir("outside");
    assert!(
        env.fail(&outside, &["--project", "../repo", "task", "list"])
            .contains("no project")
    );
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn project_with_tasks(env: &Env) -> PathBuf {
    let project = env.dir("project");
    env.ok(&project, &["init"]);
    add(env, &project, "L-1", &[]);
    add(env, &project, "E-1", &["--blocked-by", "L-1"]);
    env.ok(
        &project,
        &["question", "ask", "Front arc?", "--topic", "melee.md"],
    );
    project
}

#[test]
fn sync_is_byte_stable_and_ignores_session_state() {
    let env = Env::new();
    let project = project_with_tasks(&env);
    let report = env.json(&project, &["sync"]);
    assert_eq!(
        report["written"],
        serde_json::json!([
            "docs/.airlock-state.json",
            "docs/PLAN.md",
            "docs/spec/QUESTIONS.md"
        ])
    );
    let state = read(&project.join("docs/.airlock-state.json"));
    assert!(read(&project.join("docs/PLAN.md")).contains("### E-1"));

    env.ok(&project, &["task", "claim", "L-1"]);
    let again = env.json(&project, &["sync"]);
    assert_eq!(again["written"], serde_json::json!([]));
    assert_eq!(read(&project.join("docs/.airlock-state.json")), state);

    env.ok(&project, &["task", "done", "L-1"]);
    let changed = env.json(&project, &["sync"]);
    assert_eq!(
        changed["written"].as_array().unwrap().len(),
        2,
        "snapshot and plan, not questions"
    );
}

#[test]
fn sync_refuses_foreign_snapshots_but_accepts_its_own_history() {
    let env = Env::new();
    let project = project_with_tasks(&env);
    let state_path = project.join("docs/.airlock-state.json");
    env.ok(&project, &["sync"]);
    let first = read(&state_path);

    // Own older version (e.g. the other side of a merge): accepted.
    env.ok(
        &project,
        &["question", "ask", "Second?", "--topic", "melee.md"],
    );
    env.ok(&project, &["sync"]);
    std::fs::write(&state_path, &first).unwrap();
    env.ok(&project, &["sync"]);
    assert!(read(&state_path).contains("Second?"));

    // Foreign content: refused, then replaced with --force.
    std::fs::write(&state_path, read(&state_path).replace("Second?", "Edited?")).unwrap();
    assert!(
        env.fail(&project, &["sync"])
            .contains("not written by this hub")
    );
    assert!(read(&state_path).contains("Edited?"), "nothing overwritten");
    env.ok(&project, &["sync", "--force"]);
    assert!(read(&state_path).contains("Second?"));
}

#[test]
fn snapshot_moves_state_to_another_machine() {
    let env = Env::new();
    let project = project_with_tasks(&env);
    env.ok(&project, &["question", "answer", "1", "Truncated."]);
    env.ok(&project, &["sync"]);

    // Second machine: own hub, a clone of the repository.
    let hub2 = env.work.join("hub2");
    let clone = env.dir("clone");
    std::fs::copy(project.join(".airlock"), clone.join(".airlock")).unwrap();
    std::fs::create_dir_all(clone.join("docs")).unwrap();
    std::fs::copy(
        project.join("docs/.airlock-state.json"),
        clone.join("docs/.airlock-state.json"),
    )
    .unwrap();
    let run2 = |args: &[&str]| {
        env.cmd(&clone, args)
            .env("AIRLOCK_HOME", &hub2)
            .output()
            .unwrap()
    };

    assert!(run2(&["init"]).status.success());
    let refused = run2(&["import"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("pass --yes"));

    assert!(run2(&["import", "--yes"]).status.success());
    let tasks: Value = serde_json::from_slice(&run2(&["task", "list", "--json"]).stdout).unwrap();
    assert_eq!(tasks.as_array().unwrap().len(), 2);
    let questions: Value =
        serde_json::from_slice(&run2(&["question", "list", "--json"]).stdout).unwrap();
    assert_eq!(questions[0]["answer"], "Truncated.");

    // The imported file is known to the new hub: sync accepts it and changes nothing.
    let synced: Value = serde_json::from_slice(&run2(&["sync", "--json"]).stdout).unwrap();
    assert!(
        synced["unchanged"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "docs/.airlock-state.json")
    );
}

#[test]
fn import_rejects_invalid_snapshots() {
    let env = Env::new();
    let project = project_with_tasks(&env);
    env.ok(&project, &["sync"]);
    let state_path = project.join("docs/.airlock-state.json");
    let state = read(&state_path);

    let id = env.json(&project, &["init"])["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let other_id = state.replace(&id, "00000000-0000-4000-8000-000000000000");
    let bad = env.work.join("bad.json");
    std::fs::write(&bad, other_id).unwrap();
    assert!(
        env.fail(
            &project,
            &["import", "--yes", "--from", bad.to_str().unwrap()]
        )
        .contains("belongs to project")
    );

    std::fs::write(&bad, state.replace("\"L-1\"\n", "\"L-9\"\n")).unwrap();
    assert!(
        env.fail(
            &project,
            &["import", "--yes", "--from", bad.to_str().unwrap()]
        )
        .contains("unknown task `L-9`")
    );

    std::fs::write(
        &bad,
        state.replace("\"role\": \"engine\"", "\"role\": \"ghost\""),
    )
    .unwrap();
    assert!(
        env.fail(
            &project,
            &["import", "--yes", "--from", bad.to_str().unwrap()]
        )
        .contains("role `ghost`")
    );

    assert_eq!(
        env.json(&project, &["task", "list"])
            .as_array()
            .unwrap()
            .len(),
        2,
        "state untouched"
    );
}

#[test]
fn sync_needs_a_root_and_safe_export_paths() {
    let env = Env::new();
    let project = project_with_tasks(&env);
    let outside = env.dir("outside");
    assert!(
        env.fail(&outside, &["--project", "project", "sync"])
            .contains("needs a project root")
    );

    let id = env.json(&project, &["init"])["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let config_path = env.hub.join("projects").join(&id).join("project.json");
    let config = read(&config_path).replace("docs/.airlock-state.json", "../escaped.json");
    std::fs::write(&config_path, config).unwrap();
    assert!(env.fail(&project, &["sync"]).contains("must be relative"));
    assert!(!env.work.join("escaped.json").exists());
}

#[test]
fn sync_refuses_colliding_export_paths() {
    let env = Env::new();
    let project = project_with_tasks(&env);
    let id = env.json(&project, &["init"])["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let config_path = env.hub.join("projects").join(&id).join("project.json");
    let config = read(&config_path).replace("\"docs/PLAN.md\"", "\"docs/./.AIRLOCK-STATE.json\"");
    std::fs::write(&config_path, config).unwrap();
    assert!(
        env.fail(&project, &["sync"])
            .contains("more than one artifact")
    );
    assert!(!project.join("docs").exists(), "nothing written");
}
