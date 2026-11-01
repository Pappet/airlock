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
        // Font bodies are binary; lossy decoding keeps the headers and magic bytes readable.
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).unwrap();
        let text = String::from_utf8_lossy(&bytes).into_owned();
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

    /// An authorized API request with a JSON body for `POST` and `PATCH`.
    fn api(&self, method: &str, path: &str, body: Option<Value>) -> Response {
        let auth = format!("Bearer {}", self.token);
        let host = self.host();
        let mut headers = vec![("Host", host.as_str()), ("Authorization", auth.as_str())];
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        if method == "POST" || method == "PATCH" {
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
fn serves_embedded_fonts() {
    let s = Server::start();
    for name in ["space-grotesk", "manrope", "jetbrains-mono"] {
        let r = s.raw(
            "GET",
            &format!("/fonts/{name}.woff2"),
            &[("Host", &s.host())],
            "",
        );
        assert_eq!(r.status, 200, "{name}");
        assert!(r.body.starts_with("wOF2"), "{name}: not a woff2 body");
        assert!(
            r.head.to_lowercase().contains("content-type: font/woff2"),
            "{name}: {}",
            r.head
        );
    }
    let css = s.raw("GET", "/style.css", &[("Host", &s.host())], "").body;
    assert!(
        css.contains("url(/fonts/space-grotesk.woff2)"),
        "style.css must use the embedded fonts"
    );
    assert!(!css.contains("googleapis"), "no external font host");
}

#[test]
fn fonts_need_allowed_host() {
    let s = Server::start();
    let r = s.raw(
        "GET",
        "/fonts/manrope.woff2",
        &[("Host", "evil.example")],
        "",
    );
    assert_eq!(r.status, 403);
}

#[test]
fn dashboard_has_hero_band() {
    let s = Server::start();
    let get = |path: &str| s.raw("GET", path, &[("Host", &s.host())], "");
    let html = get("/").body;
    assert!(html.contains("id=\"hero-stats\"") && html.contains("id=\"hero-now\""));
    assert!(
        !html.contains("section-strip"),
        "the section strip is replaced by the hero band"
    );
    let js = get("/app.js").body;
    assert!(js.contains("function heroNow") && js.contains("const HERO_ROWS = 3"));
    let css = get("/style.css").body;
    assert!(
        css.contains(".tab.on::after"),
        "the active tab gets an underline anchor"
    );
}

#[test]
fn dashboard_cards_follow_the_colour_budget() {
    let s = Server::start();
    let get = |path: &str| s.raw("GET", path, &[("Host", &s.host())], "");
    let js = get("/app.js").body;
    assert!(
        js.contains("badge--conflict"),
        "red only for an overlap with a running task"
    );
    assert!(
        !js.contains("with running"),
        "plain overlaps are no longer red"
    );
    let css = get("/style.css").body;
    assert!(css.contains(".badge--conflict"));
    assert!(
        !css.contains("border-left: 3px solid var(--prio"),
        "no priority stripe on cards"
    );
    assert!(
        !css.contains("padding-top: var(--space-2); border-top"),
        "no lines between swimlane rows"
    );
}

#[test]
fn dashboard_tracks_are_line_free_rows() {
    let s = Server::start();
    let get = |path: &str| s.raw("GET", path, &[("Host", &s.host())], "");
    let js = get("/app.js").body;
    assert!(js.contains("track-row__mini") && js.contains("class: \"runrow\""));
    let css = get("/style.css").body;
    assert!(
        css.contains(".track-row:nth-child(even)"),
        "tonal zebra, not lines"
    );
    assert!(!css.contains("border-left: 3px"), "no stripe on track rows");
    assert!(
        !css.contains(".track-row:first-child { border-top"),
        "no border-top on track rows"
    );
}

#[test]
fn dashboard_activity_is_line_free() {
    let s = Server::start();
    let get = |path: &str| s.raw("GET", path, &[("Host", &s.host())], "");
    let js = get("/app.js").body;
    assert!(js.contains("blk__nobody") && js.contains("act-glyph"));
    let css = get("/style.css").body;
    assert!(
        !css.contains("border-top: 1px solid"),
        "no lines anywhere in the stylesheet"
    );
    assert!(
        !css.contains("inset 3px 0 0"),
        "no side stripe on the problems panel"
    );
    assert!(css.contains(".ch--rest"), "channel tags keep their classes");
}

#[test]
fn dashboard_assets_know_tracks() {
    let s = Server::start();
    let get = |path: &str| s.raw("GET", path, &[("Host", &s.host())], "");
    let js = get("/app.js");
    assert_eq!(js.status, 200);
    assert!(js.body.contains("/api/tracks"), "app.js must load tracks");
    assert!(js.body.contains("airlock-group-by-track"));
    let css = get("/style.css");
    assert_eq!(css.status, 200);
    assert!(css.body.contains(".p-critical"));
}

#[test]
fn dashboard_has_tracks_tab() {
    let s = Server::start();
    let js = s.raw("GET", "/app.js", &[("Host", &s.host())], "");
    assert_eq!(js.status, 200);
    assert!(
        js.body.contains("courseWarnings"),
        "app.js must compute course warnings"
    );
    assert!(
        js.body.contains("\"tracks\", \"Tracks\""),
        "Tracks must be a tab"
    );
}

#[test]
fn dashboard_has_activity_tab() {
    let s = Server::start();
    let get = |path: &str| s.raw("GET", path, &[("Host", &s.host())], "");
    let js = get("/app.js");
    assert_eq!(js.status, 200);
    let tracks = js.body.find("[\"tracks\", \"Tracks\"]");
    let activity = js.body.find("[\"activity\", \"Activity\"]");
    assert!(
        tracks.is_some() && activity > tracks,
        "Activity must be a tab after Tracks"
    );
    for needle in [
        "/api/activity?",
        "/api/activity/problems",
        "/api/activity/sessions",
        "groupByDay",
        "sessionChips",
        "blockers",
    ] {
        assert!(js.body.contains(needle), "app.js lacks {needle}");
    }
    let css = get("/style.css");
    assert!(
        css.body.contains(".ch--rest"),
        "style.css lacks channel colours"
    );
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
fn patch_requires_json() {
    let s = Server::start();
    add_task(&s, "E-1", &[]);
    let auth = format!("Bearer {}", s.token);
    let host = s.host();
    let r = s.raw(
        "PATCH",
        "/api/tasks/E-1",
        &[("Host", &host), ("Authorization", &auth)],
        r#"{"title":"x"}"#,
    );
    assert_eq!(r.status, 415);
    assert_eq!(
        s.api("GET", "/api/tasks/E-1", None).json()["title"],
        "Title"
    );
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

/// Adds an engine task and checks it got the expected id.
fn add_task(s: &Server, id: &str, touch: &[&str]) {
    let mut args = vec![
        "--json",
        "task",
        "add",
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
    let out = s.cli(&args);
    let task: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(task["id"], id);
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
            "title": "Alpha", "role": "engine", "do": "Do it",
            "verify": "cargo test", "touch": ["src/a.rs"], "priority": "high"
        })),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    assert_eq!(created.json()["id"], "E-1", "airlock assigns the id");
    assert_eq!(created.json()["status"], "ready");
    s.api(
        "POST",
        "/api/tasks",
        Some(serde_json::json!({
            "title": "Beta", "role": "engine", "do": "x", "verify": "y",
            "blocked_by": ["E-1"]
        })),
    );

    let list = s.api("GET", "/api/tasks", None).json();
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert_eq!(list[0]["priority"], "high");
    assert_eq!(list[1]["status"], "blocked");
    let blocked = s.api("GET", "/api/tasks?status=blocked", None).json();
    assert_eq!(blocked.as_array().unwrap().len(), 1);
    let lead = s.api("GET", "/api/tasks?role=lead", None).json();
    assert!(lead.as_array().unwrap().is_empty());

    let shown = s.api("GET", "/api/tasks/E-1", None);
    assert_eq!(shown.status, 200);
    assert_eq!(shown.json()["dependents"], serde_json::json!(["E-2"]));
}

#[test]
fn create_with_invalid_input_is_400() {
    let s = Server::start();
    let base = serde_json::json!({
        "title": "Alpha", "role": "engine", "do": "x", "verify": "y"
    });
    let mut unknown_role = base.clone();
    unknown_role["role"] = "nope".into();
    let mut missing = base.clone();
    missing.as_object_mut().unwrap().remove("title");
    let mut bad_prereq = base.clone();
    bad_prereq["blocked_by"] = serde_json::json!(["Z"]);
    let mut own_id = base.clone();
    own_id["id"] = "E-7".into();
    let mut numeric_priority = base.clone();
    numeric_priority["priority"] = 3.into();
    for body in [
        unknown_role,
        missing,
        bad_prereq,
        own_id,
        numeric_priority.clone(),
    ] {
        let r = s.api("POST", "/api/tasks", Some(body.clone()));
        assert_eq!(r.status, 400, "{body}: {}", r.body);
        assert!(r.json()["error"].is_string());
    }
    let r = s.api("POST", "/api/tasks", Some(numeric_priority));
    assert!(
        r.json()["error"]
            .as_str()
            .unwrap()
            .contains("critical, high, normal, or low"),
        "{}",
        r.body
    );
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
    add_task(&s, "E-1", &[]);
    add_task(&s, "E-2", &[]);
    submit_for_review(&s, "E-1");
    submit_for_review(&s, "E-2");
    let review = s.api("GET", "/api/tasks?status=in_review", None).json();
    assert_eq!(review.as_array().unwrap().len(), 2);

    let approved = s.api(
        "POST",
        "/api/tasks/E-1/approve",
        Some(serde_json::json!({ "notes": "looks good" })),
    );
    assert_eq!(approved.status, 200, "{}", approved.body);
    assert_eq!(approved.json()["task"]["status"], "done");
    assert_eq!(approved.json()["now_ready"], serde_json::json!([]));

    let rejected = s.api(
        "POST",
        "/api/tasks/E-2/reject",
        Some(serde_json::json!({ "notes": "missing test" })),
    );
    assert_eq!(rejected.status, 200, "{}", rejected.body);
    assert_eq!(rejected.json()["status"], "ready");
    assert_eq!(rejected.json()["notes"], "missing test");
}

#[test]
fn patch_edits_task_content() {
    let s = Server::start();
    add_task(&s, "E-1", &["src/a.rs"]);
    add_task(&s, "E-2", &[]);
    let r = s.api(
        "PATCH",
        "/api/tasks/E-2",
        Some(serde_json::json!({ "title": "Beta", "blocked_by": ["E-1"], "touch": ["src/b.rs"] })),
    );
    assert_eq!(r.status, 200, "{}", r.body);
    let b = r.json();
    assert_eq!(
        (b["title"].as_str(), b["status"].as_str()),
        (Some("Beta"), Some("blocked"))
    );
    assert_eq!(b["touch"], serde_json::json!(["src/b.rs"]));
    assert_eq!(b["do"], "Do it", "omitted fields are kept");

    for body in [
        serde_json::json!({}),
        serde_json::json!({ "role": "nope" }),
        serde_json::json!({ "state": "done" }),
    ] {
        let r = s.api("PATCH", "/api/tasks/E-2", Some(body.clone()));
        assert_eq!(r.status, 400, "{body}: {}", r.body);
    }
    let cycle = s.api(
        "PATCH",
        "/api/tasks/E-1",
        Some(serde_json::json!({ "blocked_by": ["E-2"] })),
    );
    assert_eq!(cycle.status, 409, "{}", cycle.body);
    assert!(
        cycle.json()["error"]
            .as_str()
            .unwrap()
            .contains("dependency cycle")
    );
    let numeric = s.api(
        "PATCH",
        "/api/tasks/E-2",
        Some(serde_json::json!({ "priority": 3 })),
    );
    assert_eq!(numeric.status, 400, "{}", numeric.body);
    assert!(
        numeric.json()["error"]
            .as_str()
            .unwrap()
            .contains("critical, high, normal, or low"),
        "{}",
        numeric.body
    );
    let missing = s.api(
        "PATCH",
        "/api/tasks/nope",
        Some(serde_json::json!({ "title": "x" })),
    );
    assert_eq!(missing.status, 404);

    s.cli(&["--session", "s1", "task", "claim", "E-1"]);
    let started = s.api(
        "PATCH",
        "/api/tasks/E-1",
        Some(serde_json::json!({ "title": "x" })),
    );
    assert_eq!(started.status, 409, "{}", started.body);
    let forced = s.api(
        "PATCH",
        "/api/tasks/E-1",
        Some(serde_json::json!({ "title": "x", "force": true })),
    );
    assert_eq!(forced.status, 200, "{}", forced.body);
    assert_eq!(forced.json()["claimed_by"], "s1");
}

fn ids(r: &Response) -> Vec<String> {
    assert_eq!(r.status, 200, "{}", r.body);
    r.json()
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn tracks_endpoint_and_filters() {
    let s = Server::start();
    s.cli(&["track", "add", "comfort", "--priority", "high"]);
    s.cli(&["track", "add", "core", "--description", "Engine core"]);
    let post = |title: &str, extra: Value| {
        let mut body = serde_json::json!({
            "title": title, "role": "engine", "do": "x", "verify": "y"
        });
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let r = s.api("POST", "/api/tasks", Some(body));
        assert_eq!(r.status, 201, "{}", r.body);
        r.json()
    };
    let a = post(
        "A",
        serde_json::json!({ "track": "comfort", "priority": "high" }),
    );
    assert_eq!(a["track"], "comfort");
    post(
        "B",
        serde_json::json!({ "track": "comfort", "blocked_by": ["E-1"] }),
    );
    post(
        "C",
        serde_json::json!({ "track": "core", "priority": "critical" }),
    );
    post("D", serde_json::json!({}));

    let tracks = s.api("GET", "/api/tracks", None);
    assert_eq!(tracks.status, 200, "{}", tracks.body);
    let tracks = tracks.json();
    let names: Vec<&str> = tracks
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["comfort", "core"], "bare array in store order");
    assert_eq!(tracks[0]["priority"], "high");
    assert_eq!(
        tracks[0]["counts"],
        serde_json::json!({ "blocked": 1, "ready": 1, "in_progress": 0, "in_review": 0, "done": 0 })
    );
    assert_eq!(tracks[1]["description"], "Engine core");
    assert_eq!(tracks[1]["counts"]["ready"], 1);

    let list = |q: &str| ids(&s.api("GET", &format!("/api/tasks{q}"), None));
    assert_eq!(list("?track=comfort"), ["E-1", "E-2"]);
    assert_eq!(list("?track="), ["E-4"], "empty value means no track");
    assert_eq!(list("?priority=high"), ["E-1", "E-3"]);
    assert_eq!(list("?priority=low").len(), 4);
    assert_eq!(list("?track=comfort&priority=high"), ["E-1"]);
    for bad in ["?priority=urgent", "?priority=3"] {
        let r = s.api("GET", &format!("/api/tasks{bad}"), None);
        assert_eq!(r.status, 400, "{bad}: {}", r.body);
        assert!(
            r.json()["error"]
                .as_str()
                .unwrap()
                .contains("critical, high, normal, or low")
        );
    }

    let unknown = s.api(
        "POST",
        "/api/tasks",
        Some(serde_json::json!({
            "title": "E", "role": "engine", "do": "x", "verify": "y", "track": "nope"
        })),
    );
    assert_eq!(unknown.status, 400, "{}", unknown.body);
    assert!(unknown.json()["error"].as_str().unwrap().contains("nope"));
    let unknown = s.api(
        "PATCH",
        "/api/tasks/E-4",
        Some(serde_json::json!({ "track": "nope" })),
    );
    assert_eq!(unknown.status, 409, "{}", unknown.body);

    let set = s.api(
        "PATCH",
        "/api/tasks/E-4",
        Some(serde_json::json!({ "track": "core" })),
    );
    assert_eq!(set.status, 200, "{}", set.body);
    assert_eq!(set.json()["track"], "core");
    let keep = s.api(
        "PATCH",
        "/api/tasks/E-4",
        Some(serde_json::json!({ "title": "D2" })),
    );
    assert_eq!(keep.json()["track"], "core", "absent track is kept");
    let clear = s.api(
        "PATCH",
        "/api/tasks/E-4",
        Some(serde_json::json!({ "track": null })),
    );
    assert_eq!(clear.status, 200, "{}", clear.body);
    assert!(clear.json()["track"].is_null());
    let empty = s.api("PATCH", "/api/tasks/E-4", Some(serde_json::json!({})));
    assert_eq!(empty.status, 400, "{}", empty.body);
    let wrong = s.api(
        "PATCH",
        "/api/tasks/E-4",
        Some(serde_json::json!({ "track": 3 })),
    );
    assert_eq!(wrong.status, 400, "{}", wrong.body);
}

#[test]
fn reject_without_notes_is_400() {
    let s = Server::start();
    add_task(&s, "E-1", &[]);
    submit_for_review(&s, "E-1");
    for body in [serde_json::json!({}), serde_json::json!({ "notes": "  " })] {
        let r = s.api("POST", "/api/tasks/E-1/reject", Some(body));
        assert_eq!(r.status, 400, "{}", r.body);
    }
}

#[test]
fn invalid_transitions_are_409() {
    let s = Server::start();
    add_task(&s, "E-1", &[]);
    let r = s.api(
        "POST",
        "/api/tasks/E-1/approve",
        Some(serde_json::json!({})),
    );
    assert_eq!(r.status, 409, "{}", r.body);
    let r = s.api(
        "POST",
        "/api/tasks/E-1/release",
        Some(serde_json::json!({})),
    );
    assert_eq!(r.status, 409, "{}", r.body);
}

#[test]
fn force_release_frees_lease() {
    let s = Server::start();
    add_task(&s, "E-1", &[]);
    s.cli(&["--session", "s1", "task", "claim", "E-1"]);
    let r = s.api(
        "POST",
        "/api/tasks/E-1/release",
        Some(serde_json::json!({})),
    );
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

    add_task(&s, "E-1", &["src/sim.rs", "src/a.rs"]);
    add_task(&s, "E-2", &["./src/sim.rs"]);
    add_task(&s, "E-3", &[]);
    s.cli(&["--session", "s1", "task", "claim", "E-1"]);
    let overlaps = s.api("GET", "/api/overlaps", None).json();
    assert_eq!(overlaps["files"][0]["path"], "src/sim.rs");
    assert_eq!(
        overlaps["files"][0]["tasks"],
        serde_json::json!(["E-1", "E-2"])
    );
    assert_eq!(
        overlaps["files"][0]["occupied_by"],
        serde_json::json!(["E-1"])
    );
    assert_eq!(overlaps["no_touch"], serde_json::json!(["E-3"]));
    let b = &overlaps["tasks"][1];
    assert_eq!(b["id"], "E-2");
    assert_eq!(b["overlaps"][0]["occupied"], true);
    assert_eq!(b["stage"], 1);

    // A directory entry covers the files beneath it; the file is the row.
    add_task(&s, "E-4", &["src/render/"]);
    add_task(&s, "E-5", &["src/render/mesh.rs"]);
    let overlaps = s.api("GET", "/api/overlaps", None).json();
    let row = overlaps["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == "src/render/mesh.rs")
        .expect("the covered file is listed");
    assert_eq!(row["tasks"], serde_json::json!(["E-4", "E-5"]));
    assert!(
        overlaps["files"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["path"] != "src/render/"),
        "{overlaps}"
    );
}

#[test]
fn verify_is_never_executed() {
    let s = Server::start();
    let marker = s.project.join("verify-ran");
    let verify = format!("touch {}", marker.display());
    s.cli(&[
        "task", "add", "Title", "--role", "engine", "--do", "x", "--verify", &verify,
    ]);
    for path in [
        "/api/tasks",
        "/api/tasks/E-1",
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
    add_task(&s, "E-1", &[]);
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

    s.cli(&["--session", "s1", "task", "claim", "E-1"]);
    read_until("event: task_changed\ndata: {\"id\":\"E-1\"}", &mut seen);
    s.cli(&["question", "ask", "Why?", "--topic", "melee.md"]);
    read_until("event: question_changed\ndata: {\"id\":1}", &mut seen);
    s.cli(&["track", "add", "core"]);
    read_until("event: track_changed\ndata: {\"id\":\"core\"}", &mut seen);
}

/// The project's activity log, found through the hub.
fn activity_log(hub: &Path) -> airlock::activity::ActivityLog {
    let dir = std::fs::read_dir(hub.join("projects"))
        .unwrap()
        .next()
        .expect("a project in the hub")
        .unwrap()
        .path();
    airlock::activity::ActivityLog::open(&dir.join("activity.db")).unwrap()
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

// Sends SIGTERM through `kill`, which Windows lacks.
#[cfg(unix)]
#[test]
fn serve_registers_while_running_and_unregisters_on_shutdown() {
    let mut s = Server::start();
    let running = activity_log(&s.hub).processes(now()).unwrap();
    assert_eq!(running.len(), 1, "{running:?}");
    assert_eq!(running[0].kind, "serve");
    assert_eq!(running[0].pid, s.child.id());
    assert_eq!(running[0].session, None);
    assert_eq!(running[0].build, airlock::activity::build());

    // SIGTERM is a clean shutdown: the row goes away.
    let killed = Command::new("kill")
        .args(["-TERM", &s.child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    s.child.wait().unwrap();
    assert!(activity_log(&s.hub).processes(now()).unwrap().is_empty());
}

/// The REST entries of the activity log, oldest first.
fn rest_entries(hub: &Path) -> Vec<airlock::activity::Entry> {
    let filter = airlock::activity::Filter {
        channel: Some(airlock::activity::Channel::Rest),
        ..Default::default()
    };
    let mut entries = activity_log(hub).entries(&filter).unwrap();
    entries.sort_by_key(|e| e.id);
    entries
}

#[test]
fn rest_writes_are_logged_as_rest_session() {
    let s = Server::start();
    add_task(&s, "E-1", &[]);
    let r = s.api(
        "PATCH",
        "/api/tasks/E-1",
        Some(serde_json::json!({ "priority": "high" })),
    );
    assert_eq!(r.status, 200, "{}", r.body);

    let entries = rest_entries(&s.hub);
    assert_eq!(entries.len(), 1, "{entries:?}");
    let e = &entries[0];
    assert_eq!(e.session, "rest");
    assert_eq!(e.role, None);
    assert!(!e.clean_room);
    assert_eq!(e.agent, None);
    assert_eq!(e.command, "PATCH /api/tasks/{id}");
    assert_eq!(e.target.as_deref(), Some("E-1"));
    assert_eq!(e.changed, ["priority: high"]);
    assert!(e.write && e.ok && e.error.is_none(), "{e:?}");
}

#[test]
fn dashboard_reads_are_not_logged_but_its_writes_are() {
    let s = Server::start();
    add_task(&s, "E-1", &[]);
    s.cli(&["--session", "s1", "task", "claim", "E-1"]);
    let auth = format!("Bearer {}", s.token);
    let host = s.host();
    let headers = [
        ("Host", host.as_str()),
        ("Authorization", auth.as_str()),
        ("Content-Type", "application/json"),
        ("X-Airlock-Client", "dashboard"),
    ];
    assert_eq!(s.raw("GET", "/api/tasks", &headers, "").status, 200);
    assert!(rest_entries(&s.hub).is_empty());

    let r = s.raw("POST", "/api/tasks/E-1/release", &headers, "{}");
    assert_eq!(r.status, 200, "{}", r.body);
    let entries = rest_entries(&s.hub);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].session, "dashboard");
    assert_eq!(entries[0].command, "POST /api/tasks/{id}/release");
    assert_eq!(entries[0].target.as_deref(), Some("E-1"));

    // Without the header a read is logged, as a read.
    assert_eq!(s.api("GET", "/api/tasks", None).status, 200);
    let entries = rest_entries(&s.hub);
    assert_eq!(entries.len(), 2, "{entries:?}");
    assert_eq!(entries[1].command, "GET /api/tasks");
    assert!(!entries[1].write);
}

#[test]
fn question_path_ids_are_logged_as_hash_numbers() {
    let s = Server::start();
    s.cli(&["question", "ask", "Why?", "--topic", "melee.md"]);
    let r = s.api(
        "POST",
        "/api/questions/1/answer",
        Some(serde_json::json!({ "answer": "Because the secret sentence." })),
    );
    assert_eq!(r.status, 200, "{}", r.body);
    let entries = rest_entries(&s.hub);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].command, "POST /api/questions/{id}/answer");
    assert_eq!(entries[0].target.as_deref(), Some("#1"));
    assert_eq!(entries[0].changed, ["answer"]);
}

#[test]
fn rest_failure_is_logged_with_error_message() {
    let s = Server::start();
    add_task(&s, "E-1", &[]);
    let r = s.api(
        "PATCH",
        "/api/tasks/E-1",
        Some(serde_json::json!({ "track": "no-such-track" })),
    );
    assert_eq!(r.status, 409, "{}", r.body);
    let entries = rest_entries(&s.hub);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert!(!entries[0].ok);
    assert_eq!(
        entries[0].error.as_deref(),
        r.json()["error"].as_str(),
        "{entries:?}"
    );
    // The client still receives the unchanged error body.
    assert!(!r.json()["error"].as_str().unwrap().is_empty());
}

#[test]
fn guard_rejections_are_not_logged() {
    let s = Server::start();
    let r = s.raw("GET", "/api/tasks", &[("Host", &s.host())], "");
    assert_eq!(r.status, 401);
    assert!(rest_entries(&s.hub).is_empty());
}

#[test]
fn events_stream_is_never_logged() {
    let s = Server::start();
    let auth = format!("Bearer {}", s.token);
    let mut stream = TcpStream::connect(("127.0.0.1", s.port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "GET /api/events HTTP/1.1\r\nHost: {}\r\nAuthorization: {auth}\r\n\r\n",
        s.host()
    )
    .unwrap();
    let mut seen = String::new();
    let mut buf = [0u8; 4096];
    while !seen.contains("text/event-stream") {
        let n = stream.read(&mut buf).unwrap();
        assert!(n > 0, "closed: {seen:?}");
        seen.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
    // A logged call after the stream opened shows what a logged entry looks like.
    assert_eq!(s.api("GET", "/api/tasks", None).status, 200);
    let entries = rest_entries(&s.hub);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].command, "GET /api/tasks");
}

#[test]
fn correcting_an_answer_over_rest_is_logged_as_a_correction() {
    let s = Server::start();
    s.cli(&["question", "ask", "Why?", "--topic", "melee.md"]);
    let answer = |text: &str| {
        s.api(
            "POST",
            "/api/questions/1/answer",
            Some(serde_json::json!({ "answer": text })),
        )
    };
    assert_eq!(answer("first").status, 200);
    let resolved = s.api(
        "POST",
        "/api/questions/1/resolve",
        Some(serde_json::json!({})),
    );
    assert_eq!(resolved.status, 200, "{}", resolved.body);
    let corrected = answer("second");
    assert_eq!(corrected.status, 200, "{}", corrected.body);
    assert_eq!(corrected.json()["status"], "resolved");
    assert_eq!(corrected.json()["history"][0]["answer"], "first");

    let entries = rest_entries(&s.hub);
    let answers: Vec<_> = entries
        .iter()
        .filter(|e| e.command == "POST /api/questions/{id}/answer")
        .collect();
    assert_eq!(answers.len(), 2, "{entries:?}");
    assert_eq!(answers[0].changed, ["answer"]);
    assert_eq!(answers[1].changed, ["answer", "correction"]);
}

#[test]
fn api_activity_filters_and_problems() {
    let s = Server::start();
    add_task(&s, "E-1", &[]);
    s.cli(&["--session", "s1", "task", "list"]);
    let failed = s
        .command(&["--session", "s1", "task", "claim", "E-99"])
        .output()
        .unwrap();
    assert!(!failed.status.success());
    s.cli(&["--session", "s2", "task", "claim", "E-1"]);
    assert_eq!(s.api("GET", "/api/tasks", None).status, 200);

    let get = |path: &str| {
        let r = s.api("GET", path, None);
        assert_eq!(r.status, 200, "{path}: {}", r.body);
        r.json()
    };
    let commands = |path: &str| -> Vec<String> {
        get(path)
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["command"].as_str().unwrap().to_string())
            .collect()
    };

    let all = get("/api/activity");
    let all = all.as_array().unwrap();
    assert_eq!(all.len(), 5, "{all:?}");
    assert_eq!(all[0]["command"], "GET /api/tasks", "newest first");
    assert_eq!(all[0]["channel"], "rest");
    assert_eq!(all[1]["command"], "task claim");
    assert_eq!(all[1]["session"], "s2");
    assert_eq!(all[4]["command"], "task add");

    assert_eq!(commands("/api/activity?failures=true"), ["task claim"]);
    assert_eq!(
        get("/api/activity?failures=true")[0]["error"],
        "no task `E-99`"
    );
    assert_eq!(commands("/api/activity?session=s1").len(), 2);
    assert_eq!(commands("/api/activity?channel=rest"), ["GET /api/tasks"]);
    assert_eq!(commands("/api/activity?channel=cli").len(), 4);
    assert_eq!(commands("/api/activity?writes=true").len(), 3);
    assert_eq!(commands("/api/activity?limit=1"), ["GET /api/tasks"]);
    assert_eq!(commands("/api/activity?since=1h").len(), 5);
    assert_eq!(commands("/api/activity?since=4102444800").len(), 0);
    assert_eq!(commands("/api/activity?role=engine").len(), 0);
    assert_eq!(
        commands("/api/activity?session=s1&channel=cli&failures=true&limit=5"),
        ["task claim"]
    );

    for bad in [
        "/api/activity?channel=smoke",
        "/api/activity?since=soon",
        "/api/activity?limit=many",
        "/api/activity?writes=maybe",
    ] {
        let r = s.api("GET", bad, None);
        assert_eq!(r.status, 400, "{bad}: {}", r.body);
        assert!(r.json()["error"].is_string());
    }

    let problems = get("/api/activity/problems");
    let problems = problems.as_array().unwrap();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0]["kind"], "failure");
    assert_eq!(problems[0]["session"], "s1");
    assert_eq!(problems[0]["message"], "no task `E-99`");
    assert_eq!(
        get("/api/activity/problems?session=s2"),
        serde_json::json!([])
    );
    assert_eq!(
        get("/api/activity/problems?session=s1")
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let sessions = get("/api/activity/sessions");
    let sessions = sessions.as_array().unwrap();
    let by_name = |name: &str| {
        sessions
            .iter()
            .find(|v| v["session"] == name)
            .unwrap_or_else(|| panic!("{name} in {sessions:?}"))
    };
    assert_eq!(sessions.len(), 4, "{sessions:?}");
    assert_eq!(sessions[0]["session"], "rest", "latest activity first");
    assert_eq!(sessions[1]["session"], "s2");
    assert_eq!(
        by_name("s1")["calls"],
        serde_json::json!({ "mcp": 0, "cli": 2, "rest": 0 })
    );
    assert_eq!(
        by_name("rest")["calls"],
        serde_json::json!({ "mcp": 0, "cli": 0, "rest": 1 })
    );
    assert!(by_name("s1")["role"].is_null());
    assert!(by_name("s1")["last_at"].as_i64().unwrap() > 1_700_000_000);

    // None of the reads above were logged.
    assert_eq!(get("/api/activity").as_array().unwrap().len(), 5);
}

#[test]
fn api_activity_is_empty_without_a_log() {
    let s = Server::start();
    let dir = std::fs::read_dir(s.hub.join("projects"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    // `serve` registered itself, so the file exists; an empty log still reads as [].
    assert!(dir.join("activity.db").exists());
    for path in [
        "/api/activity",
        "/api/activity/problems",
        "/api/activity/sessions",
    ] {
        let r = s.api("GET", path, None);
        assert_eq!(r.status, 200, "{path}: {}", r.body);
        assert_eq!(r.json(), serde_json::json!([]), "{path}");
    }
}

#[test]
fn sse_emits_activity_event() {
    let s = Server::start();
    let mut stream = TcpStream::connect(("127.0.0.1", s.port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    write!(
        stream,
        "GET /api/events HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\n\r\n",
        s.host(),
        s.token
    )
    .unwrap();
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

    // A read does not touch the store, but it is logged: only the log grows.
    s.cli(&["task", "list"]);
    let first = activity_log(&s.hub).latest_id().unwrap();
    assert!(first > 0);
    read_until(
        &format!("event: activity\ndata: {{\"id\":{first}}}"),
        &mut seen,
    );

    s.cli(&["task", "list"]);
    let second = activity_log(&s.hub).latest_id().unwrap();
    assert!(second > first);
    read_until(
        &format!("event: activity\ndata: {{\"id\":{second}}}"),
        &mut seen,
    );
}

#[test]
fn api_activity_sessions_report_the_latest_role() {
    let s = Server::start();
    s.cli(&["role", "set", "engine"]);
    s.cli(&["use", "engine", "--session", "bound"]);
    s.cli(&["task", "list"]);
    s.cli(&["task", "list", "--session", "other"]);
    let sessions = s.api("GET", "/api/activity/sessions", None).json();
    let sessions = sessions.as_array().unwrap();
    assert!(
        sessions.last().unwrap()["role"].is_null(),
        "role set had no binding yet"
    );
    let find = |name: &str| sessions.iter().find(|v| v["session"] == name).unwrap();
    assert_eq!(find("bound")["role"], "engine");
    assert_eq!(find("bound")["calls"]["cli"], 2, "use and task list");
    assert_eq!(find("other")["role"], "engine", "the worktree's binding");
    assert_eq!(
        sessions.last().unwrap()["calls"]["cli"],
        1,
        "least recent session last"
    );
    let engine = s.api("GET", "/api/activity?role=engine", None).json();
    assert_eq!(engine.as_array().unwrap().len(), 3);
}
