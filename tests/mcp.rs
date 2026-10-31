//! MCP server over stdio (Spec 03), driven through the real binary.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::Write;
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
            "task", "add", "L-1", "Spec", "--role", "spec", "--do", "x", "--verify", "y",
        ]);
        env.cli(&[
            "task",
            "add",
            "E-1",
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
                json!({ "id": "E-2", "title": "More", "role": "engine", "do": "d", "verify": "v",
                        "blocked_by": ["L-1"], "read": ["docs/spec/a.md"], "priority": 3 }),
            ),
            call(4, "task_claim", json!({ "task_id": "L-1", "lease_minutes": 5 })),
            call(5, "task_heartbeat", json!({ "task_id": "L-1" })),
            call(6, "task_complete", json!({ "task_id": "L-1", "notes": "spec written" })),
            call(7, "task_next", json!({ "role": "engine" })),
            call(8, "task_get", json!({ "task_id": "L-1" })),
            call(9, "question_ask", json!({ "topic": "melee.md", "question": "Arc?" })),
            call(10, "question_answer", json!({ "question_id": 1, "answer": "Truncated.", "spec_file": "docs/spec/melee.md" })),
            call(11, "question_resolve", json!({ "question_id": 1 })),
            call(12, "question_list", json!({ "status": "resolved" })),
            call(13, "doc_sync", json!({})),
            call(14, "task_claim", json!({ "task_id": "nope" })),
        ],
    );
    assert_eq!(s.tool_names(1).len(), 14);
    assert!(s.tool_error(2).contains("missing required argument `role`"));
    assert_eq!(s.ok(3)["status"], "blocked");
    assert!(s.ok(4)["lease_expires_at"].is_i64());
    assert_eq!(s.ok(6)["now_ready"], json!(["E-2"]));
    assert_eq!(s.ok(7)["task"]["id"], "E-2", "priority 3 beats E-1");
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
                        "blocked_by": ["L-1"], "priority": 2 }),
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
                json!({ "task_id": "L-1", "blocked_by": ["L-1"] }),
            ),
            call(6, "task_claim", json!({ "task_id": "L-1" })),
            call(
                7,
                "task_update",
                json!({ "task_id": "L-1", "do": "Changed" }),
            ),
            call(
                8,
                "task_update",
                json!({ "task_id": "L-1", "do": "Changed", "force": true }),
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
            edited["priority"].as_i64()
        ),
        (Some("Renamed"), Some("blocked"), Some(2))
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
            call(4, "task_get", json!({ "task_id": "L-1" })),
            call(5, "task_claim", json!({ "task_id": "L-1" })),
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
        "task", "add", "L-2", "Plan", "--role", "lead", "--do", "x", "--verify", "y",
    ]);
    let s = env.session(
        &["--role", "lead"],
        &[
            request(1, "tools/list", json!({})),
            call(
                2,
                "task_create",
                json!({ "id": "E-9", "title": "t", "role": "engine", "do": "d", "verify": "v" }),
            ),
            call(3, "task_claim", json!({ "task_id": "E-1" })),
            call(4, "task_get", json!({ "task_id": "E-1" })),
            call(5, "task_claim", json!({ "task_id": "L-2" })),
        ],
    );
    assert_eq!(s.tool_names(1).len(), 14);
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
        env.cli(&["task", "show", "L-2"])
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
