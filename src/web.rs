//! Local web dashboard and REST/SSE API (Spec 04).
//!
//! The server binds loopback addresses only and guards every request: the
//! `Host` header must name this server (DNS-rebinding defense), and `/api/*`
//! additionally needs the bearer token printed at startup, a same-origin or
//! absent `Origin`, and a JSON body for `POST`. Each request opens its own
//! [`Store`] on a blocking thread, like any CLI invocation. A watcher thread
//! polls `PRAGMA data_version` once per second and turns changes into
//! server-sent events.

use crate::app::{TaskView, done_ids, ensure_role, unix_now};
use crate::core::{QuestionStatus, TaskStatus, dependents};
use crate::hub::Project;
use crate::overlap;
use crate::store::{NewTask, NotFound, Store};
use anyhow::{Context, Result};
use axum::Router;
use axum::body::Bytes;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::io::Write;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

/// Session id recorded for answers and force-releases made in the dashboard.
pub const DASHBOARD_SESSION: &str = "dashboard";

const INDEX_HTML: &str = include_str!("web/index.html");
const APP_JS: &str = include_str!("web/app.js");
const STYLE_CSS: &str = include_str!("web/style.css");

/// Shared server state.
struct AppState {
    project: Project,
    port: u16,
    token: String,
    events: broadcast::Sender<Change>,
}

/// One changed entity, sent as an SSE event named `kind` with data `{ "id": id }`.
#[derive(Debug, Clone)]
struct Change {
    kind: &'static str,
    id: Value,
}

type Shared = Arc<AppState>;

/// Runs the dashboard for `project` until the process is stopped.
pub fn serve(project: Project, port: u16) -> Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(project, port))
}

async fn run(project: Project, port: u16) -> Result<()> {
    let v4 = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .await
        .with_context(|| format!("cannot listen on 127.0.0.1:{port}"))?;
    let port = v4.local_addr()?.port();
    // IPv6 loopback is optional: some hosts have it disabled.
    let v6 = tokio::net::TcpListener::bind((Ipv6Addr::LOCALHOST, port))
        .await
        .ok();
    let name = project.config.name.clone();
    let (events, _) = broadcast::channel(256);
    let state = Arc::new(AppState {
        project,
        port,
        token: new_token()?,
        events,
    });
    let watched = state.clone();
    std::thread::spawn(move || watch(&watched.project, &watched.events));
    let app = router(state.clone());

    println!("Dashboard: http://127.0.0.1:{port}/#token={}", state.token);
    std::io::stdout().flush()?;
    eprintln!("airlock serve: project {name}; press Ctrl-C to stop");

    if let Some(v6) = v6 {
        tokio::spawn(axum::serve(v6, app.clone()).into_future());
    }
    axum::serve(v4, app).await?;
    Ok(())
}

fn router(state: Shared) -> Router {
    Router::new()
        .route(
            "/",
            get(|| async { asset("text/html; charset=utf-8", INDEX_HTML) }),
        )
        .route(
            "/app.js",
            get(|| async { asset("text/javascript; charset=utf-8", APP_JS) }),
        )
        .route(
            "/style.css",
            get(|| async { asset("text/css; charset=utf-8", STYLE_CSS) }),
        )
        .route("/api/project", get(show_project))
        .route("/api/tasks", get(list_tasks).post(create_task))
        .route("/api/tasks/{id}", get(show_task))
        .route("/api/tasks/{id}/release", post(release_task))
        .route("/api/tasks/{id}/approve", post(approve_task))
        .route("/api/tasks/{id}/reject", post(reject_task))
        .route("/api/questions", get(list_questions))
        .route("/api/questions/{id}/answer", post(answer_question))
        .route("/api/questions/{id}/resolve", post(resolve_question))
        .route("/api/overlaps", get(show_overlaps))
        .route("/api/events", get(events))
        .fallback(|| async { ApiError::new(StatusCode::NOT_FOUND, "not found") })
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

/// 256 random bits as lowercase hex.
fn new_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("generating the access token: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn asset(content_type: &'static str, body: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-store"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'self'; img-src 'self' data:; frame-ancestors 'none'",
            ),
        ],
        body,
    )
        .into_response()
}

/// Security checks of Spec 04 §2, applied before every handler.
async fn guard(State(state): State<Shared>, req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let port = state.port;
    let allowed = [
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        format!("[::1]:{port}"),
    ];
    if !allowed.contains(&host) {
        return ApiError::new(StatusCode::FORBIDDEN, "unexpected Host header").into_response();
    }
    if req.uri().path().starts_with("/api/") {
        let origin = req.headers().get(header::ORIGIN);
        if origin.is_some_and(|o| o.as_bytes() != format!("http://{host}").as_bytes()) {
            return ApiError::new(StatusCode::FORBIDDEN, "cross-origin request refused")
                .into_response();
        }
        let token = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            .unwrap_or_default();
        if !same_secret(token.as_bytes(), state.token.as_bytes()) {
            return ApiError::new(StatusCode::UNAUTHORIZED, "missing or invalid access token")
                .into_response();
        }
        let json = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .is_some_and(|t| t.split(';').next() == Some("application/json"));
        if req.method() == Method::POST && !json {
            return ApiError::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "mutating requests must be application/json",
            )
            .into_response();
        }
    }
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

/// Compares secrets in time independent of where they differ.
fn same_secret(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// An error response `{ "error": "<message>" }`.
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    /// Maps a store error: unknown ids are `404`, anything else `status`.
    fn from_store(error: anyhow::Error, status: StatusCode) -> Self {
        let status = if error.downcast_ref::<NotFound>().is_some() {
            StatusCode::NOT_FOUND
        } else {
            status
        };
        Self::new(status, format!("{error:#}"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, axum::Json(json!({ "error": self.message }))).into_response()
    }
}

type ApiResult = Result<axum::Json<Value>, ApiError>;

/// Runs `f` on a fresh store connection on a blocking thread. Store errors
/// become `status` (or `404` for unknown ids).
async fn with_store<F>(state: &Shared, status: StatusCode, f: F) -> ApiResult
where
    F: FnOnce(&mut Store, &Project, i64) -> Result<Value> + Send + 'static,
{
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        let mut store = Store::open(&state.project.database)
            .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;
        f(&mut store, &state.project, unix_now())
            .map(axum::Json)
            .map_err(|e| ApiError::from_store(e, status))
    })
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
}

/// Parses a JSON request body; an empty body counts as `{}`.
fn body<T: DeserializeOwned>(bytes: &Bytes) -> Result<T, ApiError> {
    let bytes: &[u8] = if bytes.is_empty() { b"{}" } else { bytes };
    serde_json::from_slice(bytes).map_err(|e| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("invalid request body: {e}"),
        )
    })
}

fn query<T>(query: Result<Query<T>, QueryRejection>) -> Result<T, ApiError> {
    query
        .map(|Query(q)| q)
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.body_text()))
}

/// A required free-text field: must contain more than whitespace.
fn required(value: Option<String>, field: &str) -> Result<String, ApiError> {
    value
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, format!("`{field}` is required")))
}

fn question_id(raw: &str) -> Result<u64, ApiError> {
    raw.parse()
        .map_err(|_| ApiError::new(StatusCode::NOT_FOUND, format!("no question #{raw}")))
}

async fn show_project(State(state): State<Shared>) -> ApiResult {
    let project = &state.project;
    let roles: Vec<Value> = project
        .config
        .roles
        .iter()
        .map(|(name, role)| {
            json!({ "name": name, "description": role.description, "clean_room": role.clean_room })
        })
        .collect();
    Ok(axum::Json(json!({
        "id": project.id,
        "name": project.config.name,
        "roles": roles,
        "lease_minutes": project.config.lease_minutes,
        "session": DASHBOARD_SESSION,
        "version": crate::VERSION,
    })))
}

#[derive(Deserialize)]
struct TaskFilter {
    role: Option<String>,
    status: Option<TaskStatus>,
}

async fn list_tasks(
    State(state): State<Shared>,
    filter: Result<Query<TaskFilter>, QueryRejection>,
) -> ApiResult {
    let filter = query(filter)?;
    with_store(&state, StatusCode::BAD_REQUEST, move |store, _, now| {
        let tasks = store.tasks(now)?;
        let done = done_ids(&tasks);
        let views: Vec<TaskView> = tasks
            .iter()
            .map(|t| TaskView::new(t, &done))
            .filter(|v| filter.role.as_ref().is_none_or(|r| &v.task.role == r))
            .filter(|v| filter.status.is_none_or(|s| v.status == s))
            .collect();
        Ok(serde_json::to_value(views)?)
    })
    .await
}

/// Body of `POST /api/tasks`, with the parameter names of MCP `task_create`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateTask {
    id: String,
    title: String,
    role: String,
    #[serde(default)]
    priority: i32,
    #[serde(default)]
    blocked_by: Vec<String>,
    #[serde(default)]
    read: Vec<String>,
    #[serde(default)]
    touch: Vec<String>,
    r#do: String,
    verify: String,
}

async fn create_task(State(state): State<Shared>, bytes: Bytes) -> Result<Response, ApiError> {
    let new: CreateTask = body(&bytes)?;
    let task = with_store(
        &state,
        StatusCode::BAD_REQUEST,
        move |store, project, now| {
            ensure_role(project, &new.role)?;
            let task = store.add_task(
                NewTask {
                    id: new.id,
                    title: new.title,
                    role: new.role,
                    priority: new.priority,
                    blocked_by: new.blocked_by,
                    read: new.read,
                    touch: new.touch,
                    r#do: new.r#do,
                    verify: new.verify,
                },
                now,
            )?;
            let tasks = store.tasks(now)?;
            Ok(serde_json::to_value(TaskView::new(
                &task,
                &done_ids(&tasks),
            ))?)
        },
    )
    .await?;
    Ok((StatusCode::CREATED, task).into_response())
}

async fn show_task(State(state): State<Shared>, Path(id): Path<String>) -> ApiResult {
    with_store(&state, StatusCode::BAD_REQUEST, move |store, _, now| {
        let task = store.task(&id, now)?;
        let tasks = store.tasks(now)?;
        let mut view = TaskView::new(&task, &done_ids(&tasks));
        view.dependents = Some(
            dependents(&tasks, &id)
                .into_iter()
                .map(String::from)
                .collect(),
        );
        Ok(serde_json::to_value(view)?)
    })
    .await
}

/// The task with its effective status after a change.
fn task_value(store: &Store, id: &str, now: i64) -> Result<Value> {
    let tasks = store.tasks(now)?;
    let task = store.task(id, now)?;
    Ok(serde_json::to_value(TaskView::new(
        &task,
        &done_ids(&tasks),
    ))?)
}

async fn release_task(
    State(state): State<Shared>,
    Path(id): Path<String>,
    bytes: Bytes,
) -> ApiResult {
    let _: Value = body(&bytes)?;
    with_store(&state, StatusCode::CONFLICT, move |store, _, now| {
        store.release(&id, DASHBOARD_SESSION, true, now)?;
        task_value(store, &id, now)
    })
    .await
}

#[derive(Deserialize)]
struct Notes {
    notes: Option<String>,
}

async fn approve_task(
    State(state): State<Shared>,
    Path(id): Path<String>,
    bytes: Bytes,
) -> ApiResult {
    let Notes { notes } = body(&bytes)?;
    with_store(&state, StatusCode::CONFLICT, move |store, _, now| {
        let completion =
            store.approve(&id, notes.as_deref().filter(|n| !n.trim().is_empty()), now)?;
        Ok(json!({ "task": task_value(store, &id, now)?, "now_ready": completion.now_ready }))
    })
    .await
}

async fn reject_task(
    State(state): State<Shared>,
    Path(id): Path<String>,
    bytes: Bytes,
) -> ApiResult {
    let Notes { notes } = body(&bytes)?;
    let notes = required(notes, "notes")?;
    with_store(&state, StatusCode::CONFLICT, move |store, _, now| {
        store.reject(&id, &notes, now)?;
        task_value(store, &id, now)
    })
    .await
}

#[derive(Deserialize)]
struct QuestionFilter {
    topic: Option<String>,
    status: Option<QuestionStatus>,
}

async fn list_questions(
    State(state): State<Shared>,
    filter: Result<Query<QuestionFilter>, QueryRejection>,
) -> ApiResult {
    let filter = query(filter)?;
    with_store(&state, StatusCode::BAD_REQUEST, move |store, _, _| {
        let questions: Vec<_> = store
            .questions()?
            .into_iter()
            .filter(|q| filter.topic.as_ref().is_none_or(|t| &q.topic == t))
            .filter(|q| filter.status.is_none_or(|s| q.status == s))
            .collect();
        Ok(serde_json::to_value(questions)?)
    })
    .await
}

#[derive(Deserialize)]
struct AnswerBody {
    answer: Option<String>,
    spec_file: Option<String>,
}

async fn answer_question(
    State(state): State<Shared>,
    Path(id): Path<String>,
    bytes: Bytes,
) -> ApiResult {
    let id = question_id(&id)?;
    let AnswerBody { answer, spec_file } = body(&bytes)?;
    let answer = required(answer, "answer")?;
    with_store(&state, StatusCode::CONFLICT, move |store, _, now| {
        let spec_file = spec_file.filter(|s| !s.trim().is_empty());
        let q = store.answer(id, &answer, DASHBOARD_SESSION, spec_file.as_deref(), now)?;
        Ok(serde_json::to_value(q)?)
    })
    .await
}

#[derive(Deserialize)]
struct ResolveBody {
    spec_file: Option<String>,
}

async fn resolve_question(
    State(state): State<Shared>,
    Path(id): Path<String>,
    bytes: Bytes,
) -> ApiResult {
    let id = question_id(&id)?;
    let ResolveBody { spec_file } = body(&bytes)?;
    with_store(&state, StatusCode::CONFLICT, move |store, _, now| {
        let spec_file = spec_file.filter(|s| !s.trim().is_empty());
        Ok(serde_json::to_value(store.resolve(
            id,
            spec_file.as_deref(),
            now,
        )?)?)
    })
    .await
}

async fn show_overlaps(State(state): State<Shared>) -> ApiResult {
    with_store(&state, StatusCode::BAD_REQUEST, move |store, _, now| {
        Ok(serde_json::to_value(overlap::analyze(&store.tasks(now)?))?)
    })
    .await
}

async fn events(
    State(state): State<Shared>,
) -> Sse<impl futures_util::Stream<Item = Result<SseEvent, Infallible>>> {
    let stream = futures_util::stream::unfold(state.events.subscribe(), |mut rx| async move {
        let event = match rx.recv().await {
            Ok(change) => SseEvent::default()
                .event(change.kind)
                .data(json!({ "id": change.id }).to_string()),
            // Missed events: the client reloads everything.
            Err(broadcast::error::RecvError::Lagged(_)) => {
                SseEvent::default().event("resync").data("{}")
            }
            Err(broadcast::error::RecvError::Closed) => return None,
        };
        Some((Ok(event), rx))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Serialized form of every task and question, for change detection.
#[derive(Default)]
struct Digest {
    tasks: BTreeMap<String, String>,
    questions: BTreeMap<u64, String>,
    /// Earliest lease expiry: leases run out without a write to the store
    next_expiry: Option<i64>,
}

impl Digest {
    fn load(store: &Store, now: i64) -> Result<Self> {
        let tasks = store.tasks(now)?;
        Ok(Self {
            next_expiry: tasks.iter().filter_map(|t| t.lease_expires_at).min(),
            tasks: tasks
                .iter()
                .map(|t| Ok((t.id.clone(), serde_json::to_string(t)?)))
                .collect::<Result<_>>()?,
            questions: store
                .questions()?
                .iter()
                .map(|q| Ok((q.id, serde_json::to_string(q)?)))
                .collect::<Result<_>>()?,
        })
    }
}

/// Ids whose entry was added, removed, or changed between `old` and `new`.
fn changed<K: Ord + Clone>(old: &BTreeMap<K, String>, new: &BTreeMap<K, String>) -> Vec<K> {
    let mut ids: Vec<K> = new
        .iter()
        .filter(|(k, v)| old.get(*k) != Some(*v))
        .map(|(k, _)| k.clone())
        .collect();
    ids.extend(old.keys().filter(|k| !new.contains_key(*k)).cloned());
    ids
}

/// Polls the store once per second and broadcasts what changed (Spec 04 §4).
fn watch(project: &Project, events: &broadcast::Sender<Change>) {
    let store = match Store::open(&project.database) {
        Ok(store) => store,
        Err(e) => return eprintln!("airlock serve: live updates disabled: {e:#}"),
    };
    let mut version = store.data_version().ok();
    let mut digest = Digest::load(&store, unix_now()).unwrap_or_default();
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let now = unix_now();
        let current = store.data_version().ok();
        let expired = digest.next_expiry.is_some_and(|t| t <= now);
        if current == version && !expired {
            continue;
        }
        version = current;
        let Ok(next) = Digest::load(&store, now) else {
            continue;
        };
        // Sending fails only while no client listens; nothing to do then.
        for id in changed(&digest.tasks, &next.tasks) {
            let _ = events.send(Change {
                kind: "task_changed",
                id: id.into(),
            });
        }
        for id in changed(&digest.questions, &next.questions) {
            let _ = events.send(Change {
                kind: "question_changed",
                id: id.into(),
            });
        }
        digest = next;
    }
}
