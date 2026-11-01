//! MCP server over stdio (Spec 03), driven through the real binary.

use airlock::activity::{ActivityLog, Channel, Filter};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;

struct Env {
    _tmp: TempDir,
    hub: PathBuf,
    project: PathBuf,
}

impl Env {
    /// A project with roles lead/spec/engine (engine is clean-room) and two tasks.
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let hub = tmp.path().join("hub");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let env = Self {
            _tmp: tmp,
            hub,
            project,
        };
        env.cli(&["init"]);
        env.cli(&[
            "task", "add", "Spec", "--role", "spec", "--do", "x", "--verify", "y",
        ]);
        env.cli(&[
            "task",
            "add",
            "Engine",
            "--role",
            "engine",
            "--do",
            "Build",
            "--verify",
            "cargo test",
        ]);
        env
    }

    /// The activity log of the (only) project in the hub.
    fn activity(&self) -> ActivityLog {
        let projects = self.hub.join("projects");
        let dir = std::fs::read_dir(&projects)
            .unwrap()
            .next()
            .expect("a project in the hub")
            .unwrap()
            .path();
        ActivityLog::open(&dir.join("activity.db")).unwrap()
    }

    fn command(&self, cwd: &Path, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_airlock"));
        cmd.args(args)
            .current_dir(cwd)
            .env("AIRLOCK_HOME", &self.hub)
            .env_remove("AIRLOCK_PROJECT")
            .env_remove("AIRLOCK_SESSION");
        cmd
    }

    fn cli(&self, args: &[&str]) -> String {
        let out = self.command(&self.project, args).output().unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// Runs one server session: sends `lines`, closes stdin, returns responses by id.
    fn session(&self, args: &[&str], lines: &[String]) -> Session {
        let mut argv = vec!["mcp"];
        argv.extend_from_slice(args);
        let mut child = self
            .command(&self.project, &argv)
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
        assert!(
            out.status.success(),
            "server failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        let mut responses = HashMap::new();
        let mut count = 0;
        for line in String::from_utf8(out.stdout).unwrap().lines() {
            let message: Value = serde_json::from_str(line).expect("stdout carries only JSON-RPC");
            responses.insert(message["id"].to_string(), message);
            count += 1;
        }
        Session { responses, count }
    }
}

struct Session {
    responses: HashMap<String, Value>,
    count: usize,
}

impl Session {
    fn get(&self, id: u64) -> &Value {
        self.responses
            .get(&id.to_string())
            .unwrap_or_else(|| panic!("no response for id {id}"))
    }

    /// The structured result of a successful tool call.
    fn ok(&self, id: u64) -> &Value {
        let result = &self.get(id)["result"];
        assert_eq!(result["isError"], false, "call {id} failed: {result}");
        &result["structuredContent"]
    }

    /// The error text of a failed tool call.
    fn tool_error(&self, id: u64) -> String {
        let result = &self.get(id)["result"];
        assert_eq!(result["isError"], true, "call {id} should fail: {result}");
        result["content"][0]["text"].as_str().unwrap().to_string()
    }

    fn tool_names(&self, id: u64) -> Vec<String> {
        self.get(id)["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    }
}

fn request(id: u64, method: &str, params: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string()
}

fn call(id: u64, tool: &str, arguments: Value) -> String {
    request(
        id,
        "tools/call",
        json!({ "name": tool, "arguments": arguments }),
    )
}

fn initialize(id: u64, version: &str) -> String {
    request(
        id,
        "initialize",
        json!({ "protocolVersion": version, "capabilities": {}, "clientInfo": { "name": "test", "version": "0" } }),
    )
}

#[test]
fn handshake_negotiates_versions_and_ignores_notifications() {
    let env = Env::new();
    let s = env.session(
        &[],
        &[
            initialize(1, "2025-06-18"),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string(),
            initialize(2, "1999-01-01"),
            request(3, "ping", json!({})),
        ],
    );
    assert_eq!(s.count, 3, "no response to the notification");
    let init = &s.get(1)["result"];
    assert_eq!(init["protocolVersion"], "2025-06-18");
    assert_eq!(init["serverInfo"]["name"], "airlock");
    assert!(init["capabilities"]["tools"].is_object());
    assert!(init["instructions"].as_str().unwrap().contains("task_next"));
    assert_eq!(
        s.get(2)["result"]["protocolVersion"],
        "2025-11-25",
        "unknown version: newest offered"
    );
    assert_eq!(s.get(3)["result"], json!({}));
}

#[test]
fn protocol_errors_use_json_rpc_codes() {
    let env = Env::new();
    let s = env.session(
        &[],
        &[
            "not json".to_string(),
            request(1, "resources/list", json!({})),
            call(2, "no_such_tool", json!({})),
            request(
                3,
                "tools/call",
                json!({ "name": "task_get", "arguments": [1] }),
            ),
        ],
    );
    assert_eq!(
        s.responses["null"]["error"]["code"], -32700,
        "parse error has id null"
    );
    assert_eq!(s.get(1)["error"]["code"], -32601);
    assert_eq!(s.get(2)["error"]["code"], -32602);
    assert_eq!(s.get(3)["error"]["code"], -32602);
}

#[test]
fn malformed_requests_are_answered_but_client_responses_are_not() {
    let env = Env::new();
    let s = env.session(
        &[],
        &[
            json!({ "jsonrpc": "2.0", "id": 1 }).to_string(),
            json!({ "jsonrpc": "2.0", "id": 2, "method": 42 }).to_string(),
            json!({ "jsonrpc": "2.0", "id": 3, "result": {} }).to_string(),
            json!({ "jsonrpc": "2.0", "id": 4, "error": { "code": 1, "message": "x" } })
                .to_string(),
            request(5, "ping", json!({})),
        ],
    );
    assert_eq!(s.get(1)["error"]["code"], -32600);
    assert_eq!(s.get(2)["error"]["code"], -32600);
    assert_eq!(s.count, 3, "responses 3 and 4 are not answered");
    assert_eq!(s.get(5)["result"], json!({}));
}

#[test]
fn unbound_session_runs_the_full_workflow() {
    let env = Env::new();
    let s = env.session(
        &["--session", "lead-1"],
        &[
            request(1, "tools/list", json!({})),
            call(2, "task_next", json!({})),
            call(
                3,
                "task_create",
                json!({ "title": "More", "role": "engine", "do": "d", "verify": "v",
                        "blocked_by": ["S-1"], "read": ["docs/spec/a.md"], "priority": "high" }),
            ),
            call(4, "task_claim", json!({ "task_id": "S-1", "lease_minutes": 5 })),
            call(5, "task_heartbeat", json!({ "task_id": "S-1" })),
            call(6, "task_complete", json!({ "task_id": "S-1", "notes": "spec written" })),
            call(7, "task_next", json!({ "role": "engine" })),
            call(8, "task_get", json!({ "task_id": "S-1" })),
            call(9, "question_ask", json!({ "topic": "melee.md", "question": "Arc?" })),
            call(10, "question_answer", json!({ "question_id": 1, "answer": "Truncated.", "spec_file": "docs/spec/melee.md" })),
            call(11, "question_resolve", json!({ "question_id": 1 })),
            call(12, "question_list", json!({ "status": "resolved" })),
            call(13, "doc_sync", json!({})),
            call(14, "task_claim", json!({ "task_id": "nope" })),
            call(
                15,
                "task_create",
                json!({ "id": "E-7", "title": "Own id", "role": "engine", "do": "d", "verify": "v" }),
            ),
            call(
                16,
                "task_create",
                json!({ "title": "Numeric", "role": "engine", "do": "d", "verify": "v", "priority": 22 }),
            ),
        ],
    );
    assert_eq!(s.tool_names(1).len(), 18);
    let own_id = s.tool_error(15);
    assert!(own_id.contains("ids are assigned by Airlock"), "{own_id}");
    let numeric = s.tool_error(16);
    assert!(
        numeric.contains("critical, high, normal, or low"),
        "{numeric}"
    );
    assert!(s.tool_error(2).contains("missing required argument `role`"));
    assert_eq!(s.ok(3)["id"], "E-2", "the result carries the new id");
    assert_eq!(s.ok(3)["status"], "blocked");
    assert!(s.ok(4)["lease_expires_at"].is_i64());
    assert_eq!(s.ok(6)["now_ready"], json!(["E-2"]));
    assert_eq!(s.ok(7)["task"]["id"], "E-2", "a high priority beats E-1");
    assert_eq!(s.ok(7)["task"]["read"], json!(["docs/spec/a.md"]));
    let l1 = s.ok(8);
    assert_eq!(
        (l1["status"].as_str(), l1["notes"].as_str()),
        (Some("done"), Some("spec written"))
    );
    assert_eq!(l1["dependents"], json!(["E-2"]));
    assert_eq!(s.ok(9)["question_id"], 1);
    assert_eq!(s.ok(10)["answered_by"], "lead-1");
    assert_eq!(s.ok(12)["questions"][0]["spec_file"], "docs/spec/melee.md");
    assert_eq!(s.ok(13)["written"].as_array().unwrap().len(), 3);
    assert!(env.project.join("docs/PLAN.md").is_file());
    assert!(s.tool_error(14).contains("no task `nope`"));
}

#[test]
fn task_update_edits_content_and_needs_force_for_started_tasks() {
    let env = Env::new();
    let s = env.session(
        &["--session", "lead-1"],
        &[
            call(
                1,
                "task_update",
                json!({ "task_id": "E-1", "title": "Renamed", "touch": ["src/e.rs"],
                        "blocked_by": ["S-1"], "priority": "high" }),
            ),
            call(
                2,
                "task_update",
                json!({ "task_id": "E-1", "blocked_by": [] }),
            ),
            call(3, "task_update", json!({ "task_id": "E-1" })),
            call(
                4,
                "task_update",
                json!({ "task_id": "E-1", "role": "nope" }),
            ),
            call(
                5,
                "task_update",
                json!({ "task_id": "S-1", "blocked_by": ["S-1"] }),
            ),
            call(6, "task_claim", json!({ "task_id": "S-1" })),
            call(
                7,
                "task_update",
                json!({ "task_id": "S-1", "do": "Changed" }),
            ),
            call(
                8,
                "task_update",
                json!({ "task_id": "S-1", "do": "Changed", "force": true }),
            ),
            call(
                9,
                "task_update",
                json!({ "task_id": "E-1", "read": "a.md" }),
            ),
        ],
    );
    let edited = s.ok(1);
    assert_eq!(
        (
            edited["title"].as_str(),
            edited["status"].as_str(),
            edited["priority"].as_str()
        ),
        (Some("Renamed"), Some("blocked"), Some("high"))
    );
    assert_eq!(edited["touch"], json!(["src/e.rs"]));
    assert_eq!(edited["do"], "Build", "omitted fields are kept");
    assert_eq!(s.ok(2)["blocked_by"], json!([]), "an empty list clears");
    assert_eq!(s.ok(2)["status"], "ready");
    assert!(s.tool_error(3).contains("nothing to change"));
    assert!(s.tool_error(4).contains("unknown role"));
    assert!(s.tool_error(5).contains("dependency cycle"));
    assert!(s.tool_error(7).contains("force"));
    assert_eq!(s.ok(8)["do"], "Changed");
    assert_eq!(s.ok(8)["claimed_by"], "lead-1", "the claim survives");
    assert!(s.tool_error(9).contains("array of strings"));
}

#[test]
fn overlapping_work_warns_and_asks_the_lead() {
    let env = Env::new();
    // Added after E-1: E-2 (normal) and E-3 (high).
    for priority in ["normal", "high"] {
        env.cli(&[
            "task",
            "add",
            "Sim",
            "--role",
            "engine",
            "--do",
            "x",
            "--verify",
            "y",
            "--touch",
            "src/sim.rs",
            "--priority",
            priority,
        ]);
    }
    env.cli(&["--session", "engine-2", "task", "claim", "E-2"]);

    // E-3 outranks E-1 but shares the occupied src/sim.rs; E-1 has no touch
    // data, so its overlap is unknown and it is not held back.
    let first = env.session(
        &["--role", "engine", "--session", "engine-1"],
        &[call(1, "task_next", json!({}))],
    );
    assert_eq!(first.ok(1)["task"]["id"], "E-1");
    assert!(first.ok(1)["task"].get("overlaps").is_none());

    env.cli(&["--session", "engine-3", "task", "claim", "E-1"]);
    let s = env.session(
        &["--role", "engine", "--session", "engine-1"],
        &[
            call(1, "session_start", json!({})),
            call(2, "task_next", json!({})),
            call(3, "task_claim", json!({ "task_id": "E-3" })),
            call(4, "question_list", json!({})),
        ],
    );
    let brief = &s.get(1)["result"];
    assert_eq!(
        brief["structuredContent"]["next"]["overlaps"][0]["id"],
        "E-2"
    );
    let text = brief["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("src/sim.rs with E-2"), "{text}");
    let next = &s.ok(2)["task"];
    assert_eq!(next["id"], "E-3", "nothing clean left: E-3 anyway");
    assert_eq!(next["overlaps"][0]["paths"], json!(["src/sim.rs"]));

    let claimed = s.ok(3);
    assert_eq!(claimed["overlaps"][0]["id"], "E-2");
    let question_id = claimed["question_id"].as_u64().unwrap();
    let questions = s.ok(4)["questions"].as_array().unwrap().clone();
    assert_eq!(
        questions.len(),
        1,
        "the clean-room session sees its overlap question"
    );
    assert_eq!(questions[0]["id"].as_u64(), Some(question_id));
    assert_eq!(questions[0]["topic"], "overlap:E-3");
}

#[test]
fn clean_room_session_gets_a_restricted_tool_set() {
    let env = Env::new();
    env.cli(&[
        "question",
        "ask",
        "Lead's own question?",
        "--topic",
        "plan.md",
    ]);
    let s = env.session(
        &["--role", "engine", "--session", "engine-1"],
        &[
            request(1, "tools/list", json!({})),
            call(2, "task_next", json!({})),
            call(3, "task_next", json!({ "role": "spec" })),
            call(4, "task_get", json!({ "task_id": "S-1" })),
            call(5, "task_claim", json!({ "task_id": "S-1" })),
            call(
                6,
                "question_answer",
                json!({ "question_id": 1, "answer": "x" }),
            ),
            call(7, "task_create", json!({})),
            call(11, "task_update", json!({ "task_id": "E-1", "title": "x" })),
            call(
                8,
                "question_ask",
                json!({ "topic": "melee.md", "question": "Arc?" }),
            ),
            call(9, "question_list", json!({})),
            call(10, "task_claim", json!({ "task_id": "E-1" })),
        ],
    );
    assert_eq!(
        s.tool_names(1),
        [
            "session_start",
            "task_next",
            "task_get",
            "task_claim",
            "task_heartbeat",
            "task_release",
            "task_complete",
            "question_ask",
            "question_list"
        ]
    );
    assert_eq!(s.ok(2)["task"]["id"], "E-1", "role filled from the binding");
    assert!(s.tool_error(3).contains("bound to role `engine`"));
    for id in [4, 5] {
        let error = s.tool_error(id);
        assert!(error.contains("not available to role `engine`"), "{error}");
        assert!(
            !error.contains("spec"),
            "foreign role not revealed: {error}"
        );
    }
    for id in [6, 7, 11] {
        assert_eq!(
            s.get(id)["error"]["code"],
            -32602,
            "hidden tools are unknown"
        );
    }
    let questions = s.ok(9)["questions"].as_array().unwrap().clone();
    assert_eq!(questions.len(), 1, "only the session's own questions");
    assert_eq!(questions[0]["asked_by"], "engine-1");
    assert!(s.ok(10)["lease_expires_at"].is_i64());
}

#[test]
fn bound_lead_session_keeps_all_tools_but_works_only_its_own_tasks() {
    let env = Env::new();
    env.cli(&[
        "task", "add", "Plan", "--role", "lead", "--do", "x", "--verify", "y",
    ]);
    let s = env.session(
        &["--role", "lead"],
        &[
            request(1, "tools/list", json!({})),
            call(
                2,
                "task_create",
                json!({ "title": "t", "role": "engine", "do": "d", "verify": "v" }),
            ),
            call(3, "task_claim", json!({ "task_id": "E-1" })),
            call(4, "task_get", json!({ "task_id": "E-1" })),
            call(5, "task_claim", json!({ "task_id": "L-1" })),
        ],
    );
    assert_eq!(s.tool_names(1).len(), 18);
    assert_eq!(
        s.ok(2)["role"],
        "engine",
        "leads create work for other roles"
    );
    assert!(s.tool_error(3).contains("bound to `lead`"));
    assert_eq!(s.ok(4)["id"], "E-1", "reading is not restricted");
    let lease = s.ok(5);
    assert!(lease["lease_expires_at"].is_i64());
    assert!(
        env.cli(&["task", "show", "L-1"])
            .contains("claimed by: mcp-"),
        "generated session id"
    );
}

#[test]
fn startup_fails_without_project_or_with_unknown_role() {
    let env = Env::new();
    let outside = env.project.parent().unwrap();
    let out = env
        .command(outside, &["mcp"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not inside an Airlock project"));

    let out = env
        .command(&env.project, &["mcp", "--role", "ghost"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown role `ghost`"));
}

#[test]
fn tracks_through_mcp() {
    let env = Env::new();
    let s = env.session(
        &["--session", "lead-1"],
        &[
            request(1, "tools/list", json!({})),
            call(
                2,
                "track_create",
                json!({ "name": "core", "priority": "critical", "description": "The battle" }),
            ),
            call(3, "track_create", json!({ "name": "comfort", "priority": 2 })),
            call(
                4,
                "task_create",
                json!({ "title": "t", "role": "engine", "do": "d", "verify": "v", "track": "core" }),
            ),
            call(5, "track_list", json!({})),
            call(6, "task_update", json!({ "task_id": "E-2", "track": null })),
            call(
                7,
                "task_update",
                json!({ "task_id": "E-2", "track": "nope" }),
            ),
            call(
                8,
                "track_update",
                json!({ "name": "core", "new_name": "heart", "priority": "high" }),
            ),
            call(9, "task_get", json!({ "task_id": "E-2" })),
            call(10, "track_remove", json!({ "name": "heart" })),
            call(11, "track_update", json!({ "name": "heart" })),
            call(12, "task_update", json!({ "task_id": "E-2", "track": "heart" })),
            call(13, "track_remove", json!({ "name": "heart" })),
            call(14, "track_create", json!({ "name": "bad name" })),
        ],
    );
    let names = s.tool_names(1);
    for tool in ["track_list", "track_create", "track_update", "track_remove"] {
        assert!(names.contains(&tool.to_string()), "{tool}");
    }
    assert_eq!(s.ok(2)["name"], "core");
    assert_eq!(s.ok(2)["priority"], "critical");
    let numeric = s.tool_error(3);
    assert!(
        numeric.contains("critical, high, normal, or low"),
        "{numeric}"
    );
    assert_eq!(s.ok(4)["track"], "core");
    let tracks = s.ok(5)["tracks"].as_array().unwrap().clone();
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0]["counts"]["ready"], 1);
    assert!(
        s.ok(6).get("track").is_none_or(Value::is_null),
        "null clears"
    );
    assert!(s.tool_error(7).contains("core"), "lists the defined tracks");
    assert_eq!(s.ok(8)["name"], "heart");
    assert_eq!(s.ok(8)["priority"], "high");
    assert!(s.ok(9).get("track").is_none_or(Value::is_null));
    assert_eq!(s.ok(10)["removed"], "heart");
    assert!(s.tool_error(11).contains("nothing to change"));
    assert!(s.tool_error(12).contains("no track `heart`"));
    assert!(s.tool_error(13).contains("no track `heart`"));
    assert!(s.tool_error(14).contains("track name"));
}

#[test]
fn clean_room_sessions_get_no_track_tools() {
    let env = Env::new();
    let s = env.session(
        &["--role", "engine", "--session", "engine-1"],
        &[
            request(1, "tools/list", json!({})),
            call(2, "track_list", json!({})),
            call(3, "track_create", json!({ "name": "x" })),
        ],
    );
    assert!(s.tool_names(1).iter().all(|n| !n.starts_with("track_")));
    for id in [2, 3] {
        assert_eq!(s.get(id)["error"]["code"], -32602);
    }
}

/// MCP entries of the log in call order (the log reads newest first); the
/// CLI calls that set a test up are logged too, but are not what these check.
fn logged(env: &Env) -> Vec<airlock::activity::Entry> {
    let filter = Filter {
        channel: Some(Channel::Mcp),
        ..Filter::default()
    };
    let mut entries = env.activity().entries(&filter).unwrap();
    entries.reverse();
    entries
}

#[test]
fn every_tool_call_is_logged_with_session_role_and_result() {
    let env = Env::new();
    env.cli(&[
        "task", "add", "Plan", "--role", "lead", "--do", "x", "--verify", "y",
    ]);
    env.session(
        &["--role", "lead", "--session", "lead-1"],
        &[
            call(1, "task_claim", json!({ "task_id": "L-1" })),
            call(2, "task_complete", json!({ "task_id": "L-1" })),
            call(3, "task_claim", json!({ "task_id": "L-1" })),
            call(
                4,
                "task_update",
                json!({ "task_id": "S-1", "title": "T", "priority": "high" }),
            ),
        ],
    );
    let entries = logged(&env);
    assert_eq!(entries.len(), 4, "{entries:?}");
    for entry in &entries {
        assert_eq!(entry.channel, Channel::Mcp);
        assert_eq!(entry.session, "lead-1");
        assert_eq!(entry.role.as_deref(), Some("lead"));
        assert!(!entry.clean_room);
        assert!(entry.write);
        assert!(entry.pid > 0);
        assert_eq!(entry.build, airlock::activity::build());
    }
    assert_eq!(entries[0].command, "task_claim");
    assert_eq!(entries[0].target.as_deref(), Some("L-1"));
    assert!(entries[0].ok && entries[0].error.is_none());
    assert!(entries[1].ok);
    assert!(!entries[2].ok, "claiming a done task fails");
    let error = entries[2].error.as_deref().unwrap();
    assert!(error.contains("L-1"), "the store's error text: {error}");
    assert_eq!(entries[3].command, "task_update");
    assert_eq!(entries[3].target.as_deref(), Some("S-1"));
    assert_eq!(entries[3].changed, ["priority: high", "title"]);
}

#[test]
fn clean_room_hidden_tool_is_logged_as_failure() {
    let env = Env::new();
    env.session(
        &["--role", "engine", "--session", "engine-1"],
        &[call(1, "task_create", json!({}))],
    );
    let entries = logged(&env);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].command, "task_create");
    assert!(entries[0].clean_room);
    assert!(!entries[0].ok);
    assert!(
        entries[0]
            .error
            .as_deref()
            .unwrap()
            .contains("unknown tool"),
        "{entries:?}"
    );
}

#[test]
fn session_start_and_prompt_are_logged_as_briefings() {
    let env = Env::new();
    env.session(
        &["--role", "engine", "--session", "engine-1"],
        &[
            call(1, "session_start", json!({})),
            request(2, "prompts/get", json!({ "name": "start" })),
        ],
    );
    let entries = logged(&env);
    let commands: Vec<_> = entries.iter().map(|e| e.command.as_str()).collect();
    assert_eq!(commands, ["session_start", "prompt start"]);
    for entry in &entries {
        assert!(entry.ok && !entry.write, "{entry:?}");
        assert_eq!(entry.session, "engine-1");
    }
    assert!(env.activity().last_briefing("engine-1").unwrap().is_some());
}

#[test]
fn mcp_log_holds_no_free_text() {
    let env = Env::new();
    env.cli(&["question", "ask", "Why?", "--topic", "plan.md"]);
    env.session(
        &["--session", "lead-1"],
        &[
            call(
                1,
                "task_create",
                json!({ "title": "SECRET-TITLE", "role": "engine", "do": "SECRET-DO-TEXT",
                        "verify": "SECRET-VERIFY",
                        "read": ["SECRET-READ"] }),
            ),
            call(
                2,
                "question_answer",
                json!({ "question_id": 1, "answer": "SECRET-ANSWER", "spec_file": "SECRET-SPEC" }),
            ),
            call(3, "task_claim", json!({ "task_id": "S-1" })),
            call(
                4,
                "task_complete",
                json!({ "task_id": "S-1", "notes": "SECRET-NOTES" }),
            ),
            call(
                5,
                "track_create",
                json!({ "name": "t", "description": "SECRET-DESCRIPTION" }),
            ),
        ],
    );
    let entries = logged(&env);
    assert_eq!(entries.len(), 5);
    // Error messages are kept verbatim and may echo input, so every call here
    // must succeed for the scan below to prove anything about arguments.
    assert!(entries.iter().all(|e| e.ok), "{entries:?}");
    assert_eq!(entries[1].target.as_deref(), Some("#1"));
    assert_eq!(entries[1].changed, ["answer", "spec_file"]);
    // Scan the raw bytes of every file the log lives in: the database and
    // its write-ahead log, whichever holds the pages.
    drop(entries);
    let log_dir = std::fs::read_dir(env.hub.join("projects"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut scanned = 0;
    for file in std::fs::read_dir(&log_dir).unwrap() {
        let file = file.unwrap().path();
        if !file
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("activity.db")
        {
            continue;
        }
        scanned += 1;
        let bytes = std::fs::read(&file).unwrap();
        for secret in ["SECRET", "Why?"] {
            assert!(
                !bytes.windows(secret.len()).any(|w| w == secret.as_bytes()),
                "{} holds {secret}",
                file.display()
            );
        }
    }
    assert!(scanned >= 1);
}

#[test]
fn mcp_process_registers_while_running() {
    let env = Env::new();
    let mut child = env
        .command(
            &env.project,
            &["mcp", "--session", "lead-1", "--role", "lead"],
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    writeln!(stdin, "{}", request(1, "ping", json!({}))).unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert!(line.contains("\"result\""), "{line}");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let running = env.activity().processes(now).unwrap();
    assert_eq!(running.len(), 1, "{running:?}");
    assert_eq!(running[0].kind, "mcp");
    assert_eq!(running[0].session.as_deref(), Some("lead-1"));
    assert_eq!(running[0].role.as_deref(), Some("lead"));
    assert_eq!(running[0].pid, child.id());

    drop(stdin);
    assert!(child.wait().unwrap().success());
    assert!(env.activity().processes(now).unwrap().is_empty());
}

#[test]
fn agent_is_taken_from_client_info() {
    let env = Env::new();
    env.session(
        &["--session", "lead-1"],
        &[
            call(1, "track_list", json!({})),
            request(
                2,
                "initialize",
                json!({ "protocolVersion": "2025-06-18", "capabilities": {},
                        "clientInfo": { "name": "claude-code", "version": "2" } }),
            ),
            call(3, "track_list", json!({})),
        ],
    );
    let entries = logged(&env);
    assert_eq!(entries[0].agent, None);
    assert_eq!(entries[1].agent.as_deref(), Some("claude-code"));
}

#[test]
fn question_answer_on_resolved_question_corrects_it() {
    let env = Env::new();
    let s = env.session(
        &["--session", "lead-1"],
        &[
            call(
                1,
                "question_ask",
                json!({ "topic": "melee.md", "question": "Arc?" }),
            ),
            call(
                2,
                "question_answer",
                json!({ "question_id": 1, "answer": "Rounded." }),
            ),
            call(3, "question_resolve", json!({ "question_id": 1 })),
            call(
                4,
                "question_answer",
                json!({ "question_id": 1, "answer": "Truncated." }),
            ),
        ],
    );
    let corrected = s.ok(4);
    assert_eq!(corrected["status"], "resolved");
    assert_eq!(corrected["answer"], "Truncated.");
    assert_eq!(corrected["history"][0]["answer"], "Rounded.");
    assert_eq!(corrected["corrected_by"], "lead-1");

    let entries = logged(&env);
    assert_eq!(
        entries[1].changed,
        ["answer"],
        "first answer is not a correction"
    );
    assert_eq!(entries[3].command, "question_answer");
    assert!(entries[3].ok, "{:?}", entries[3]);
    assert_eq!(entries[3].changed, ["answer", "correction"]);
}
