# Web Dashboard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `airlock serve` — loopback-only, token-protected dashboard implementing Spec 04 plus the Stages and File-matrix overlap views.

**Architecture:** Pure overlap/stage analysis in `src/overlap.rs`; HTTP server in `src/web.rs` (axum on a tokio runtime started only by `serve`); every request opens its own `Store` inside `spawn_blocking`; one watcher thread polls `PRAGMA data_version` and feeds a broadcast channel for SSE. Frontend is three embedded files (`src/web/index.html`, `app.js`, `style.css`, via `include_str!`), no build step.

**Tech Stack:** Rust 2024, axum 0.8, tokio, getrandom (token), rusqlite; vanilla JS + inline SVG; SPECTRUM styling.

**Spec:** `docs/superpowers/specs/2026-10-29-dashboard-design.md`, `docs/spec/04_web_dashboard_and_api.md`

## Global Constraints

- MSRV 1.85, edition 2024, `--locked`; `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` green.
- Bind `127.0.0.1` and `::1` only (`::1` optional when the host has no IPv6). No flag to change the interface.
- Token: 256 random bits, hex; URL printed as `http://127.0.0.1:<port>/#token=<token>`.
- Host must be `127.0.0.1:<port>`, `localhost:<port>` or `[::1]:<port>` → else `403` (all routes, static included).
- `/api/*`: `Authorization: Bearer <token>` → else `401`. `Origin` present and not `http://<allowed host>` → `403`. `POST` without `Content-Type: application/json` → `415`. No CORS headers.
- Errors: `{ "error": "<message>" }`; `400` invalid input, `404` unknown id, `409` invalid transition / lost race.
- `verify` is never executed. No external URLs in any asset.
- Dashboard session id: `dashboard`.

## Review Focus

- `EventSource` cannot send `Authorization` → client streams `/api/events` with `fetch` and parses SSE itself (test: events endpoint without token → 401).
- Leases expire without a write, so `data_version` alone misses them → watcher also re-diffs when the earliest lease expiry passes.
- Tasks with empty `touch` must show as "no touch data", never as clean (unit test in Task 1).
- Port already in use → clear error naming the port, exit non-zero.
- Host header with the right host but another port (`127.0.0.1:9999`) → 403 (e2e test in Task 3).

---

### Task 1: Overlap and stage analysis

**Files:** Create `src/overlap.rs`; modify `src/lib.rs`; test inline `#[cfg(test)]` in `src/overlap.rs`.

**Interfaces — Produces:**
- `pub fn normalize_path(p: &str) -> String` — trim, `\`→`/`, strip leading `./` (repeatedly), collapse `//`.
- `pub fn is_occupying(status: TaskStatus) -> bool` — `InProgress | InReview`.
- `pub fn analyze(tasks: &[Task]) -> Analysis` (all tasks in; done ones ignored).
- `#[derive(Serialize)] pub struct Analysis { pub tasks: Vec<TaskAnalysis>, pub files: Vec<FileUse>, pub no_touch: Vec<String> }`
- `pub struct TaskAnalysis { pub id: String, pub status: TaskStatus, pub stage: u32, pub unknown: bool, pub overlaps: Vec<Overlap> }` (open tasks, sorted by id)
- `pub struct Overlap { pub id: String, pub occupied: bool, pub paths: Vec<String> }` (`occupied` = the *other* task is occupying)
- `pub struct FileUse { pub path: String, pub tasks: Vec<String>, pub occupied_by: Vec<String> }` (only paths with ≥2 open tasks; sorted by task count desc, then path)

Stage: open task without open prerequisites → 1; else `1 + max(stage of open prerequisites)`; memoised, cycle-safe (a revisit counts as stage 1).

- [ ] Tests: `normalizes_paths` (`"./src/a.rs"`, `"src//a.rs"`, `"src\\a.rs"` → `"src/a.rs"`; `"src/"` ≠ `"src/a.rs"`), `done_tasks_never_overlap`, `shared_path_is_symmetric_overlap`, `running_task_marks_overlap_occupied` (in_progress and in_review), `empty_touch_is_unknown_not_clean` (in `no_touch`, `unknown: true`, no overlaps), `stages_ignore_done_prerequisites` (A done, B←A ⇒ B stage 1; C←B ⇒ C stage 2), `files_sorted_by_count_then_path`.
- [ ] Run `cargo test overlap` → fail; implement; → pass. Commit `feat: overlap and stage analysis for the dashboard`.

### Task 2: Typed not-found errors in the store

**Files:** Modify `src/store.rs` (`load_task`, `load_question`, `find`).

**Interfaces — Produces:** `#[derive(Debug)] pub struct NotFound(pub String);` implementing `Display` (the existing message, e.g. ``no task `X` ``) and `std::error::Error`; store returns it wrapped in `anyhow::Error`. CLI/MCP text unchanged.

- [ ] Test in `tests/store.rs`: `unknown_ids_are_not_found` — `store.task("nope")`, `store.release("nope",..)`, `store.question(99)` errors `downcast_ref::<NotFound>().is_some()`; a claim of a blocked task is **not** `NotFound`.
- [ ] Implement, `cargo test` green, commit `feat: typed not-found errors in the store`.

### Task 3: `airlock serve` skeleton and security

**Files:** Create `src/web.rs`, `src/web/index.html` (placeholder), `src/web/app.js`, `src/web/style.css`; modify `Cargo.toml` (axum, tokio `rt-multi-thread,net,macros,time,sync`, getrandom), `src/lib.rs`, `src/app.rs` (dispatch `Command::Serve`, make `ensure_role`, `done_ids`, `unix_now`, `TaskView` usable from `web`), `src/cli.rs` (`--port` doc: `0` picks a free port); test `tests/web.rs`.

**Interfaces — Produces:** `pub fn serve(project: Project, port: u16) -> anyhow::Result<()>` — binds, prints `Dashboard: http://127.0.0.1:<real port>/#token=<hex>` to stdout (flushed), blocks. `struct State { project: Project, port: u16, token: String, events: broadcast::Sender<Event> }`. Helper `async fn with_store<T>(state, f: impl FnOnce(&mut Store, &Project, i64) -> Result<T>) -> Result<T, ApiError>` (spawn_blocking). `ApiError { status, message }` → JSON body; `NotFound` → 404.

- [ ] `tests/web.rs` fixture: init a project in a temp `AIRLOCK_HOME`, spawn `airlock serve --port 0`, read the URL line, raw `TcpStream` HTTP/1.1 helper `req(method, path, headers, body) -> (u16, String)`; kill child on drop.
- [ ] Tests: `serves_dashboard_with_allowed_host` (GET `/` → 200, html), `rejects_foreign_host` (`Host: evil.example` → 403; `Host: 127.0.0.1:9999` → 403), `api_requires_token` (no / wrong token → 401), `rejects_foreign_origin` (`Origin: http://evil.example` → 403), `post_requires_json` (`text/plain` → 415), `no_cors_headers`, `port_in_use_fails` (second serve on same port exits non-zero, stderr mentions port).
- [ ] Implement; tests green; commit `feat: airlock serve with loopback, token, host and origin checks`.

### Task 4: REST endpoints

**Files:** Modify `src/web.rs`; test `tests/web.rs`.

Endpoints (bodies use MCP names): `GET /api/project` → `{ id, name, roles: [..] }`; `GET /api/tasks?role=&status=` → `[TaskView]`; `POST /api/tasks` (`id,title,role,priority?,blocked_by?,read?,touch?,do,verify`; unknown role/invalid id/missing prereq/duplicate → 400); `GET /api/tasks/{id}` (with `dependents`); `POST /api/tasks/{id}/release` (force, session `dashboard`); `/approve` (`notes?`); `/reject` (`notes` non-empty, else 400); `GET /api/questions?topic=&status=`; `POST /api/questions/{id}/answer` (`answer` non-empty, `spec_file?`); `/resolve` (`spec_file?`); `GET /api/overlaps` → `Analysis`. Transition failures → 409.

- [ ] Tests: `create_list_show_task`, `create_with_unknown_role_is_400`, `unknown_task_is_404`, `approve_and_reject_flow` (seed via CLI `task claim`/`done --review`), `reject_without_notes_is_400`, `approve_open_task_is_409`, `force_release_frees_lease`, `answer_and_resolve_question`, `overlaps_endpoint_reports_shared_paths`, `verify_is_never_executed` (task whose verify writes a marker file; hit every GET; file absent).
- [ ] Implement; green; commit `feat: dashboard REST API`.

### Task 5: Live events

**Files:** Modify `src/web.rs`; test `tests/web.rs`.

Watcher thread: own `Store` connection; every 1 s read `PRAGMA data_version` (add `pub fn data_version(&self) -> Result<i64>` to `Store`); on change, or when `now` passes the earliest `lease_expires_at` seen, diff `(id → (state, claimed_by, lease_expires_at, updated_at))` for tasks and `(id → (status, answered_at))` for questions; send `task_changed` / `question_changed` with `{ "id": .. }`. `GET /api/events` → SSE (`text/event-stream`, keep-alive comments).

- [ ] Test `events_report_task_changes`: open `/api/events` with token, run CLI `task claim`, read stream until `event: task_changed` with the id (timeout 10 s).
- [ ] Implement; green; commit `feat: server-sent events from data_version polling`.

### Task 6: Frontend

**Files:** `src/web/index.html`, `src/web/app.js`, `src/web/style.css`.

Token from `location.hash` → `sessionStorage`, hash cleared. Tabs: Board (Kanban, 5 columns, role filter, overlap badge ⚠n, "no touch data" badge), Stages (columns by `stage`, SVG arrows for `blocked_by`, click selects: overlapping cards red ⚠ + shared paths line, Esc clears), Files (matrix from `Analysis.files`, occupied cells green ▶, row/column hover highlight, `no_touch` list below), Review (in_review tasks, notes, Approve / Reject with required feedback), Questions (status filter, answer/revise, resolve with optional spec file), Leases (in_progress with session, minutes left ticking client-side, Force release). Shared detail drawer: read, touch, do, verify (as text, labelled "never executed"), notes, dependencies. SSE via `fetch` stream → refetch. SPECTRUM tokens; every colour paired with ⚠/▶/text; empty states per view; `textContent` only (no `innerHTML` with data).

- [ ] Manual check with OpenSHR hub: `airlock -C ~/Projects/openshr serve`, open URL, every tab renders, select E-6d in Stages shows 4 overlaps via sim.rs/behaviour.rs, Files shows sim.rs and drive.rs with 5 tasks.
- [ ] Commit `feat: embedded dashboard UI`.

### Task 7: Docs

**Files:** `docs/spec/04_web_dashboard_and_api.md` (§3.5 Stages, §3.6 File matrix, overlap rules, `/api/project`, `/api/overlaps`, 401/415, fetch-based SSE, session `dashboard`), `docs/spec/02_cli_interface.md` (`--port 0`), `docs/plans/PHASE_PLAN.md` (Phase 4 ticks, open item: overlap warning in `task next` / `claim`), `docs/walkthroughs/quickstart.md` (open the dashboard), `.gitignore` (`.superpowers/`).

- [ ] Final: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` green; commit `docs: dashboard spec, plan and quickstart`.
