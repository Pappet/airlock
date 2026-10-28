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

    fn add_engine_task(&self, id: &str) {
        self.ok(
            &self.main,
            &[
                "task",
                "add",
                id,
                "Engine work",
                "--role",
                "engine",
                "--do",
                "Build",
                "--verify",
                "cargo test",
            ],
        );
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
            "task", "add", "Q-1", "Check", "--role", "qa", "--do", "x", "--verify", "y",
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

    let unbound = env.ok(&env.main, &["start"]);
    assert!(unbound.contains("## No role"));
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
