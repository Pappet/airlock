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

/// Adds a task for the role behind `id`'s prefix and checks it got `id`.
fn add(env: &Env, cwd: &Path, id: &str, extra: &[&str]) {
    let role = match id.split('-').next() {
        Some("E") => "engine",
        Some("L") => "lead",
        Some("S") => "spec",
        other => panic!("no role for prefix {other:?}"),
    };
    let mut args = vec![
        "task", "add", "Title", "--role", role, "--do", "x", "--verify", "y",
    ];
    args.extend_from_slice(extra);
    assert_eq!(env.json(cwd, &args)["id"], id);
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
        &["--blocked-by", "L-1", "--priority", "low"],
    );
    add(&env, &project, "E-2", &["--priority", "normal"]);

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
fn a_directory_in_touch_occupies_the_files_beneath_it() {
    let env = Env::new();
    let project = env.dir("project");
    env.ok(&project, &["init"]);
    add(&env, &project, "E-1", &["--touch", "src/render/"]);
    add(
        &env,
        &project,
        "E-2",
        &["--touch", "src/render/mesh.rs", "--priority", "high"],
    );
    add(&env, &project, "E-3", &["--touch", "src/sim.rs"]);
    env.ok(&project, &["--session", "engine-2", "task", "claim", "E-1"]);

    // E-2 has the higher priority but lies beneath the occupied directory.
    let next = env.json(&project, &["task", "next", "--role", "engine"]);
    assert_eq!(next["id"], "E-3");
    env.ok(&project, &["--session", "engine-3", "task", "claim", "E-3"]);

    let claimed = env.json(&project, &["--session", "engine-1", "task", "claim", "E-2"]);
    assert_eq!(claimed["overlaps"][0]["id"], "E-1");
    assert_eq!(
        claimed["overlaps"][0]["paths"],
        serde_json::json!(["src/render/mesh.rs"])
    );
    assert!(claimed["question_id"].is_u64(), "the lead gets a question");
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
        &["--touch", "./src/sim.rs", "--priority", "high"],
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
    let fallback = env.json(&project, &["task", "next", "--role", "engine"]);
    assert_eq!(fallback["id"], "E-2");
    assert_eq!(fallback["overlaps"][0]["id"], "E-1");
    assert_eq!(
        fallback["overlaps"][0]["paths"],
        serde_json::json!(["src/sim.rs"])
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
fn a_failed_overlap_question_does_not_hide_the_claim() {
    let env = Env::new();
    let project = env.dir("project");
    env.ok(&project, &["init"]);
    add(&env, &project, "E-1", &["--touch", "src/sim.rs"]);
    add(&env, &project, "E-2", &["--touch", "src/sim.rs"]);
    env.ok(&project, &["--session", "engine-2", "task", "claim", "E-1"]);
    let marker: Value = serde_json::from_str(&read(&project.join(".airlock"))).unwrap();
    let db = env
        .hub
        .join("projects")
        .join(marker["project_id"].as_str().unwrap())
        .join("airlock.db");
    rusqlite::Connection::open(db)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER no_questions BEFORE INSERT ON questions
             BEGIN SELECT RAISE(ABORT, 'questions are read-only'); END;",
        )
        .unwrap();

    let out = env.run(
        &project,
        &["--session", "engine-1", "task", "claim", "E-2", "--json"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let claimed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(claimed["task_id"], "E-2");
    assert_eq!(claimed["overlaps"][0]["id"], "E-1");
    assert!(claimed.get("question_id").is_none());
    assert!(
        claimed["question_error"]
            .as_str()
            .unwrap()
            .contains("questions are read-only")
    );
    let warning = String::from_utf8_lossy(&out.stderr);
    assert!(
        warning.contains("could not record the question for the lead")
            && warning.contains("read-only"),
        "{warning}"
    );
    assert_eq!(
        env.json(&project, &["task", "show", "E-2"])["claimed_by"],
        "engine-1"
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
fn a_restarted_session_resumes_its_claim() {
    // Every CLI call is its own process: the second half plays a new session in the
    // same bound worktree after the first one lost its context.
    let env = Env::new();
    let project = env.dir("project");
    env.ok(&project, &["init"]);
    add(&env, &project, "E-1", &[]);
    add(&env, &project, "E-2", &[]);
    env.ok(&project, &["use", "engine"]);
    env.ok(&project, &["task", "claim", "E-1"]);
    env.ok(
        &project,
        &["task", "heartbeat", "E-1", "--notes", "half done"],
    );

    let brief = env.json(&project, &["start"]);
    assert_eq!(brief["claimed"][0]["id"], "E-1");
    assert_eq!(brief["claimed"][0]["progress"], "half done");
    assert!(brief["claimed"][0]["lease_minutes_left"].as_i64().unwrap() > 0);
    assert_eq!(
        brief["next"],
        serde_json::Value::Null,
        "no new package while E-1 is held"
    );
    let out = env.run(&project, &["task", "claim", "E-1"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("already claimed by this session; use `task heartbeat`"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    env.ok(&project, &["task", "done", "E-1", "--notes", "finished"]);
    let brief = env.json(&project, &["start"]);
    assert_eq!(brief["claimed"], serde_json::json!([]));
    assert_eq!(brief["next"]["id"], "E-2");
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
    env.ok(
        &project,
        &[
            "task",
            "heartbeat",
            "L-1",
            "--notes",
            "half done; next: docs",
        ],
    );
    let again = env.json(&project, &["sync"]);
    assert_eq!(again["written"], serde_json::json!([]));
    assert_eq!(read(&project.join("docs/.airlock-state.json")), state);
    assert!(!read(&project.join("docs/PLAN.md")).contains("half done"));

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

    let imported = run2(&["import", "--yes"]);
    assert!(imported.status.success());
    let summary = String::from_utf8_lossy(&imported.stdout).into_owned();
    assert!(
        summary.contains("Imported 2 tasks, 0 tracks, and"),
        "{summary}"
    );
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
fn import_accepts_a_version_1_snapshot_and_the_next_sync_writes_format_2() {
    let env = Env::new();
    let project = env.dir("project");
    let id = env.json(&project, &["init"])["project_id"]
        .as_str()
        .unwrap()
        .to_string();

    // A snapshot as committed before tracks: integer priorities, no tracks.
    let task = |n: u32, priority: i64| {
        format!(
            r#"{{"id": "L-{n}", "title": "t", "role": "lead", "state": "open", "priority": {priority},
            "blocked_by": [], "read": [], "touch": [], "do": "d", "verify": "v",
            "notes": null, "created_at": 1700000000}}"#
        )
    };
    let tasks: Vec<String> = [40, 30, 29, 10, 9, 5]
        .iter()
        .enumerate()
        .map(|(i, p)| task(i as u32 + 1, *p))
        .collect();
    let v1 = format!(
        r#"{{"format_version": 1, "project_id": "{id}",
        "roles": {{"lead": {{"description": "l", "clean_room": false}}}},
        "tasks": [{}], "questions": []}}"#,
        tasks.join(",")
    );
    let state_path = project.join("docs/.airlock-state.json");
    std::fs::create_dir_all(state_path.parent().unwrap()).unwrap();
    std::fs::write(&state_path, v1).unwrap();

    env.ok(&project, &["import", "--yes"]);
    let listed = env.json(&project, &["task", "list"]);
    let priorities: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["priority"].as_str().unwrap())
        .collect();
    assert_eq!(
        priorities,
        ["high", "high", "normal", "normal", "low", "low"]
    );
    assert!(
        env.json(&project, &["track", "list"])
            .as_array()
            .unwrap()
            .is_empty()
    );

    env.ok(&project, &["sync"]);
    let exported: Value = serde_json::from_str(&read(&state_path)).unwrap();
    assert_eq!(exported["format_version"], 3);
    assert_eq!(exported["tracks"], serde_json::json!([]));
    assert_eq!(exported["tasks"][0]["priority"], "high");
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

#[test]
fn tracks_from_the_cli() {
    let env = Env::new();
    let project = env.dir("project");
    env.ok(&project, &["init"]);
    env.ok(
        &project,
        &[
            "track",
            "add",
            "core",
            "--priority",
            "critical",
            "--description",
            "The battle faithful to the tick",
        ],
    );
    env.ok(&project, &["track", "add", "comfort", "--priority", "low"]);
    assert!(
        env.fail(&project, &["track", "add", "bad name"])
            .contains("track name"),
    );
    assert!(
        env.fail(&project, &["track", "add", "core"])
            .contains("already exists")
    );

    add(&env, &project, "E-1", &["--track", "comfort"]);
    let task = env.json(&project, &["task", "show", "E-1"]);
    assert_eq!(task["track"], "comfort");
    assert!(
        env.fail(
            &project,
            &[
                "task", "add", "T", "--role", "engine", "--do", "x", "--verify", "y", "--track",
                "nope"
            ]
        )
        .contains("core")
    );

    let tracks = env.json(&project, &["track", "list"]);
    let names: Vec<&str> = tracks
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["core", "comfort"], "most important first");
    assert_eq!(tracks[0]["priority"], "critical");
    assert_eq!(tracks[0]["counts"]["ready"], 0);
    assert_eq!(tracks[1]["counts"]["ready"], 1);
    assert_eq!(
        tracks[1]["counts"],
        serde_json::json!({ "blocked": 0, "ready": 1, "in_progress": 0, "in_review": 0, "done": 0 })
    );

    add(&env, &project, "E-2", &["--priority", "low"]);
    add(&env, &project, "E-3", &["--priority", "high"]);
    let high = env.json(&project, &["task", "list", "--priority", "high"]);
    assert_eq!(high.as_array().unwrap().len(), 1, "at least high");
    assert_eq!(high[0]["id"], "E-3");
    let comfort = env.json(&project, &["task", "list", "--track", "comfort"]);
    assert_eq!(comfort.as_array().unwrap().len(), 1);
    let none = env.json(&project, &["task", "list", "--no-track"]);
    assert_eq!(none.as_array().unwrap().len(), 2);
    let unknown = env.fail(&project, &["task", "list", "--track", "nope"]);
    assert!(
        unknown.contains("no track `nope`; defined tracks: comfort, core"),
        "{unknown}"
    );

    assert!(
        env.fail(&project, &["track", "remove", "comfort"])
            .contains("used by 1 task")
    );
    assert!(
        env.fail(&project, &["track", "edit", "comfort"])
            .contains("nothing to change")
    );
    env.ok(&project, &["track", "edit", "comfort", "--name", "polish"]);
    assert_eq!(
        env.json(&project, &["task", "show", "E-1"])["track"],
        "polish"
    );

    env.ok(&project, &["task", "edit", "E-1", "--clear-track"]);
    assert!(
        env.json(&project, &["task", "show", "E-1"])
            .get("track")
            .is_none_or(Value::is_null)
    );
    env.ok(&project, &["task", "edit", "E-1", "--track", "core"]);
    assert_eq!(
        env.json(&project, &["task", "next", "--role", "engine"])["id"],
        "E-1",
        "the critical track goes first"
    );
    env.ok(&project, &["track", "remove", "polish"]);

    let text = env.ok(&project, &["track", "list"]);
    assert!(text.contains("critical  core"), "{text}");
    assert!(text.contains("(1 ready)"), "{text}");
    assert!(text.contains("The battle faithful to the tick"), "{text}");
    let shown = env.ok(&project, &["task", "show", "E-1"]);
    assert!(shown.contains("track: core"), "{shown}");
    let table = env.ok(&project, &["task", "list"]);
    assert!(table.contains("TRACK"), "{table}");
}

#[test]
fn inbox_on_the_cli_and_events_never_reach_the_snapshot() {
    let env = Env::new();
    let project = project_with_tasks(&env);
    // `init` without a template binds nothing; one session reads for two roles.
    assert_eq!(
        env.ok(&project, &["inbox", "--role", "engine"]).trim(),
        "Nothing new for engine."
    );
    env.ok(&project, &["inbox", "--role", "lead"]);
    env.ok(&project, &["sync"]);
    let state = read(&project.join("docs/.airlock-state.json"));

    // Claim and release L-1: an event, but no change to what `sync` exports.
    env.ok(&project, &["task", "claim", "L-1"]);
    env.ok(&project, &["task", "release", "L-1"]);
    let lead = env.ok(&project, &["inbox", "--role", "lead", "--peek"]);
    assert!(lead.contains("L-1 ready"), "{lead}");
    env.ok(&project, &["inbox", "--role", "lead"]);
    assert_eq!(
        env.ok(&project, &["inbox", "--role", "lead"]).trim(),
        "Nothing new for lead."
    );
    let again = env.json(&project, &["sync"]);
    assert_eq!(again["written"], serde_json::json!([]));
    assert_eq!(read(&project.join("docs/.airlock-state.json")), state);

    // Completing L-1 unblocks E-1 for the engine.
    env.ok(&project, &["task", "claim", "L-1"]);
    env.ok(&project, &["task", "done", "L-1"]);
    let engine = env.json(&project, &["inbox", "--role", "engine"]);
    assert_eq!(engine["items"][0]["kind"], "task_ready");
    assert_eq!(engine["items"][0]["task"]["id"], "E-1");
    assert!(
        env.fail(&project, &["inbox", "--role", "nobody"])
            .contains("nobody"),
        "unknown roles are refused"
    );
}
