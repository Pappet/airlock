//! Dashboard server: security checks, REST API, live events (Spec 04).

use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;
use tempfile::TempDir;

struct Server {
    child: Child,
    port: u16,
    token: String,
    hub: PathBuf,
    project: PathBuf,
    _tmp: TempDir,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn airlock(hub: &Path, project: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_airlock"));
    cmd.args(args)
        .current_dir(project)
        .env("AIRLOCK_HOME", hub)
        .env_remove("AIRLOCK_PROJECT")
        .env_remove("AIRLOCK_SESSION")
        .env_remove("AIRLOCK_ROLE");
    cmd
}

struct Response {
    status: u16,
    head: String,
    body: String,
}

impl Response {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {}", self.body))
    }
}

impl Server {
    fn start() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let hub = tmp.path().join("hub");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let cmd = |args: &[&str]| airlock(&hub, &project, args);
        let init = cmd(&["init", "--name", "demo"]).output().unwrap();
        assert!(
            init.status.success(),
            "{}",
            String::from_utf8_lossy(&init.stderr)
        );
        let mut child = cmd(&["serve", "--port", "0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn serve");
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let mut server = Self {
            child,
            port: 0,
            token: String::new(),
            hub,
            project,
            _tmp: tmp,
        };
        let url = line
            .split_whitespace()
            .find(|w| w.starts_with("http://"))
            .unwrap_or_else(|| panic!("no URL in {line:?}"));
        let rest = url.strip_prefix("http://127.0.0.1:").expect("loopback URL");
        let (port, token) = rest.split_once("/#token=").expect("token in fragment");
        server.port = port.parse().unwrap();
        server.token = token.to_string();
        assert_eq!(server.token.len(), 64, "256-bit hex token");
        server
    }

    fn command(&self, args: &[&str]) -> Command {
        airlock(&self.hub, &self.project, args)
    }

    fn cli(&self, args: &[&str]) -> Output {
        let out = self.command(args).output().expect("run airlock");
        assert!(
            out.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    /// Sends one request with exactly `headers` (plus framing headers).
    fn raw(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Response {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut request = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\n");
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
        stream.write_all(request.as_bytes()).unwrap();
        let mut text = String::new();
        stream.read_to_string(&mut text).unwrap();
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("bad response: {text:?}"));
        Response {
            status,
            head: head.to_string(),
            body: body.to_string(),
        }
    }

    /// An authorized API request with a JSON body for `POST`.
    fn api(&self, method: &str, path: &str, body: Option<Value>) -> Response {
        let auth = format!("Bearer {}", self.token);
        let host = self.host();
        let mut headers = vec![("Host", host.as_str()), ("Authorization", auth.as_str())];
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        if method == "POST" {
            headers.push(("Content-Type", "application/json"));
        }
        self.raw(method, path, &headers, &body)
    }
}

#[test]
fn serves_dashboard_with_allowed_host() {
    let s = Server::start();
    for host in [
        s.host(),
        format!("localhost:{}", s.port),
        format!("[::1]:{}", s.port),
    ] {
        let r = s.raw("GET", "/", &[("Host", &host)], "");
        assert_eq!(r.status, 200, "{host}");
        assert!(r.body.contains("<!doctype html>"), "{}", r.body);
    }
    for asset in ["/app.js", "/style.css"] {
        assert_eq!(s.raw("GET", asset, &[("Host", &s.host())], "").status, 200);
    }
}

#[test]
fn rejects_foreign_host() {
    let s = Server::start();
    for host in ["evil.example", "127.0.0.1:9999", "localhost"] {
        assert_eq!(
            s.raw("GET", "/", &[("Host", host)], "").status,
            403,
            "{host}"
        );
    }
    let auth = format!("Bearer {}", s.token);
    let r = s.raw(
        "GET",
        "/api/tasks",
        &[("Host", "evil.example"), ("Authorization", &auth)],
        "",
    );
    assert_eq!(r.status, 403);
}

#[test]
fn api_requires_token() {
    let s = Server::start();
    let host = s.host();
    assert_eq!(
        s.raw("GET", "/api/tasks", &[("Host", &host)], "").status,
        401
    );
    let wrong = format!("Bearer {}", "0".repeat(64));
    let r = s.raw(
        "GET",
        "/api/tasks",
        &[("Host", &host), ("Authorization", &wrong)],
        "",
    );
    assert_eq!(r.status, 401);
    assert_eq!(r.json()["error"], "missing or invalid access token");
    assert_eq!(
        s.raw("GET", "/api/events", &[("Host", &host)], "").status,
        401
    );
    assert_eq!(s.api("GET", "/api/tasks", None).status, 200);
}

#[test]
fn rejects_foreign_origin() {
    let s = Server::start();
    let auth = format!("Bearer {}", s.token);
    let host = s.host();
    let r = s.raw(
        "GET",
        "/api/tasks",
        &[
            ("Host", &host),
            ("Authorization", &auth),
            ("Origin", "http://evil.example"),
        ],
        "",
    );
    assert_eq!(r.status, 403);
    let own = format!("http://{host}");
    let r = s.raw(
        "GET",
        "/api/tasks",
        &[("Host", &host), ("Authorization", &auth), ("Origin", &own)],
        "",
    );
    assert_eq!(r.status, 200);
}

#[test]
fn post_requires_json() {
    let s = Server::start();
    let auth = format!("Bearer {}", s.token);
    let host = s.host();
    let r = s.raw(
        "POST",
        "/api/tasks",
        &[
            ("Host", &host),
            ("Authorization", &auth),
            ("Content-Type", "text/plain"),
        ],
        "{}",
    );
    assert_eq!(r.status, 415);
}

#[test]
fn no_cors_headers() {
    let s = Server::start();
    let r = s.api("GET", "/api/tasks", None);
    assert!(
        !r.head.to_ascii_lowercase().contains("access-control-"),
        "{}",
        r.head
    );
}

#[test]
fn port_in_use_fails() {
    let s = Server::start();
    let out = s
        .command(&["serve", "--port", &s.port.to_string()])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(&s.port.to_string()), "{stderr}");
}

fn add_task(s: &Server, id: &str, touch: &[&str]) {
    let mut args = vec![
        "task",
        "add",
        id,
        "Title",
        "--role",
        "engine",
        "--do",
        "Do it",
        "--verify",
        "cargo test",
    ];
    for path in touch {
        args.extend(["--touch", path]);
    }
    s.cli(&args);
}

fn submit_for_review(s: &Server, id: &str) {
    s.cli(&["--session", "s1", "task", "claim", id]);
    s.cli(&["--session", "s1", "task", "done", id, "--review"]);
}

#[test]
fn create_list_show_task() {
    let s = Server::start();
    let created = s.api(
        "POST",
        "/api/tasks",
        Some(serde_json::json!({
            "id": "A", "title": "Alpha", "role": "engine", "do": "Do it",
            "verify": "cargo test", "touch": ["src/a.rs"], "priority": 3
        })),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    assert_eq!(created.json()["status"], "ready");
    s.api(
        "POST",
        "/api/tasks",
        Some(serde_json::json!({
            "id": "B", "title": "Beta", "role": "engine", "do": "x", "verify": "y",
            "blocked_by": ["A"]
        })),
    );

    let list = s.api("GET", "/api/tasks", None).json();
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert_eq!(list[0]["priority"], 3);
    assert_eq!(list[1]["status"], "blocked");
    let blocked = s.api("GET", "/api/tasks?status=blocked", None).json();
    assert_eq!(blocked.as_array().unwrap().len(), 1);
    let lead = s.api("GET", "/api/tasks?role=lead", None).json();
    assert!(lead.as_array().unwrap().is_empty());

    let shown = s.api("GET", "/api/tasks/A", None);
    assert_eq!(shown.status, 200);
    assert_eq!(shown.json()["dependents"], serde_json::json!(["B"]));
}

#[test]
fn create_with_invalid_input_is_400() {
    let s = Server::start();
    let base = serde_json::json!({
        "id": "A", "title": "Alpha", "role": "engine", "do": "x", "verify": "y"
    });
    let mut unknown_role = base.clone();
    unknown_role["role"] = "nope".into();
    let mut missing = base.clone();
    missing.as_object_mut().unwrap().remove("title");
    let mut bad_prereq = base.clone();
    bad_prereq["blocked_by"] = serde_json::json!(["Z"]);
    for body in [unknown_role, missing, bad_prereq] {
        let r = s.api("POST", "/api/tasks", Some(body.clone()));
        assert_eq!(r.status, 400, "{body}: {}", r.body);
        assert!(r.json()["error"].is_string());
    }
    let auth = format!("Bearer {}", s.token);
    let host = s.host();
    let garbled = s.raw(
        "POST",
        "/api/tasks",
        &[
            ("Host", &host),
            ("Authorization", &auth),
            ("Content-Type", "application/json"),
        ],
        "{not json",
    );
    assert_eq!(garbled.status, 400);
    let bad_filter = s.api("GET", "/api/tasks?status=sleeping", None);
    assert_eq!(bad_filter.status, 400);
    assert!(bad_filter.json()["error"].is_string());
}

#[test]
fn unknown_ids_are_404() {
    let s = Server::start();
    assert_eq!(s.api("GET", "/api/tasks/nope", None).status, 404);
    let r = s.api(
        "POST",
        "/api/tasks/nope/approve",
        Some(serde_json::json!({})),
    );
    assert_eq!(r.status, 404);
    let r = s.api(
        "POST",
        "/api/questions/9/answer",
        Some(serde_json::json!({ "answer": "x" })),
    );
    assert_eq!(r.status, 404);
}

#[test]
fn approve_and_reject_flow() {
    let s = Server::start();
    add_task(&s, "A", &[]);
    add_task(&s, "B", &[]);
    submit_for_review(&s, "A");
    submit_for_review(&s, "B");
    let review = s.api("GET", "/api/tasks?status=in_review", None).json();
    assert_eq!(review.as_array().unwrap().len(), 2);

    let approved = s.api(
        "POST",
        "/api/tasks/A/approve",
        Some(serde_json::json!({ "notes": "looks good" })),
    );
    assert_eq!(approved.status, 200, "{}", approved.body);
    assert_eq!(approved.json()["task"]["status"], "done");
    assert_eq!(approved.json()["now_ready"], serde_json::json!([]));

    let rejected = s.api(
        "POST",
        "/api/tasks/B/reject",
        Some(serde_json::json!({ "notes": "missing test" })),
    );
    assert_eq!(rejected.status, 200, "{}", rejected.body);
    assert_eq!(rejected.json()["status"], "ready");
    assert_eq!(rejected.json()["notes"], "missing test");
}

#[test]
fn reject_without_notes_is_400() {
    let s = Server::start();
    add_task(&s, "A", &[]);
    submit_for_review(&s, "A");
    for body in [serde_json::json!({}), serde_json::json!({ "notes": "  " })] {
        let r = s.api("POST", "/api/tasks/A/reject", Some(body));
        assert_eq!(r.status, 400, "{}", r.body);
    }
}

#[test]
fn invalid_transitions_are_409() {
    let s = Server::start();
    add_task(&s, "A", &[]);
    let r = s.api("POST", "/api/tasks/A/approve", Some(serde_json::json!({})));
    assert_eq!(r.status, 409, "{}", r.body);
    let r = s.api("POST", "/api/tasks/A/release", Some(serde_json::json!({})));
    assert_eq!(r.status, 409, "{}", r.body);
}

#[test]
fn force_release_frees_lease() {
    let s = Server::start();
    add_task(&s, "A", &[]);
    s.cli(&["--session", "s1", "task", "claim", "A"]);
    let r = s.api("POST", "/api/tasks/A/release", Some(serde_json::json!({})));
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["status"], "ready");
    assert!(r.json()["claimed_by"].is_null());
}

#[test]
fn answer_and_resolve_question() {
    let s = Server::start();
    s.cli(&[
        "question",
        "ask",
        "How wide is the arc?",
        "--topic",
        "melee.md",
    ]);
    let empty = s.api(
        "POST",
        "/api/questions/1/answer",
        Some(serde_json::json!({ "answer": "" })),
    );
    assert_eq!(empty.status, 400);
    let answered = s.api(
        "POST",
        "/api/questions/1/answer",
        Some(serde_json::json!({ "answer": "90 degrees" })),
    );
    assert_eq!(answered.status, 200, "{}", answered.body);
    assert_eq!(answered.json()["answered_by"], "dashboard");
    let resolved = s.api(
        "POST",
        "/api/questions/1/resolve",
        Some(serde_json::json!({ "spec_file": "docs/spec/melee.md" })),
    );
    assert_eq!(resolved.json()["status"], "resolved");
    assert_eq!(resolved.json()["spec_file"], "docs/spec/melee.md");
    let list = s.api("GET", "/api/questions?status=resolved&topic=melee.md", None);
    assert_eq!(list.json().as_array().unwrap().len(), 1);
    let again = s.api(
        "POST",
        "/api/questions/1/resolve",
        Some(serde_json::json!({})),
    );
    assert_eq!(again.status, 409);
}

#[test]
fn project_and_overlaps_endpoints() {
    let s = Server::start();
    let project = s.api("GET", "/api/project", None).json();
    assert_eq!(project["name"], "demo");
    assert!(
        project["roles"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["name"] == "engine")
    );

    add_task(&s, "A", &["src/sim.rs", "src/a.rs"]);
    add_task(&s, "B", &["./src/sim.rs"]);
    add_task(&s, "C", &[]);
    s.cli(&["--session", "s1", "task", "claim", "A"]);
    let overlaps = s.api("GET", "/api/overlaps", None).json();
    assert_eq!(overlaps["files"][0]["path"], "src/sim.rs");
    assert_eq!(overlaps["files"][0]["tasks"], serde_json::json!(["A", "B"]));
    assert_eq!(
        overlaps["files"][0]["occupied_by"],
        serde_json::json!(["A"])
    );
    assert_eq!(overlaps["no_touch"], serde_json::json!(["C"]));
    let b = &overlaps["tasks"][1];
    assert_eq!(b["id"], "B");
    assert_eq!(b["overlaps"][0]["occupied"], true);
    assert_eq!(b["stage"], 1);
}

#[test]
fn verify_is_never_executed() {
    let s = Server::start();
    let marker = s.project.join("verify-ran");
    let verify = format!("touch {}", marker.display());
    s.cli(&[
        "task", "add", "A", "Title", "--role", "engine", "--do", "x", "--verify", &verify,
    ]);
    for path in [
        "/api/tasks",
        "/api/tasks/A",
        "/api/overlaps",
        "/api/project",
    ] {
        assert_eq!(s.api("GET", path, None).status, 200, "{path}");
    }
    assert!(!marker.exists());
}

#[test]
fn events_report_task_and_question_changes() {
    let s = Server::start();
    add_task(&s, "A", &[]);
    let mut stream = TcpStream::connect(("127.0.0.1", s.port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let request = format!(
        "GET /api/events HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\n\r\n",
        s.host(),
        s.token
    );
    stream.write_all(request.as_bytes()).unwrap();

    let mut seen = String::new();
    let mut read_until = |needle: &str, seen: &mut String| {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut buf = [0u8; 4096];
        while !seen.contains(needle) && std::time::Instant::now() < deadline {
            if let Ok(n) = stream.read(&mut buf) {
                seen.push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        }
        assert!(seen.contains(needle), "no {needle:?} in {seen:?}");
    };
    read_until("text/event-stream", &mut seen);

    s.cli(&["--session", "s1", "task", "claim", "A"]);
    read_until("event: task_changed\ndata: {\"id\":\"A\"}", &mut seen);
    s.cli(&["question", "ask", "Why?", "--topic", "melee.md"]);
    read_until("event: question_changed\ndata: {\"id\":1}", &mut seen);
}
