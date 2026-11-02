//! Onboarding (Spec 06): bindings, `-C`, roles, briefing, doctor, setup.

use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

/// A git repository `game` with a registered project and a worktree `game-engine`.
struct Env {
    tmp: TempDir,
    hub: PathBuf,
    home: PathBuf,
    main: PathBuf,
    engine: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let hub = tmp.path().join("hub");
        let home = tmp.path().join("home");
        let main = tmp.path().join("game");
        let engine = tmp.path().join("game-engine");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&main).unwrap();
        let env = Self {
            tmp,
            hub,
            home,
            main,
            engine,
        };
        env.git(&env.main, &["init", "-q", "-b", "main"]);
        env.git(&env.main, &["commit", "-q", "--allow-empty", "-m", "init"]);
        env.ok(&env.main, &["init"]);
        env.git(&env.main, &["add", ".airlock"]);
        env.git(&env.main, &["commit", "-q", "-m", "marker"]);
        env.git(
            &env.main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "engine",
                env.engine.to_str().unwrap(),
            ],
        );
        env
    }

    fn git(&self, dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    fn cmd(&self, cwd: &Path, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_airlock"));
        cmd.args(args)
            .current_dir(cwd)
            .env("AIRLOCK_HOME", &self.hub)
            .env("HOME", &self.home)
            .env_remove("AIRLOCK_PROJECT")
            .env_remove("AIRLOCK_SESSION");
        cmd
    }

    fn run(&self, cwd: &Path, args: &[&str]) -> Output {
        self.cmd(cwd, args).output().unwrap()
    }

    fn ok(&self, cwd: &Path, args: &[&str]) -> String {
        let out = self.run(cwd, args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn fail(&self, cwd: &Path, args: &[&str]) -> String {
        let out = self.run(cwd, args);
        assert!(!out.status.success(), "{args:?} should fail");
        String::from_utf8(out.stderr).unwrap()
    }

    fn json(&self, cwd: &Path, args: &[&str]) -> Value {
        let mut args = args.to_vec();
        args.push("--json");
        serde_json::from_str(&self.ok(cwd, &args)).unwrap()
    }

    /// Adds an engine task and checks it got `id`.
    fn add_engine_task(&self, id: &str) {
        let task = self.json(
            &self.main,
            &[
                "task",
                "add",
                "Engine work",
                "--role",
                "engine",
                "--do",
                "Build",
                "--verify",
                "cargo test",
            ],
        );
        assert_eq!(task["id"], id);
    }
}

#[test]
fn use_binds_shows_locks_and_clears() {
    let env = Env::new();
    assert!(env.ok(&env.engine, &["use"]).contains("has no binding"));

    let bound = env.json(&env.engine, &["use", "engine", "--lock"]);
    assert_eq!(
        bound,
        json!({ "role": "engine", "session": "engine@game-engine", "locked": true })
    );
    assert_eq!(env.json(&env.engine, &["use"])["role"], "engine");

    assert!(
        env.fail(&env.engine, &["use", "lead"])
            .contains("locked to role `engine`")
    );
    assert!(
        env.fail(&env.engine, &["use", "--clear"])
            .contains("--force")
    );
    assert!(
        env.fail(&env.main, &["use", "ghost"])
            .contains("unknown role")
    );

    let custom = env.json(&env.main, &["use", "lead", "--session", "lead-1"]);
    assert_eq!(custom["session"], "lead-1");

    env.ok(&env.engine, &["use", "--clear", "--force"]);
    assert_eq!(env.json(&env.engine, &["use"]), Value::Null);

    let outside = env.tmp.path();
    assert!(
        env.fail(outside, &["--project", "game", "use", "lead"])
            .contains("needs a project root")
    );
}

#[test]
fn binding_supplies_session_and_role_to_the_cli() {
    let env = Env::new();
    env.add_engine_task("E-1");
    assert!(env.fail(&env.engine, &["task", "next"]).contains("no role"));

    env.ok(&env.engine, &["use", "engine"]);
    assert_eq!(env.json(&env.engine, &["task", "next"])["id"], "E-1");
    env.ok(&env.engine, &["task", "claim", "E-1"]);
    assert_eq!(
        env.json(&env.main, &["task", "show", "E-1"])["claimed_by"],
        "engine@game-engine"
    );

    // An explicit session still wins over the binding.
    let out = env
        .cmd(&env.engine, &["task", "release", "E-1"])
        .env("AIRLOCK_SESSION", "other")
        .output()
        .unwrap();
    assert!(!out.status.success(), "other session cannot release");
    env.ok(&env.engine, &["task", "release", "E-1"]);
}

#[test]
fn dash_c_runs_as_if_started_in_the_directory() {
    let env = Env::new();
    env.ok(&env.engine, &["use", "engine"]);
    env.add_engine_task("E-1");
    let outside = env.tmp.path();
    assert_eq!(
        env.json(outside, &["-C", "game-engine", "task", "next"])["id"],
        "E-1"
    );
    assert_eq!(
        env.json(outside, &["-C", "game-engine", "use"])["role"],
        "engine"
    );
    assert!(
        env.fail(outside, &["-C", "nope", "task", "list"])
            .contains("not a directory")
    );
}

#[test]
fn role_prefixes_number_tasks_and_must_be_unique() {
    let env = Env::new();
    env.add_engine_task("E-1");
    let clash = env.fail(
        &env.main,
        &["role", "set", "simulation", "--description", "Sim"],
    );
    assert!(
        clash.contains("roles `simulation` and `spec` share the task prefix `S`"),
        "{clash}"
    );
    assert!(env.json(&env.main, &["role", "list"])["simulation"].is_null());

    let role = env.json(&env.main, &["role", "set", "simulation", "--prefix", "SIM"]);
    assert_eq!(role["prefix"], "SIM");
    let task = env.json(
        &env.main,
        &[
            "task",
            "add",
            "Run",
            "--role",
            "simulation",
            "--do",
            "x",
            "--verify",
            "y",
        ],
    );
    assert_eq!(task["id"], "SIM-1");
    env.add_engine_task("E-2");

    let taken = env.fail(&env.main, &["role", "set", "simulation", "--prefix", "E"]);
    assert!(taken.contains("share the task prefix `E`"), "{taken}");
    let bad = env.fail(&env.main, &["role", "set", "simulation", "--prefix", "S-"]);
    assert!(bad.contains("invalid task prefix `S-`"), "{bad}");
    assert_eq!(
        env.json(&env.main, &["role", "list"])["simulation"]["prefix"],
        "SIM",
        "refused changes are not saved"
    );
    assert!(
        env.ok(&env.main, &["role", "list"])
            .contains("simulation (prefix SIM)")
    );

    // `--prefix ""` returns to the default: `R` for `runner`.
    env.ok(&env.main, &["role", "set", "runner", "--prefix", "RUN"]);
    let reset = env.json(&env.main, &["role", "set", "runner", "--prefix", ""]);
    assert!(reset.get("prefix").is_none(), "{reset}");
    let task = env.json(
        &env.main,
        &[
            "task", "add", "Go", "--role", "runner", "--do", "x", "--verify", "y",
        ],
    );
    assert_eq!(task["id"], "R-1");
}

#[test]
fn roles_are_managed_from_the_cli() {
    let env = Env::new();
    let qa = env.json(
        &env.main,
        &[
            "role",
            "set",
            "qa",
            "--description",
            "Quality",
            "--instructions",
            "Test only.",
            "--read",
            "docs/QA.md",
        ],
    );
    assert_eq!(
        qa,
        json!({ "description": "Quality", "clean_room": false, "instructions": "Test only.", "read": ["docs/QA.md"] })
    );
    env.ok(&env.main, &["role", "set", "qa", "--clean-room", "true"]);
    let roles = env.json(&env.main, &["role", "list"]);
    assert_eq!(roles["qa"]["clean_room"], true);
    assert_eq!(
        roles["qa"]["read"],
        json!(["docs/QA.md"]),
        "unchanged when --read is not given"
    );

    env.ok(
        &env.main,
        &[
            "task", "add", "Check", "--role", "qa", "--do", "x", "--verify", "y",
        ],
    );
    assert!(
        env.fail(&env.main, &["role", "remove", "qa"])
            .contains("used by 1 task")
    );
    env.ok(&env.main, &["use", "spec"]);
    assert!(
        env.fail(&env.main, &["role", "remove", "spec"])
            .contains("is bound to")
    );
    env.ok(&env.main, &["role", "remove", "lead"]);
    assert!(env.json(&env.main, &["role", "list"]).get("lead").is_none());
    assert!(
        env.fail(&env.main, &["role", "set", "bad name"])
            .contains("invalid role name")
    );
}

#[test]
fn start_briefs_claims_next_work_and_answers() {
    let env = Env::new();
    env.ok(
        &env.main,
        &[
            "role",
            "set",
            "engine",
            "--instructions",
            "Work only from docs/spec.",
            "--read",
            "CLAUDE.md",
        ],
    );
    env.add_engine_task("E-1");
    env.ok(&env.engine, &["use", "engine"]);

    let text = env.ok(&env.engine, &["start"]);
    assert!(text.contains("role **engine** (clean-room)"));
    assert!(text.contains("Work only from docs/spec."));
    assert!(text.contains("Read first: `CLAUDE.md`"));
    assert!(text.contains("## Next work package") && text.contains("airlock task claim E-1"));
    // The Workflow rules: one task at a time, the next one from Airlock, read the news.
    assert!(
        text.contains("never plan task ids ahead and never claim a second task while you hold one")
            && text.contains(
                "take the next task from the `Next:` line it prints or from `airlock task next`;"
            )
            && text.contains("Read the `Since your last look` lines of every `airlock task done`"),
        "{text}"
    );
    let mcp = env.ok(&env.engine, &["start", "--hints", "mcp"]);
    assert!(
        mcp.contains("take the next task from `task_complete`'s `next` field or from `task_next`")
            && mcp.contains("Read the `inbox` part of every `task_complete` result")
            && mcp.contains("call `inbox_wait` instead of ending your turn — but only if your instructions say you run unattended"),
        "{mcp}"
    );

    env.ok(&env.engine, &["task", "claim", "E-1"]);
    env.ok(
        &env.engine,
        &["question", "ask", "Arc?", "--topic", "melee.md"],
    );
    env.ok(&env.main, &["question", "answer", "1", "Truncated."]);
    let brief = env.json(&env.engine, &["start"]);
    assert_eq!(brief["claimed"][0]["id"], "E-1");
    assert_eq!(brief["claimed"][0]["lease_minutes_left"], 60);
    assert_eq!(brief["next"], Value::Null);
    assert_eq!(brief["answered_questions"][0]["answer"], "Truncated.");
    let text = env.ok(&env.engine, &["start"]);
    assert!(text.contains("## Your claimed work") && text.contains("## Answers to your questions"));
    assert!(
        text.contains("--notes"),
        "the workflow names progress notes: {text}"
    );

    // Progress notes reach the holder's briefing, and the next session's after a release.
    env.ok(
        &env.engine,
        &[
            "task",
            "heartbeat",
            "E-1",
            "--notes",
            "Arc done.\nNext: knockback.",
        ],
    );
    let brief = env.json(&env.engine, &["start"]);
    assert_eq!(
        brief["claimed"][0]["progress"],
        "Arc done.\nNext: knockback."
    );
    let text = env.ok(&env.engine, &["start"]);
    assert!(
        text.contains("- **Progress:** Arc done.\n  Next: knockback."),
        "{text}"
    );
    assert!(
        text.contains("from where the progress note says it stopped"),
        "{text}"
    );
    env.ok(&env.engine, &["task", "release", "E-1"]);
    env.ok(&env.engine, &["use", "engine", "--session", "engine-2"]);
    let text = env.ok(&env.engine, &["start"]);
    assert!(text.contains("## Next work package"), "{text}");
    assert!(text.contains("- **Earlier progress:** Arc done."), "{text}");
    assert!(
        env.ok(&env.engine, &["task", "show", "E-1"])
            .contains("**Progress:** Arc done."),
        "task show prints the note"
    );

    let unbound = env.ok(&env.main, &["start"]);
    assert!(unbound.contains("## No role"));
}

#[test]
fn answers_already_shown_are_one_line_and_hints_can_name_mcp_tools() {
    let env = Env::new();
    env.add_engine_task("E-1");
    env.ok(&env.engine, &["use", "engine"]);
    env.ok(
        &env.engine,
        &["question", "ask", "Arc width?", "--topic", "melee.md"],
    );
    env.ok(
        &env.main,
        &[
            "question",
            "answer",
            "1",
            "LONG-ANSWER-TEXT",
            "--spec",
            "docs/spec/melee.md",
        ],
    );

    // The first briefing after the answer shows it in full. Times are whole seconds:
    // only a briefing strictly later than the answer counts as having shown it.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let text = env.ok(&env.engine, &["start"]);
    assert!(text.contains("LONG-ANSWER-TEXT"), "{text}");
    let text = env.ok(&env.engine, &["start"]);
    assert!(!text.contains("LONG-ANSWER-TEXT"), "{text}");
    assert!(
        text.contains("Already shown in an earlier briefing (read one again with `airlock question show <id>`)")
            && text.contains("- **#1 (melee.md)** — codified in `docs/spec/melee.md`"),
        "{text}"
    );
    let brief = env.json(&env.engine, &["start"]);
    assert_eq!(
        brief["answered_questions"][0]["answer"], "LONG-ANSWER-TEXT",
        "the structured form keeps everything"
    );

    // A new answer is shown in full again, next to the one-liner.
    env.ok(
        &env.engine,
        &["question", "ask", "Second?", "--topic", "melee.md"],
    );
    std::thread::sleep(std::time::Duration::from_millis(1100));
    env.ok(&env.main, &["question", "answer", "2", "NEW-ANSWER"]);
    let text = env.ok(&env.engine, &["start", "--hints", "mcp"]);
    assert!(
        text.contains("NEW-ANSWER") && !text.contains("LONG-ANSWER-TEXT"),
        "{text}"
    );
    assert!(
        text.contains("`task_claim`") && !text.contains("airlock task claim"),
        "{text}"
    );
    assert!(
        text.contains("read one again with `question_list`"),
        "{text}"
    );
}

#[test]
fn the_package_names_its_track_and_priority() {
    let env = Env::new();
    env.ok(
        &env.main,
        &["track", "add", "core", "--priority", "critical"],
    );
    env.ok(
        &env.main,
        &[
            "task",
            "add",
            "Engine work",
            "--role",
            "engine",
            "--do",
            "Build",
            "--verify",
            "cargo test",
            "--track",
            "core",
            "--priority",
            "high",
        ],
    );
    env.ok(&env.engine, &["use", "engine"]);
    let text = env.ok(&env.engine, &["start"]);
    assert!(
        text.contains("**Track:** core · **Priority:** high"),
        "{text}"
    );
    let brief = env.json(&env.engine, &["start"]);
    assert_eq!(brief["next"]["track"], "core");
    assert_eq!(brief["next"]["priority"], "high");

    env.ok(&env.main, &["task", "edit", "E-1", "--clear-track"]);
    let text = env.ok(&env.engine, &["start"]);
    assert!(text.contains("- **Priority:** high"), "{text}");
    assert!(!text.contains("**Track:**"), "{text}");
}

fn mcp_session(env: &Env, cwd: &Path, args: &[&str], lines: &[Value]) -> (Vec<Value>, Output) {
    let mut argv = vec!["mcp"];
    argv.extend_from_slice(args);
    let mut child = env
        .cmd(cwd, &argv)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for line in lines {
        writeln!(stdin, "{line}").unwrap();
    }
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    let responses = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    (responses, out)
}

#[test]
fn mcp_takes_role_and_session_from_the_binding() {
    let env = Env::new();
    env.add_engine_task("E-1");
    env.ok(&env.engine, &["use", "engine", "--lock"]);
    let request = |id: u64, method: &str, params: Value| json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
    let (responses, _) = mcp_session(
        &env,
        &env.engine,
        &[],
        &[
            request(
                1,
                "initialize",
                json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "t", "version": "0" } }),
            ),
            request(
                2,
                "tools/call",
                json!({ "name": "session_start", "arguments": {} }),
            ),
            request(
                3,
                "tools/call",
                json!({ "name": "task_claim", "arguments": { "task_id": "E-1" } }),
            ),
            request(4, "prompts/list", json!({})),
            request(5, "prompts/get", json!({ "name": "start" })),
            request(6, "prompts/get", json!({ "name": "nope" })),
        ],
    );
    assert!(responses[0]["result"]["capabilities"]["prompts"].is_object());
    let brief = &responses[1]["result"]["structuredContent"];
    assert_eq!(
        (brief["role"].as_str(), brief["session"].as_str()),
        (Some("engine"), Some("engine@game-engine"))
    );
    assert_eq!(brief["next"]["id"], "E-1");
    assert!(
        responses[1]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("`task_claim`")
    );
    assert_eq!(responses[2]["result"]["isError"], false);
    assert_eq!(responses[3]["result"]["prompts"][0]["name"], "start");
    let prompt = responses[4]["result"]["messages"][0]["content"]["text"]
        .as_str()
        .unwrap();
    assert!(
        prompt.contains("## Your claimed work")
            && prompt.ends_with("Begin with the claimed or next work package.")
    );
    assert_eq!(responses[5]["error"]["code"], -32602);
    assert_eq!(
        env.json(&env.main, &["task", "show", "E-1"])["claimed_by"],
        "engine@game-engine"
    );

    let (_, out) = mcp_session(&env, &env.engine, &["--role", "lead"], &[]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("locked to role `engine`"));
}

#[test]
fn doctor_reports_worktrees_bindings_and_configurations() {
    let env = Env::new();
    env.ok(&env.main, &["use", "lead", "--session", "shared"]);
    env.ok(
        &env.engine,
        &["use", "engine", "--session", "shared", "--lock"],
    );
    std::fs::write(
        env.engine.join(".mcp.json"),
        json!({ "mcpServers": { "airlock": { "command": "airlock", "args": ["mcp", "--role", "lead"] } } }).to_string(),
    )
    .unwrap();

    let findings = env.json(&env.main, &["doctor"]);
    let messages: Vec<(String, String)> = findings
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["level"].as_str().unwrap().to_string(),
                f["message"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let has =
        |level: &str, text: &str| messages.iter().any(|(l, m)| l == level && m.contains(text));
    assert!(has("ok", ".airlock is committed"), "{messages:?}");
    assert!(
        has("ok", "role engine, session shared (locked)"),
        "{messages:?}"
    );
    assert!(
        has("warn", "session shared is bound in several worktrees"),
        "{messages:?}"
    );
    assert!(
        has(
            "warn",
            "--role lead contradicts the locked binding to engine"
        ),
        "{messages:?}"
    );

    // A binding whose role disappeared is an error and fails the run.
    let id = env.json(&env.main, &["init"])["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let config = env.hub.join("projects").join(&id).join("project.json");
    let mut value: Value =
        serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    value["roles"].as_object_mut().unwrap().remove("lead");
    std::fs::write(&config, value.to_string()).unwrap();
    assert!(
        env.fail(&env.main, &["doctor"])
            .contains("doctor found 1 error")
    );
}

#[test]
fn doctor_works_outside_a_project() {
    let env = Env::new();
    let out = env.ok(env.tmp.path(), &["doctor"]);
    assert!(out.contains("no project here"));
}

#[test]
fn setup_prints_harness_configurations() {
    let env = Env::new();
    env.ok(&env.engine, &["use", "engine"]);
    let root = std::fs::canonicalize(&env.engine)
        .unwrap()
        .to_string_lossy()
        .into_owned();

    let claude = env.json(&env.engine, &["setup", "claude"]);
    assert_eq!(
        claude["args"],
        json!(["mcp"]),
        "role and session come from the binding"
    );
    let claude_spec = env.json(&env.engine, &["setup", "claude", "--role", "spec"]);
    assert_eq!(
        claude_spec["args"],
        json!(["mcp", "--role", "spec", "--session", "spec@game-engine"])
    );

    let antigravity = env.json(&env.engine, &["setup", "antigravity"]);
    assert_eq!(
        antigravity["args"],
        json!([
            "-C",
            root,
            "mcp",
            "--role",
            "engine",
            "--session",
            "engine@game-engine"
        ])
    );

    let dsh = env.ok(&env.engine, &["setup", "dsh"]);
    assert!(dsh.contains(
        "- insert:\n    - id: mcp-airlock-engine\n      name: '@deepseek-ai/dsh-mcp-client'\n"
    ));
    assert!(dsh.contains(&format!("'-C', '{root}', 'mcp'")));
    assert!(dsh.contains("dsh web --patch ~/.dsh/airlock/game-engine.yml --port 0"));

    assert!(
        env.fail(&env.main, &["setup", "claude"])
            .contains("no role")
    );
}

#[test]
fn concurrent_use_calls_keep_every_binding() {
    let env = Env::new();
    let marker = std::fs::read(env.main.join(".airlock")).unwrap();
    let dirs: Vec<PathBuf> = (0..8)
        .map(|i| {
            let dir = env.tmp.path().join(format!("wt{i}"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(".airlock"), &marker).unwrap();
            dir
        })
        .collect();
    let children: Vec<_> = dirs
        .iter()
        .map(|dir| {
            env.cmd(dir, &["use", "lead"])
                .stdout(Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }
    for (i, dir) in dirs.iter().enumerate() {
        assert_eq!(
            env.json(dir, &["use"])["session"],
            format!("lead@wt{i}"),
            "binding of wt{i} lost"
        );
    }
}

#[test]
fn use_and_role_flags_are_consistent() {
    let env = Env::new();
    let out = env.run(&env.main, &["use", "--lock"]);
    assert_eq!(out.status.code(), Some(2), "--lock needs a role");
    assert_eq!(env.json(&env.main, &["use", "--clear"])["removed"], false);

    env.ok(&env.main, &["role", "set", "lead", "--read", "CLAUDE.md"]);
    let cleared = env.json(&env.main, &["role", "set", "lead", "--clear-read"]);
    assert!(
        cleared.get("read").is_none(),
        "empty list is omitted: {cleared}"
    );
    assert_eq!(
        env.run(
            &env.main,
            &["role", "set", "lead", "--read", "x", "--clear-read"]
        )
        .status
        .code(),
        Some(2)
    );
}

#[test]
fn doctor_understands_flags_with_equals_sign() {
    let env = Env::new();
    env.ok(&env.engine, &["use", "engine", "--lock"]);
    std::fs::write(
        env.engine.join(".mcp.json"),
        json!({ "mcpServers": { "airlock": { "command": "airlock", "args": ["mcp", "--role=lead"] } } }).to_string(),
    )
    .unwrap();
    let findings = env.json(&env.main, &["doctor"]);
    assert!(
        findings.as_array().unwrap().iter().any(|f| f["message"]
            .as_str()
            .unwrap()
            .contains("--role lead contradicts the locked binding")),
        "{findings}"
    );
}

#[test]
fn doctor_flags_bare_commands_and_split_identities() {
    let env = Env::new();
    env.ok(&env.engine, &["use", "engine"]);
    std::fs::write(
        env.engine.join(".mcp.json"),
        json!({ "mcpServers": { "airlock": {
            "command": "airlock",
            "args": ["mcp", "--role", "lead", "--session", "engine-1"],
        } } })
        .to_string(),
    )
    .unwrap();
    let findings = env.json(&env.main, &["doctor"]);
    let warnings: Vec<String> = findings
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["level"] == "warn")
        .map(|f| f["message"].as_str().unwrap().to_string())
        .collect();
    let has = |text: &str| warnings.iter().any(|w| w.contains(text));
    assert!(has("is looked up in the client's PATH"), "{warnings:?}");
    assert!(
        has("--session engine-1 differs from the worktree's bound session engine@game-engine"),
        "{warnings:?}"
    );
    assert!(
        has("--role lead differs from the worktree's bound role engine"),
        "{warnings:?}"
    );
}

#[test]
fn setup_writes_only_file_content_to_stdout() {
    let env = Env::new();
    env.ok(&env.engine, &["use", "engine"]);
    let out = env.run(&env.engine, &["setup", "claude"]);
    assert!(out.status.success());
    let config: Value = serde_json::from_slice(&out.stdout).expect("stdout is a valid .mcp.json");
    assert_eq!(config["mcpServers"]["airlock"]["args"], json!(["mcp"]));
    let hint = String::from_utf8_lossy(&out.stderr);
    assert!(
        hint.contains(".mcp.json") && hint.contains("local"),
        "{hint}"
    );

    let out = env.run(&env.engine, &["setup", "antigravity"]);
    serde_json::from_slice::<Value>(&out.stdout).expect("stdout is valid JSON");

    let out = env.run(&env.engine, &["setup", "claude-hooks"]);
    assert!(out.status.success());
    serde_json::from_slice::<Value>(&out.stdout).expect("stdout is a valid settings file");
    let hint = String::from_utf8_lossy(&out.stderr);
    assert!(
        hint.contains(".claude/settings.local.json") && hint.contains("merge"),
        "{hint}"
    );
}

/// The SessionStart hook command of a `setup claude-hooks` settings file.
fn hook_command(settings: &Value) -> String {
    assert_eq!(
        settings.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["hooks"],
        "the file carries only hooks: {settings}"
    );
    let group = &settings["hooks"]["SessionStart"][0];
    assert_eq!(group["matcher"], "startup|resume|clear|compact");
    let hook = &group["hooks"][0];
    assert_eq!(hook["type"], "command");
    assert_eq!(hook["timeout"], 15);
    hook["command"].as_str().unwrap().to_string()
}

#[test]
fn setup_prints_claude_hooks() {
    let env = Env::new();
    env.ok(&env.engine, &["use", "engine"]);
    let root = std::fs::canonicalize(&env.engine)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let bin = std::fs::canonicalize(env!("CARGO_BIN_EXE_airlock"))
        .unwrap()
        .to_string_lossy()
        .into_owned();

    let settings: Value =
        serde_json::from_str(&env.ok(&env.engine, &["setup", "claude-hooks"])).unwrap();
    let command = hook_command(&settings);
    assert!(command.contains("airlock"), "{command}");
    assert!(
        command.ends_with(" start --hints mcp"),
        "role and session come from the binding; the briefing names MCP tools: {command}"
    );
    assert!(command.contains(" -C "), "{command}");
    assert!(!command.contains("--role"), "{command}");

    let json = env.json(&env.engine, &["setup", "claude-hooks"]);
    assert_eq!(json["command"], bin);
    assert_eq!(json["args"], json!(["-C", root, "start", "--hints", "mcp"]));

    let spec = env.json(&env.engine, &["setup", "claude-hooks", "--role", "spec"]);
    assert_eq!(
        spec["args"],
        json!([
            "-C",
            root,
            "start",
            "--hints",
            "mcp",
            "--role",
            "spec",
            "--session",
            "spec@game-engine"
        ])
    );
    assert_eq!(spec["session"], "spec@game-engine");
}

#[test]
fn claude_hooks_run_from_paths_with_spaces() {
    let env = Env::new();
    let spaced = env.tmp.path().join("my engine");
    std::fs::create_dir_all(&spaced).unwrap();
    std::fs::copy(env.main.join(".airlock"), spaced.join(".airlock")).unwrap();
    env.ok(&spaced, &["use", "engine"]);
    let settings: Value =
        serde_json::from_str(&env.ok(&spaced, &["setup", "claude-hooks"])).unwrap();
    let command = hook_command(&settings);
    assert!(
        command.contains("'"),
        "a path with a space is quoted: {command}"
    );

    // The hook runs through a shell from another directory and still briefs this worktree.
    #[cfg(unix)]
    {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(&command)
            .current_dir(env.tmp.path())
            .env("AIRLOCK_HOME", &env.hub)
            .env_remove("AIRLOCK_SESSION")
            .env_remove("AIRLOCK_PROJECT")
            .env_remove("AIRLOCK_ROLE")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(text.contains("engine"), "{text}");
    }
}

#[test]
fn doctor_checks_claude_hooks() {
    let env = Env::new();
    env.ok(&env.engine, &["use", "engine", "--lock"]);
    let settings_dir = env.engine.join(".claude");
    std::fs::create_dir_all(&settings_dir).unwrap();
    let write = |file: &str, command: &str| {
        std::fs::write(
            settings_dir.join(file),
            json!({ "hooks": { "SessionStart": [{
                "matcher": "startup|resume|clear|compact",
                "hooks": [{ "type": "command", "command": command, "timeout": 15 }],
            }] } })
            .to_string(),
        )
        .unwrap();
    };
    let findings = |env: &Env| -> Vec<(String, String)> {
        let out = env.run(&env.main, &["doctor", "--json"]);
        let value: Value = serde_json::from_slice(&out.stdout).unwrap();
        value
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| f["message"].as_str().unwrap().contains("SessionStart"))
            .map(|f| {
                (
                    f["level"].as_str().unwrap().to_string(),
                    f["message"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    };

    // A valid hook, as `setup claude-hooks` prints it, is fine.
    let generated: Value =
        serde_json::from_str(&env.ok(&env.engine, &["setup", "claude-hooks"])).unwrap();
    std::fs::write(
        settings_dir.join("settings.local.json"),
        generated.to_string(),
    )
    .unwrap();
    let found = findings(&env);
    assert!(
        !found.is_empty() && found.iter().all(|(level, _)| level == "ok"),
        "{found:?}"
    );

    // A bare command name warns.
    write("settings.local.json", "airlock start");
    let found = findings(&env);
    assert!(
        found
            .iter()
            .any(|(level, m)| level == "warn" && m.contains("is looked up in Claude Code's PATH")),
        "{found:?}"
    );

    // --role against the locked binding is an error and fails the run.
    let bin = std::fs::canonicalize(env!("CARGO_BIN_EXE_airlock")).unwrap();
    write(
        "settings.json",
        &format!("{} start --role lead", bin.display()),
    );
    let found = findings(&env);
    assert!(
        found.iter().any(|(level, m)| level == "error"
            && m.contains("--role lead contradicts the locked binding to engine")),
        "{found:?}"
    );

    // A -C that points elsewhere warns.
    write(
        "settings.json",
        &format!("{} -C {} start", bin.display(), env.main.display()),
    );
    let found = findings(&env);
    assert!(
        found
            .iter()
            .any(|(level, m)| level == "warn" && m.contains("is not this worktree")),
        "{found:?}"
    );

    // A settings file that is not JSON is an error.
    std::fs::write(settings_dir.join("settings.json"), "{ not json").unwrap();
    let out = env.run(&env.main, &["doctor"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("Claude Code ignores its settings and hooks")
            || String::from_utf8_lossy(&out.stderr)
                .contains("Claude Code ignores its settings and hooks")
    );
}

#[test]
fn doctor_mentions_the_optional_hook_next_to_an_mcp_json() {
    let env = Env::new();
    env.ok(&env.engine, &["use", "engine"]);
    let mcp = env.ok(&env.engine, &["setup", "claude"]);
    std::fs::write(env.engine.join(".mcp.json"), mcp).unwrap();
    let findings = env.json(&env.main, &["doctor"]);
    assert!(
        findings
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["level"] == "ok"
                && f["message"]
                    .as_str()
                    .unwrap()
                    .contains("no Claude Code SessionStart hook (optional)")),
        "{findings}"
    );
}

#[test]
fn start_takes_a_second_role_in_a_shared_checkout() {
    let env = Env::new();
    env.ok(&env.main, &["use", "spec", "--session", "spec-1"]);
    env.ok(
        &env.main,
        &[
            "task",
            "add",
            "Lead work",
            "--role",
            "lead",
            "--do",
            "x",
            "--verify",
            "y",
        ],
    );

    let brief = env.json(
        &env.main,
        &["--session", "lead-1", "start", "--role", "lead"],
    );
    assert_eq!(
        (brief["role"].as_str(), brief["session"].as_str()),
        (Some("lead"), Some("lead-1"))
    );
    assert_eq!(brief["next"]["id"], "L-1");

    let out = env
        .cmd(&env.main, &["start", "--json"])
        .env("AIRLOCK_ROLE", "lead")
        .env("AIRLOCK_SESSION", "lead-1")
        .output()
        .unwrap();
    let brief: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(brief["role"], "lead");
    let next = env
        .cmd(&env.main, &["task", "next", "--json"])
        .env("AIRLOCK_ROLE", "lead")
        .output()
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&next.stdout).unwrap()["id"],
        "L-1"
    );

    assert!(
        env.fail(&env.main, &["start", "--role", "ghost"])
            .contains("unknown role")
    );
    env.ok(&env.engine, &["use", "engine", "--lock"]);
    assert!(
        env.fail(&env.engine, &["start", "--role", "lead"])
            .contains("locked to role `engine`")
    );
}

#[test]
fn briefing_shows_tasks_held_by_other_sessions_of_the_role() {
    let env = Env::new();
    env.add_engine_task("E-1");
    env.add_engine_task("E-2");
    env.ok(
        &env.engine,
        &["--session", "engine-a", "task", "claim", "E-1"],
    );

    let brief = env.json(
        &env.engine,
        &["--session", "engine-b", "start", "--role", "engine"],
    );
    assert_eq!(brief["others"][0]["id"], "E-1");
    assert_eq!(brief["others"][0]["session"], "engine-a");
    assert_eq!(brief["others"][0]["lease_minutes_left"], 60);
    assert_eq!(brief["next"]["id"], "E-2");
    let text = env.ok(
        &env.engine,
        &["--session", "engine-b", "start", "--role", "engine"],
    );
    assert!(
        text.contains("## In progress by other sessions of your role")
            && text.contains("`engine-a`, lease 60 min left")
    );

    let own = env.json(
        &env.engine,
        &["--session", "engine-a", "start", "--role", "engine"],
    );
    assert_eq!(
        own["others"],
        json!([]),
        "own claims are not listed as others"
    );
}

// `HOME` decides where doctor looks for ~/.claude.json; Windows ignores it.
#[cfg(unix)]
#[test]
fn doctor_checks_claude_local_scope_against_every_worktree() {
    let env = Env::new();
    env.ok(&env.engine, &["use", "engine", "--lock"]);
    let main_key = std::fs::canonicalize(&env.main)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let lead = json!({ "command": "/bin/airlock", "args": ["mcp", "--role", "lead", "--session", "lead-1"] });
    std::fs::write(
        env.home.join(".claude.json"),
        json!({ "projects": { main_key: { "mcpServers": { "airlock": lead } } } }).to_string(),
    )
    .unwrap();
    std::fs::write(
        env.main.join(".mcp.json"),
        json!({ "mcpServers": { "airlock": { "command": "/bin/airlock", "args": ["mcp"] } } })
            .to_string(),
    )
    .unwrap();
    std::fs::write(env.engine.join(".mcp.json"), "# not json\n{}").unwrap();

    let findings = env.json(&env.main, &["doctor"]);
    let warnings: Vec<String> = findings
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["level"] == "warn")
        .map(|f| f["message"].as_str().unwrap().to_string())
        .collect();
    let has = |text: &str| warnings.iter().any(|w| w.contains(text));
    assert!(has("for every worktree of this repository"), "{warnings:?}");
    assert!(
        has("--role lead contradicts the locked binding to engine"),
        "{warnings:?}"
    );
    assert!(
        has("is defined in .mcp.json and in Claude Code's local scope"),
        "{warnings:?}"
    );
    assert!(has("not valid JSON"), "{warnings:?}");
}

/// Runs `session_start` for the bound engine worktree and returns the response.
fn engine_session_start(env: &Env) -> Value {
    let (responses, _) = mcp_session(
        env,
        &env.engine,
        &[],
        &[
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "t", "version": "0" } } }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "session_start", "arguments": {} } }),
        ],
    );
    responses[1]["result"].clone()
}

#[test]
fn briefing_shows_answer_changed_until_the_next_briefing() {
    let env = Env::new();
    env.ok(&env.engine, &["use", "engine"]);
    env.ok(
        &env.engine,
        &["question", "ask", "Arc?", "--topic", "melee.md"],
    );
    env.ok(&env.main, &["question", "answer", "1", "Rounded."]);
    env.ok(&env.main, &["question", "resolve", "1"]);

    let first = engine_session_start(&env);
    let text = first["content"][0]["text"].as_str().unwrap();
    assert!(!text.contains("Answer changed"), "{text}");
    assert_eq!(first["structuredContent"]["corrected_answers"], json!([]));

    // No pause after the first briefing: a correction in its very second still shows.
    env.ok(&env.main, &["question", "answer", "1", "Truncated."]);
    // Log times are whole seconds: the second briefing must fall after the
    // correction, or the third could not tell the correction was already shown.
    std::thread::sleep(std::time::Duration::from_millis(1100));

    let second = engine_session_start(&env);
    let text = second["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("## Answer changed"), "{text}");
    assert!(text.contains("#1 (melee.md)") && text.contains("Truncated."));
    let corrected = &second["structuredContent"]["corrected_answers"];
    assert_eq!(corrected[0]["id"], 1);
    assert_eq!(corrected[0]["answer"], "Truncated.");
    assert_eq!(corrected[0]["history"][0]["answer"], "Rounded.");
    assert_eq!(second["structuredContent"]["answered_questions"], json!([]));

    let third = engine_session_start(&env);
    let text = third["content"][0]["text"].as_str().unwrap();
    assert!(!text.contains("Answer changed"), "{text}");
    assert_eq!(third["structuredContent"]["corrected_answers"], json!([]));
}

#[test]
fn corrected_answer_still_unresolved_is_marked_not_repeated() {
    let env = Env::new();
    env.ok(&env.engine, &["use", "engine"]);
    env.ok(
        &env.engine,
        &["question", "ask", "Arc?", "--topic", "melee.md"],
    );
    env.ok(&env.main, &["question", "answer", "1", "Rounded."]);
    env.ok(&env.main, &["question", "answer", "1", "Truncated."]);

    let text = env.ok(&env.engine, &["start"]);
    assert!(!text.contains("Answer changed"), "{text}");
    assert_eq!(text.matches("Truncated.").count(), 1, "{text}");
    assert!(text.contains("## Answers to your questions"));
    assert!(text.contains("**#1 (melee.md)** (corrected)"), "{text}");

    let brief = env.json(&env.engine, &["start"]);
    assert_eq!(brief["corrected_answers"], json!([]));
    assert_eq!(brief["answered_questions"][0]["id"], 1);
}

#[test]
fn a_correction_in_the_second_of_the_previous_briefing_is_shown() {
    use airlock::briefing;
    use airlock::config::ProjectConfig;
    use airlock::hub::Project;
    use airlock::store::Store;

    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("airlock.db");
    let mut store = Store::open(&database).unwrap();
    store
        .ask("melee.md", "Arc?", "engine-1", None, 1_000)
        .unwrap();
    store.answer(1, "Rounded.", "lead", None, 1_001).unwrap();
    store.resolve(1, None, 1_002).unwrap();
    store.answer(1, "Truncated.", "lead", None, 1_003).unwrap();
    let corrected_at = store.question(1).unwrap().corrected_at.unwrap();
    assert_eq!(corrected_at, 1_003);

    let project = Project {
        id: "p".into(),
        config: ProjectConfig::default(),
        root: None,
        database,
        binding: None,
    };
    let build =
        |previous| briefing::build(&store, &project, "engine-1", None, previous, 2_000).unwrap();
    // The previous briefing happened in the very second of the correction.
    let same_second = build(Some(corrected_at));
    assert_eq!(same_second.corrected_answers.len(), 1);
    assert!(
        same_second
            .markdown(briefing::Via::Cli)
            .contains("## Answer changed")
    );
    // One second later it was already seen; a first briefing shows everything.
    assert!(build(Some(corrected_at + 1)).corrected_answers.is_empty());
    assert_eq!(build(None).corrected_answers.len(), 1);
}

/// Runs `airlock hook stop` in `dir` as Claude Code does: hook JSON on stdin.
fn stop_hook(env: &Env, dir: &Path) -> (bool, String) {
    stop_hook_with(env, dir, &[])
}

/// `stop_hook` for a Claude Code started with `vars` in its environment.
fn stop_hook_with(env: &Env, dir: &Path, vars: &[(&str, &str)]) -> (bool, String) {
    use std::io::Write;
    let mut child = env
        .cmd(dir, &["hook", "stop"])
        .env_remove("AIRLOCK_ROLE")
        .envs(vars.iter().copied())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"session_id":"abc","hook_event_name":"Stop","stop_hook_active":false}"#)
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), String::from_utf8(out.stdout).unwrap())
}

fn set_auto_continue_limit(env: &Env, limit: u32) {
    let dir = std::fs::read_dir(env.hub.join("projects"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let path = dir.join("project.json");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["auto_continue_limit"] = json!(limit);
    std::fs::write(&path, config.to_string()).unwrap();
}

#[test]
fn stop_hook_continues_once_per_news() {
    let env = Env::new();
    // An unbound directory: silence, exit 0.
    assert_eq!(stop_hook(&env, &env.engine), (true, String::new()));
    env.ok(&env.engine, &["use", "engine", "--lock"]);
    // The briefing starts the session's inbox; nothing new yet.
    env.ok(&env.engine, &["start"]);
    assert_eq!(stop_hook(&env, &env.engine), (true, String::new()));

    // The engine asks; the lead answers; the engine's session is sent back to work once.
    env.ok(
        &env.engine,
        &["question", "ask", "Arc?", "--topic", "melee.md"],
    );
    env.ok(&env.main, &["question", "answer", "1", "Front 90°"]);
    let (ok, out) = stop_hook(&env, &env.engine);
    assert!(ok);
    let decision: Value = serde_json::from_str(&out).expect("one JSON object");
    assert_eq!(decision["decision"], "block");
    let reason = decision["reason"].as_str().unwrap();
    assert!(
        reason.contains("1 new for engine") && reason.contains("answer to #1"),
        "{reason}"
    );
    assert!(!reason.contains("Front"), "references only: {reason}");
    // The same news does not nudge again, read or not.
    assert_eq!(stop_hook(&env, &env.engine), (true, String::new()));

    // New news nudges again, until the hourly limit.
    set_auto_continue_limit(&env, 2);
    env.ok(
        &env.main,
        &[
            "task",
            "add",
            "Hotfix",
            "--role",
            "engine",
            "--priority",
            "critical",
            "--do",
            "x",
            "--verify",
            "y",
        ],
    );
    let (_, out) = stop_hook(&env, &env.engine);
    assert!(out.contains("E-1 ready (critical)"), "{out}");
    env.ok(
        &env.main,
        &[
            "task", "add", "More", "--role", "engine", "--do", "x", "--verify", "y",
        ],
    );
    assert_eq!(
        stop_hook(&env, &env.engine),
        (true, String::new()),
        "limit of 2 reached"
    );
    set_auto_continue_limit(&env, 0);
    env.ok(
        &env.main,
        &[
            "task", "add", "Off", "--role", "engine", "--do", "x", "--verify", "y",
        ],
    );
    assert_eq!(
        stop_hook(&env, &env.engine),
        (true, String::new()),
        "0 turns it off"
    );

    // Nudges are logged as reads with the item count.
    let log = env.ok(&env.main, &["activity", "--json"]);
    let entries: Value = serde_json::from_str(&log).unwrap();
    let hooks: Vec<&Value> = entries["entries"]
        .as_array()
        .or_else(|| entries.as_array())
        .unwrap()
        .iter()
        .filter(|e| e["command"] == "hook stop")
        .collect();
    let outcomes: Vec<String> = hooks
        .iter()
        .map(|e| {
            format!(
                "{} {}",
                e["target"].as_str().unwrap(),
                e["changed"][0].as_str().unwrap()
            )
        })
        .collect();
    // The engine never read its inbox, so the unread items add up.
    assert_eq!(outcomes, ["3 limit", "2 nudge", "1 nudge"], "newest first");
    assert!(hooks.iter().all(|e| e["write"] == false));

    // A broken hub never traps a session: silence, exit 0.
    std::fs::write(env.engine.join(".airlock"), "not json").unwrap();
    assert_eq!(stop_hook(&env, &env.engine), (true, String::new()));
}

#[test]
fn stop_hook_reads_the_session_from_the_environment() {
    let env = Env::new();
    // A shared checkout bound to spec, where a lead session also runs; its
    // Claude Code was started with AIRLOCK_SESSION and AIRLOCK_ROLE.
    env.ok(&env.main, &["use", "spec"]);
    env.ok(&env.engine, &["use", "engine", "--lock"]);
    let lead = [("AIRLOCK_SESSION", "lead-1"), ("AIRLOCK_ROLE", "lead")];
    for dir in [&env.main, &env.engine] {
        env.ok(dir, &["start"]);
    }
    let out = env.cmd(&env.main, &["start"]).envs(lead).output().unwrap();
    assert!(out.status.success());

    env.ok(
        &env.engine,
        &["question", "ask", "Arc?", "--topic", "melee.md"],
    );
    let (_, out) = stop_hook_with(&env, &env.main, &lead);
    assert!(out.contains("1 new for lead"), "{out}");
    // The binding's session has its own cursor and its own nudge.
    let (_, out) = stop_hook(&env, &env.main);
    assert!(out.contains("1 new for spec"), "{out}");
    assert_eq!(
        stop_hook_with(&env, &env.main, &lead),
        (true, String::new())
    );

    // A locked worktree keeps its role, but a second session of it has its own inbox.
    let second = [("AIRLOCK_SESSION", "engine-2"), ("AIRLOCK_ROLE", "lead")];
    let out = env
        .cmd(&env.engine, &["start"])
        .env("AIRLOCK_SESSION", "engine-2")
        .output()
        .unwrap();
    assert!(out.status.success());
    env.ok(&env.main, &["question", "answer", "1", "Front 90°"]);
    let (_, out) = stop_hook_with(&env, &env.engine, &second);
    assert!(out.contains("1 new for engine"), "{out}");
}

#[test]
fn setup_adds_the_stop_hook_on_request_and_doctor_sees_it() {
    let env = Env::new();
    let refused = env.run(&env.engine, &["setup", "claude-hooks", "--auto-continue"]);
    assert!(!refused.status.success(), "no binding, no auto-continue");
    assert!(
        env.run(&env.main, &["setup", "claude", "--auto-continue"])
            .status
            .code()
            == Some(1)
    );
    env.ok(&env.engine, &["use", "engine", "--lock"]);
    let settings: Value =
        serde_json::from_str(&env.ok(&env.engine, &["setup", "claude-hooks", "--auto-continue"]))
            .unwrap();
    hook_command(&settings);
    let stop = settings["hooks"]["Stop"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(
        stop.ends_with(" hook stop") && stop.contains(" -C "),
        "{stop}"
    );
    let plain: Value =
        serde_json::from_str(&env.ok(&env.engine, &["setup", "claude-hooks"])).unwrap();
    assert!(plain["hooks"].get("Stop").is_none(), "opt-in only");

    let dir = env.engine.join(".claude");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("settings.local.json"), plain.to_string()).unwrap();
    let doctor = env.ok(&env.engine, &["doctor"]);
    assert!(doctor.contains("auto-continue off"), "{doctor}");
    std::fs::write(dir.join("settings.local.json"), settings.to_string()).unwrap();
    let doctor = env.ok(&env.engine, &["doctor"]);
    assert!(
        doctor.contains("Stop hook") && doctor.contains("auto-continue on"),
        "{doctor}"
    );
    assert!(!doctor.contains("auto-continue off"), "{doctor}");
}
