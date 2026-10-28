# Spec 04: Web Dashboard & REST/SSE API

## 1. Overview

`airlock serve [--port <port>]` serves a dashboard and a JSON REST API for one project on `127.0.0.1:3000` (port configurable). All HTML, CSS, and JavaScript are embedded in the binary; the dashboard loads nothing from external hosts. The server operates on the hub store like any CLI invocation (Spec 01 §3.2); it is a convenience UI, not a required daemon.

---

## 2. Security

The API can mutate project state and exposes every question and answer, so it must not be reachable by other machines or by web pages the developer happens to visit:

- **Loopback only.** Bind `127.0.0.1` (and `::1`); there is no option to bind other interfaces.
- **Access token.** On startup the server generates a random 256-bit token and prints the dashboard URL as `http://127.0.0.1:<port>/#token=<token>`. Every `/api/*` request must carry `Authorization: Bearer <token>`; the dashboard reads the token from the URL fragment (which browsers never send to servers) and keeps it in `sessionStorage`.
- **DNS-rebinding defense.** Requests whose `Host` header is not `127.0.0.1:<port>`, `localhost:<port>`, or `[::1]:<port>` are rejected with `403`.
- **CSRF defense.** Mutating requests must have `Content-Type: application/json`; requests with an `Origin` header other than the server's own origin are rejected. No CORS headers are sent.
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

---

## 4. REST & Event API

All endpoints exchange JSON and require the bearer token (§2). Request bodies use the MCP parameter names (Spec 03). Errors return `{ "error": "<message>" }` with `400` (invalid input), `404` (unknown id), or `409` (invalid transition or lost race).

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/tasks?role=&status=` | List tasks (with effective `status`) |
| `POST` | `/api/tasks` | Create a task |
| `GET` | `/api/tasks/{id}` | Task details |
| `POST` | `/api/tasks/{id}/release` | Force-release a lease |
| `POST` | `/api/tasks/{id}/approve` | Approve a task under review |
| `POST` | `/api/tasks/{id}/reject` | Reject a task under review (`notes` required) |
| `GET` | `/api/questions?topic=&status=` | List questions |
| `POST` | `/api/questions/{id}/answer` | Answer or revise |
| `POST` | `/api/questions/{id}/resolve` | Resolve |
| `GET` | `/api/events` | Server-Sent Events: `task_changed`, `question_changed` with the entity id |

Claiming and completing tasks are session operations and stay with the CLI and MCP. Because other processes write to the same store, the server detects changes by polling SQLite's `PRAGMA data_version` (once per second) and emits SSE events accordingly.
