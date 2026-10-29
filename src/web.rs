//! Local web dashboard and REST/SSE API (Spec 04).
//!
//! The server binds loopback addresses only and guards every request: the
//! `Host` header must name this server (DNS-rebinding defense), and `/api/*`
//! additionally needs the bearer token printed at startup, a same-origin or
//! absent `Origin`, and a JSON body for `POST`. Each request opens its own
//! [`Store`] on a blocking thread, like any CLI invocation.

use crate::app::{TaskView, done_ids, unix_now};
use crate::core::TaskStatus;
use crate::hub::Project;
use crate::store::{NotFound, Store};
use anyhow::{Context, Result};
use axum::Router;
use axum::extract::{Query, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::Write;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

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
    let state = Arc::new(AppState {
        project,
        port,
        token: new_token()?,
    });
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
        .route("/api/tasks", get(list_tasks))
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

#[derive(Deserialize)]
struct TaskFilter {
    role: Option<String>,
    status: Option<TaskStatus>,
}

async fn list_tasks(State(state): State<Shared>, Query(filter): Query<TaskFilter>) -> ApiResult {
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
