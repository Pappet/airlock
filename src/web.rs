//! Local web dashboard and REST/SSE API (Spec 04).
//!
//! The server binds loopback addresses only and guards every request: the
//! `Host` header must name this server (DNS-rebinding defense), and `/api/*`
//! additionally needs the bearer token printed at startup, a same-origin or
//! absent `Origin`, and a JSON body for every method but `GET` and `HEAD`.
//! Each request opens its own [`Store`] on a blocking thread, like any CLI
//! invocation. A watcher thread polls `PRAGMA data_version` once per second
//! and turns changes into server-sent events.

use crate::activity::{self, ActivityLog, Channel, Entry};
use crate::activity::{Filter as ActivityFilter, Since};
use crate::app::{
    TaskView, TrackView, current_problems, done_ids, ensure_role, existing_activity_log, unix_now,
};
use crate::core::{Priority, Question, QuestionStatus, TaskStatus, dependents};
use crate::hub::Project;
use crate::overlap;
use crate::store::{NewTask, NotFound, Store, TaskEdit};
use anyhow::{Context, Result};
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::rejection::{QueryRejection, RawPathParamsRejection};
use axum::extract::{MatchedPath, Path, Query, RawPathParams, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::io::Write;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

/// Session id recorded for answers and force-releases made in the dashboard.
pub const DASHBOARD_SESSION: &str = "dashboard";

/// Request header with which the dashboard identifies its calls.
const CLIENT_HEADER: &str = "x-airlock-client";

/// Longest request body the activity log reads for its keys.
const LOGGED_BODY_MAX: usize = 1024 * 1024;

const INDEX_HTML: &str = include_str!("web/index.html");
const APP_JS: &str = include_str!("web/app.js");
const STYLE_CSS: &str = include_str!("web/style.css");
// Latin subsets, variable weight, SIL OFL 1.1 (licence texts next to the files).
const FONT_SPACE_GROTESK: &[u8] = include_bytes!("web/fonts/space-grotesk.woff2");
const FONT_MANROPE: &[u8] = include_bytes!("web/fonts/manrope.woff2");
const FONT_JETBRAINS_MONO: &[u8] = include_bytes!("web/fonts/jetbrains-mono.woff2");
const CSP: &str = "default-src 'self'; img-src 'self' data:; frame-ancestors 'none'";

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
    // Take the baselines before the URL is printed, so a change made right
    // after startup is reported instead of becoming the baseline.
    let baseline = Baseline::take(&state.project);
    let watched = state.clone();
    std::thread::spawn(move || watch(baseline, &watched.events));
    let app = router(state.clone());
    let registration = register(&state.project);

    println!("Dashboard: http://127.0.0.1:{port}/#token={}", state.token);
    std::io::stdout().flush()?;
    eprintln!("airlock serve: project {name}; press Ctrl-C to stop");

    if let Some(v6) = v6 {
        tokio::spawn(axum::serve(v6, app.clone()).into_future());
    }
    let served = tokio::select! {
        served = axum::serve(v4, app) => served.map_err(anyhow::Error::from),
        () = shutdown_signal() => Ok(()),
    };
    if let Some(registration) = registration {
        registration.end();
    }
    served
}

/// Resolves on Ctrl-C or, on Unix, SIGTERM.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let Ok(mut terminate) = signal(SignalKind::terminate()) else {
            let _ = tokio::signal::ctrl_c().await;
            return;
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}

/// This dashboard's row in the process registry and the task keeping it fresh.
struct Registration {
    log: ActivityLog,
    id: u64,
    heartbeat: tokio::task::JoinHandle<()>,
}

impl Registration {
    /// Stops the heartbeat and removes the registry row.
    fn end(mut self) {
        self.heartbeat.abort();
        if let Err(e) = self.log.unregister(self.id) {
            eprintln!("airlock: activity log: {e:#}");
        }
    }
}

/// Lists this dashboard in the project's process registry and starts its
/// heartbeat. A failure is reported and the dashboard runs unregistered.
fn register(project: &Project) -> Option<Registration> {
    let path = ActivityLog::path_for(&project.database);
    let registered = ActivityLog::open(&path).and_then(|mut log| {
        let process = log.register("serve", None, None, unix_now())?;
        Ok((log, process))
    });
    let (log, process) = match registered {
        Ok(registered) => registered,
        Err(e) => {
            eprintln!("airlock: activity log: {e:#}");
            return None;
        }
    };
    let id = process.id;
    let heartbeat = tokio::spawn(async move {
        // Its own connection: the one in `log` belongs to the shutdown path.
        let mut log = match ActivityLog::open(&path) {
            Ok(log) => log,
            Err(e) => {
                eprintln!("airlock: activity log: heartbeat: {e:#}");
                return;
            }
        };
        let period = Duration::from_secs(activity::TOUCH_SECS);
        let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        loop {
            tick.tick().await;
            let _ = log.touch(&process, unix_now());
        }
    });
    Some(Registration { log, id, heartbeat })
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
        .route(
            "/fonts/space-grotesk.woff2",
            get(|| async { font(FONT_SPACE_GROTESK) }),
        )
        .route("/fonts/manrope.woff2", get(|| async { font(FONT_MANROPE) }))
        .route(
            "/fonts/jetbrains-mono.woff2",
            get(|| async { font(FONT_JETBRAINS_MONO) }),
        )
        .route("/api/project", get(show_project))
        .route("/api/tasks", get(list_tasks).post(create_task))
        .route("/api/tasks/{id}", get(show_task).patch(edit_task))
        .route("/api/tasks/{id}/release", post(release_task))
        .route("/api/tasks/{id}/approve", post(approve_task))
        .route("/api/tasks/{id}/reject", post(reject_task))
        .route("/api/tracks", get(list_tracks))
        .route("/api/questions", get(list_questions))
        .route("/api/questions/{id}/answer", post(answer_question))
        .route("/api/questions/{id}/resolve", post(resolve_question))
        .route("/api/overlaps", get(show_overlaps))
        .route("/api/activity", get(list_activity))
        .route("/api/activity/problems", get(list_problems))
        .route("/api/activity/sessions", get(list_sessions))
        .route("/api/events", get(events))
        .fallback(|| async { ApiError::new(StatusCode::NOT_FOUND, "not found") })
        // Inside `guard`: requests it rejects stay unlogged.
        .layer(middleware::from_fn_with_state(state.clone(), log_call))
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
            (header::CONTENT_SECURITY_POLICY, CSP),
        ],
        body,
    )
        .into_response()
}

/// An embedded font; cacheable for a day, as its bytes only change with the build.
fn font(body: &'static [u8]) -> Response {
    (
        [
            (header::CONTENT_TYPE, "font/woff2"),
            (header::CACHE_CONTROL, "max-age=86400"),
            (header::CONTENT_SECURITY_POLICY, CSP),
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
        if !matches!(*req.method(), Method::GET | Method::HEAD) && !json {
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

/// Records every accepted API call in the activity log, except the event
/// stream, the activity endpoints and the dashboard's own reads. The request
/// body is read for its keys and handed on unchanged; a failed response is
/// read for its error message and rebuilt unchanged.
async fn log_call(
    State(state): State<Shared>,
    params: Result<RawPathParams, RawPathParamsRejection>,
    req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path();
    let dashboard = req
        .headers()
        .get(CLIENT_HEADER)
        .is_some_and(|v| v.as_bytes() == DASHBOARD_SESSION.as_bytes());
    let reads = matches!(*req.method(), Method::GET | Method::HEAD);
    let logged = path.starts_with("/api/")
        && !path.starts_with("/api/events")
        && !path.starts_with("/api/activity")
        && !(dashboard && reads);
    // Unrouted paths (the 404 fallback) have no route pattern and stay unlogged.
    let matched = req.extensions().get::<MatchedPath>().cloned();
    let Some(matched) = matched.filter(|_| logged) else {
        return next.run(req).await;
    };
    let method = if reads { "GET" } else { req.method().as_str() };
    let command = format!("{method} {}", matched.as_str());
    let path_id = params.ok().and_then(|params| path_id(&matched, &params));

    let (parts, request_body) = req.into_parts();
    let bytes = match axum::body::to_bytes(request_body, LOGGED_BODY_MAX).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "request body too large")
                .into_response();
        }
    };
    let (mut target, changed) = match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(args)) => activity::summarize_json(&command, &args),
        _ => (None, Vec::new()),
    };
    target = target.or(path_id);
    let req = Request::from_parts(parts, Body::from(bytes));

    let response = next.run(req).await;
    let status = response.status();
    let mut correction = false;
    let (response, error) = if status.is_success() {
        if command == "POST /api/questions/{id}/answer" {
            // Read from the outcome: the returned question says whether this call corrected it.
            let (parts, response_body) = response.into_parts();
            let bytes = axum::body::to_bytes(response_body, usize::MAX)
                .await
                .unwrap_or_default();
            correction =
                serde_json::from_slice::<Question>(&bytes).is_ok_and(|q| activity::corrects(&q));
            (Response::from_parts(parts, Body::from(bytes)), None)
        } else {
            (response, None)
        }
    } else {
        let (parts, response_body) = response.into_parts();
        let bytes = axum::body::to_bytes(response_body, usize::MAX)
            .await
            .unwrap_or_default();
        let message = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|v| v.get("error")?.as_str().map(str::to_string))
            .unwrap_or_else(|| status.to_string());
        (
            Response::from_parts(parts, Body::from(bytes)),
            Some(message),
        )
    };

    let mut changed = changed;
    if correction && error.is_none() {
        changed.push("correction".to_string());
    }
    let entry = Entry {
        id: 0,
        at: unix_now(),
        session: if dashboard { DASHBOARD_SESSION } else { "rest" }.to_string(),
        role: None,
        clean_room: false,
        channel: Channel::Rest,
        agent: None,
        write: activity::is_write(&command),
        command,
        target,
        changed,
        ok: error.is_none(),
        error: error.as_deref().map(activity::truncate_error),
        pid: std::process::id(),
        build: activity::build().to_string(),
    };
    let database = state.project.database.clone();
    // Blocking SQLite work; a failed write only reaches stderr.
    let _ = tokio::task::spawn_blocking(move || activity::record_quietly(&database, &entry)).await;
    response
}

/// The `id` of the route's path parameters, `#<n>` for a question.
fn path_id(matched: &MatchedPath, params: &RawPathParams) -> Option<String> {
    let (_, id) = params.iter().find(|(name, _)| *name == "id")?;
    let question = matched.as_str().starts_with("/api/questions/");
    Some(if question {
        format!("#{id}")
    } else {
        id.to_string()
    })
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
            .and_then(|mut store| store.act_as(DASHBOARD_SESSION).map(|()| store))
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

/// Query of `GET /api/tasks`. `track=` with an empty value selects tasks
/// without a track; `priority` selects tasks at that level or above.
#[derive(Deserialize)]
struct TaskFilter {
    role: Option<String>,
    status: Option<TaskStatus>,
    track: Option<String>,
    priority: Option<String>,
}

/// A priority level from JSON. Goes through `FromStr` so that a number or any
/// other wrong value is refused with the message that lists the four levels.
fn level<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Priority>, D::Error> {
    match Value::deserialize(deserializer)? {
        Value::Null => Ok(None),
        Value::String(s) => s.parse().map(Some).map_err(D::Error::custom),
        other => other
            .to_string()
            .parse()
            .map(Some)
            .map_err(D::Error::custom),
    }
}

/// `Option<Option<String>>` from JSON: absent keeps (via `default`), `null`
/// clears, a string sets.
fn clearable<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(deserializer).map(Some)
}

async fn list_tasks(
    State(state): State<Shared>,
    filter: Result<Query<TaskFilter>, QueryRejection>,
) -> ApiResult {
    let filter = query(filter)?;
    let at_least = filter
        .priority
        .as_deref()
        .map(str::parse::<Priority>)
        .transpose()
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, format!("{e:#}")))?;
    with_store(&state, StatusCode::BAD_REQUEST, move |store, _, now| {
        let tasks = store.tasks(now)?;
        let done = done_ids(&tasks);
        let views: Vec<TaskView> = tasks
            .iter()
            .map(|t| TaskView::new(t, &done))
            .filter(|v| filter.role.as_ref().is_none_or(|r| &v.task.role == r))
            .filter(|v| filter.status.is_none_or(|s| v.status == s))
            .filter(|v| match filter.track.as_deref() {
                None => true,
                Some("") => v.task.track.is_none(),
                Some(name) => v.task.track.as_deref() == Some(name),
            })
            .filter(|v| at_least.is_none_or(|p| v.task.priority >= p))
            .collect();
        Ok(serde_json::to_value(views)?)
    })
    .await
}

/// Body of `POST /api/tasks`, with the parameter names of MCP `task_create`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateTask {
    title: String,
    role: String,
    #[serde(default, deserialize_with = "level")]
    priority: Option<Priority>,
    #[serde(default)]
    track: Option<String>,
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
            let task = store.add_task(
                NewTask {
                    prefix: project.config.task_prefix(&new.role)?,
                    title: new.title,
                    role: new.role,
                    priority: new.priority.unwrap_or_default(),
                    track: new.track,
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

async fn list_tracks(State(state): State<Shared>) -> ApiResult {
    with_store(&state, StatusCode::BAD_REQUEST, move |store, _, now| {
        let views = TrackView::list(&store.tracks()?, &store.tasks(now)?);
        Ok(serde_json::to_value(views)?)
    })
    .await
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

/// Body of `PATCH /api/tasks/{id}`, with the parameter names of MCP `task_update`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditTask {
    title: Option<String>,
    role: Option<String>,
    #[serde(default, deserialize_with = "level")]
    priority: Option<Priority>,
    #[serde(default, deserialize_with = "clearable")]
    track: Option<Option<String>>,
    blocked_by: Option<Vec<String>>,
    read: Option<Vec<String>>,
    touch: Option<Vec<String>>,
    r#do: Option<String>,
    verify: Option<String>,
    #[serde(default)]
    force: bool,
}

async fn edit_task(State(state): State<Shared>, Path(id): Path<String>, bytes: Bytes) -> ApiResult {
    let body: EditTask = body(&bytes)?;
    let edit = TaskEdit {
        title: body.title,
        role: body.role,
        priority: body.priority,
        track: body.track,
        blocked_by: body.blocked_by,
        read: body.read,
        touch: body.touch,
        r#do: body.r#do,
        verify: body.verify,
    };
    if edit.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("nothing to change for task `{id}`"),
        ));
    }
    if let Some(role) = &edit.role {
        ensure_role(&state.project, role)
            .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, format!("{e:#}")))?;
    }
    // Remaining refusals conflict with stored state: a started task, an
    // unknown prerequisite, or a cycle.
    with_store(&state, StatusCode::CONFLICT, move |store, _, now| {
        store.edit_task(&id, edit, body.force, now)?;
        task_value(store, &id, now)
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

/// Longest `GET /api/activity` list unless `limit` says otherwise.
const ACTIVITY_LIMIT: usize = 500;

/// Query of `GET /api/activity`; `since` takes `30m`, `2h`, `1d` or unix seconds.
#[derive(Deserialize)]
struct ActivityQuery {
    session: Option<String>,
    role: Option<String>,
    channel: Option<Channel>,
    since: Option<String>,
    writes: Option<bool>,
    failures: Option<bool>,
    limit: Option<usize>,
}

/// Runs `f` on the project's activity log on a blocking thread. A log that
/// does not exist yet reads as empty (`[]`).
async fn with_activity<F>(state: &Shared, f: F) -> ApiResult
where
    F: FnOnce(&mut ActivityLog, i64) -> Result<Value> + Send + 'static,
{
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        let internal =
            |e: anyhow::Error| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"));
        match existing_activity_log(&state.project.database).map_err(internal)? {
            Some(mut log) => f(&mut log, unix_now()).map(axum::Json).map_err(internal),
            None => Ok(axum::Json(json!([]))),
        }
    })
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
}

async fn list_activity(
    State(state): State<Shared>,
    filter: Result<Query<ActivityQuery>, QueryRejection>,
) -> ApiResult {
    let q = query(filter)?;
    let since = q
        .since
        .as_deref()
        .map(str::parse::<Since>)
        .transpose()
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e))?;
    with_activity(&state, move |log, now| {
        let entries = log.entries(&ActivityFilter {
            session: q.session,
            role: q.role,
            channel: q.channel,
            since: since.map(|s| s.resolve(now)),
            writes_only: q.writes.unwrap_or(false),
            failures_only: q.failures.unwrap_or(false),
            limit: Some(q.limit.unwrap_or(ACTIVITY_LIMIT)),
        })?;
        Ok(serde_json::to_value(entries)?)
    })
    .await
}

/// Query of `GET /api/activity/problems`.
#[derive(Deserialize)]
struct ProblemQuery {
    session: Option<String>,
    role: Option<String>,
}

async fn list_problems(
    State(state): State<Shared>,
    filter: Result<Query<ProblemQuery>, QueryRejection>,
) -> ApiResult {
    let q = query(filter)?;
    with_activity(&state, move |log, now| {
        let found = current_problems(log, now, q.session.as_deref(), q.role.as_deref())?;
        Ok(serde_json::to_value(found)?)
    })
    .await
}

/// One row of `GET /api/activity/sessions`.
#[derive(Serialize)]
struct SessionSummary {
    session: String,
    /// Role of the session's latest call
    role: Option<String>,
    last_at: i64,
    calls: Calls,
}

/// Calls of a session per channel.
#[derive(Default, Serialize)]
struct Calls {
    mcp: u32,
    cli: u32,
    rest: u32,
}

/// Sessions seen in the retention window, most recently active first.
async fn list_sessions(State(state): State<Shared>) -> ApiResult {
    with_activity(&state, move |log, now| {
        // Newest first, so a session's first entry is its latest.
        let entries = log.entries(&ActivityFilter {
            since: Some(now - activity::RETENTION_SECS),
            ..ActivityFilter::default()
        })?;
        let mut sessions: Vec<SessionSummary> = Vec::new();
        let mut index: HashMap<&str, usize> = HashMap::new();
        for e in &entries {
            let at = *index.entry(e.session.as_str()).or_insert_with(|| {
                sessions.push(SessionSummary {
                    session: e.session.clone(),
                    role: e.role.clone(),
                    last_at: e.at,
                    calls: Calls::default(),
                });
                sessions.len() - 1
            });
            let calls = &mut sessions[at].calls;
            match e.channel {
                Channel::Mcp => calls.mcp += 1,
                Channel::Cli => calls.cli += 1,
                Channel::Rest => calls.rest += 1,
            }
        }
        Ok(serde_json::to_value(sessions)?)
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
    tracks: BTreeMap<String, String>,
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
            tracks: store
                .tracks()?
                .iter()
                .map(|t| Ok((t.name.clone(), serde_json::to_string(t)?)))
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

/// Polls the store and the activity log once per second and broadcasts what
/// changed (Spec 04 §4): entity changes as `*_changed`, a growing log as
/// `activity` with the newest entry's id.
/// What the watcher compares against: the store's state and the newest log id
/// at startup.
struct Baseline {
    store: Option<(Store, Option<i64>, Digest)>,
    log: ActivityWatch,
}

impl Baseline {
    fn take(project: &Project) -> Self {
        let store = match Store::open(&project.database) {
            Ok(store) => {
                let version = store.data_version().ok();
                let digest = Digest::load(&store, unix_now()).unwrap_or_default();
                Some((store, version, digest))
            }
            Err(e) => {
                eprintln!("airlock serve: live updates disabled: {e:#}");
                None
            }
        };
        Self {
            store,
            log: ActivityWatch::new(project),
        }
    }
}

fn watch(baseline: Baseline, events: &broadcast::Sender<Change>) {
    let Baseline { store, mut log } = baseline;
    let Some((store, mut version, mut digest)) = store else {
        return;
    };
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let now = unix_now();
        log.poll(events);
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
        for name in changed(&digest.tracks, &next.tracks) {
            let _ = events.send(Change {
                kind: "track_changed",
                id: name.into(),
            });
        }
        digest = next;
    }
}

/// Follows the newest id of the activity log. A log that cannot be opened or
/// read is skipped until the next poll; it never stops the watcher.
struct ActivityWatch {
    path: std::path::PathBuf,
    log: Option<ActivityLog>,
    last_id: u64,
}

impl ActivityWatch {
    fn new(project: &Project) -> Self {
        let mut watch = Self {
            path: ActivityLog::path_for(&project.database),
            log: None,
            last_id: 0,
        };
        watch.last_id = watch.latest().unwrap_or(0);
        watch
    }

    /// The newest entry id, opening the log when needed.
    fn latest(&mut self) -> Option<u64> {
        if self.log.is_none() {
            self.log = ActivityLog::open(&self.path).ok();
        }
        let latest = self.log.as_ref()?.latest_id();
        if latest.is_err() {
            // Reopen next time.
            self.log = None;
        }
        latest.ok()
    }

    fn poll(&mut self, events: &broadcast::Sender<Change>) {
        let Some(latest) = self.latest() else { return };
        if latest > self.last_id {
            let _ = events.send(Change {
                kind: "activity",
                id: latest.into(),
            });
        }
        self.last_id = latest;
    }
}
