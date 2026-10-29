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
