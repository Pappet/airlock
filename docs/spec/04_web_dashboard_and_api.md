# Spec 04: Web Dashboard & REST/SSE API

## 1. Overview

`airlock serve [--port <port>]` serves a dashboard and a JSON REST API for one project on `127.0.0.1:3000` (port configurable; `0` picks a free port). All HTML, CSS, and JavaScript are embedded in the binary; the dashboard loads nothing from external hosts. The server operates on the hub store like any CLI invocation (Spec 01 §3.2): each request opens its own store connection. It is a convenience UI, not a required daemon.

The server resolves its project like every other command (`-C`, `--project`, marker discovery; Spec 02), prints the dashboard URL on stdout, and does not open a browser itself. A port that is already in use is an error naming the port.

---

## 2. Security

The API can mutate project state and exposes every question and answer, so it must not be reachable by other machines or by web pages the developer happens to visit:

- **Loopback only.** Bind `127.0.0.1` (and `::1` where the host has IPv6); there is no option to bind other interfaces.
- **Access token.** On startup the server generates a random 256-bit token (64 hex digits) and prints the dashboard URL as `http://127.0.0.1:<port>/#token=<token>`. Every `/api/*` request must carry `Authorization: Bearer <token>`, else `401`. The dashboard reads the token from the URL fragment (which browsers never send to servers), keeps it in `sessionStorage`, and removes it from the address bar. A restarted server has a new token: opening its new URL in the same tab reloads the dashboard, and a rejected token is shown as such instead of being retried.
- **DNS-rebinding defense.** Requests whose `Host` header is not `127.0.0.1:<port>`, `localhost:<port>`, or `[::1]:<port>` are rejected with `403` — for every path, including the dashboard assets.
- **CSRF defense.** Requests with any method but `GET` and `HEAD` must have `Content-Type: application/json`, else `415`; requests with an `Origin` header other than `http://<Host>` are rejected with `403`. No CORS headers are sent. Assets carry a `Content-Security-Policy` of `default-src 'self'`.
- **No command execution.** The `verify` field is displayed, never executed — neither by the server nor by the dashboard. This holds for all of Airlock.

---

## 3. Dashboard

### 3.1 Kanban Board
- **Columns** follow the effective status (Spec 01 §2.1): `Blocked`, `Ready`, `In Progress`, `In Review`, `Done`.
- **Role filter** over the roles in the project config.
- **Cards** show id, title, priority, claiming session, and dependency badges. Clicking a card opens the full *Read*, *Touch*, *Do*, *Verify* instructions plus notes.

### 3.2 Review Queue
Lists tasks in `In Review` with their notes; **Approve** and **Reject** (feedback required) map to `task approve` / `task reject`. Tasks that need a human check (hardware tests, manual QA, PR review) are routed here by completing them with `--review`.

### 3.3 Questions Inbox
Lists questions by status. The project lead can answer, revise, and resolve them (optionally recording the spec file that codifies the answer).

### 3.4 Live Leases
Shows active leases with session id and remaining time; expired leases are marked as such. A lead can force-release a lease.

### 3.5 Overlap Rules
Two open tasks (effective status other than `done`) **overlap** when their `touch` lists share a path. Paths are compared after normalization: surrounding whitespace is trimmed, `\` becomes `/`, repeated `/` collapse, and leading `./` are removed (`./src/a.rs` equals `src/a.rs`; `src/` does not equal `src/a.rs`). Only identical paths count; a shared directory does not.

- A task with an empty `touch` list has **unknown** overlap. It is marked "no touch data", never shown as free of overlap.
- A task that is `in_progress` or `in_review` **occupies** its paths: its work is not merged yet. Overlaps with an occupying task are emphasized.
- Kanban cards and the task details carry an overlap badge naming the overlapping tasks and shared paths.

The analysis is a pure function of the task list (`src/overlap.rs`); the API serves it as `GET /api/overlaps`, and a later claim-time warning reuses it.

### 3.6 Stages View
Open tasks in columns by dependency stage: stage 1 holds every open task without open prerequisites (ready, in progress, in review); a blocked task sits in stage `1 + max(stage of its open prerequisites)`. Done prerequisites are ignored. `blocked_by` edges are drawn as arrows. Selecting a task highlights every task it overlaps with and lists the shared paths; selecting it again or pressing Escape clears the selection. The role filter applies.

### 3.7 File Matrix
Rows are paths touched by two or more open tasks, most shared first, then by path; columns are the open tasks touching them. Cells of occupying tasks are marked running. Tasks without `touch` data are listed below the matrix. The matrix always covers all roles, since overlap across roles collides just the same.

### 3.8 Dashboard Session
Answers and force-releases from the dashboard are recorded under the session id `dashboard`.

---

## 4. REST & Event API

All endpoints exchange JSON and require the bearer token (§2). Request bodies use the MCP parameter names (Spec 03). Errors return `{ "error": "<message>" }` with `400` (invalid input), `404` (unknown id), or `409` (invalid transition or lost race).

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/project` | Project id, name, roles (`name`, `description`, `clean_room`), `lease_minutes`, dashboard `session`, Airlock `version` |
| `GET` | `/api/tasks?role=&status=` | List tasks (with effective `status`) |
| `POST` | `/api/tasks` | Create a task (`201`; invalid fields, unknown role or prerequisite, duplicate id: `400`) |
| `GET` | `/api/tasks/{id}` | Task details |
| `PATCH` | `/api/tasks/{id}` | Edit task content with the `task_update` parameters (no field, unknown role or field: `400`; task past `open` without `force`, unknown prerequisite, cycle: `409`) |
| `POST` | `/api/tasks/{id}/release` | Force-release a lease |
| `POST` | `/api/tasks/{id}/approve` | Approve a task under review |
| `POST` | `/api/tasks/{id}/reject` | Reject a task under review (`notes` required) |
| `GET` | `/api/questions?topic=&status=` | List questions |
| `POST` | `/api/questions/{id}/answer` | Answer or revise |
| `POST` | `/api/questions/{id}/resolve` | Resolve |
| `GET` | `/api/overlaps` | Overlap and stage analysis (§3.5–3.7): `tasks` (`id`, `status`, `stage`, `unknown`, `overlaps` of `{ id, occupied, paths }`), `files` (`path`, `tasks`, `occupied_by`), `no_touch` |
| `GET` | `/api/events` | Server-Sent Events: `task_changed`, `question_changed` with data `{ "id": <entity id> }`; `resync` when the client fell behind |

Claiming and completing tasks are session operations and stay with the CLI and MCP. Because other processes write to the same store, the server detects changes by polling SQLite's `PRAGMA data_version` (once per second) and emits SSE events accordingly; it also re-checks when the earliest lease expires, since leases run out without a write. The browser's `EventSource` cannot send the `Authorization` header, so the dashboard reads `/api/events` as a `fetch` stream.
