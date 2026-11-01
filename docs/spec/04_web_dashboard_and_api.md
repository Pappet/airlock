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
- **Client header.** The dashboard's requests carry `X-Airlock-Client: dashboard`. It is a label for the activity log (§3.11, Spec 01 §3.5), not authentication: the token above is what protects the API, and any caller can send the header. It makes the dashboard's own `GET` requests (polling after events) invisible in the log and attributes its writes to the session `dashboard`; every other caller is logged as `rest`.
- **No command execution.** The `verify` field is displayed, never executed — neither by the server nor by the dashboard. This holds for all of Airlock.

---

## 3. Dashboard

### 3.1 Kanban Board
- **Columns** follow the effective status (Spec 01 §2.1): `Blocked`, `Ready`, `In Progress`, `In Review`, `Done`.
- **Role filter** over the roles in the project config.
- **Track and priority filters** next to the role filter: track (all / one track / no track) and minimum priority (all / `≥ low` … `≥ critical`). They apply to the Board and the Review Queue; the Tracks tab (§3.9) follows the role filter only.
- **Group by track:** a switch, on by default and remembered per browser. On: one swimlane per track in track order (priority, then name; tasks without a track form a lane at `normal`), columns as above. Off: the flat board.
- **Order within a cell:** grouped, by task priority (highest first), then oldest first. Flat, by track priority, then task priority, then oldest first, which is the `task next` order (Spec 01 §2.1).
- **Cards** show id, title, track, priority, claiming session, and dependency badges. Clicking a card opens the priority and track, the full *Read*, *Touch*, *Do*, *Verify* instructions, and notes.
- **Priority display** (§3.10) is the same on every card.

### 3.2 Review Queue
Lists tasks in `In Review` with their notes; **Approve** and **Reject** (feedback required) map to `task approve` / `task reject`. Tasks that need a human check (hardware tests, manual QA, PR review) are routed here by completing them with `--review`.

### 3.3 Questions Inbox
Lists questions by status. The project lead can answer, revise, and resolve them (optionally recording the spec file that codifies the answer). Revising an answered question is a correction (Spec 01 §2.2): a question that was corrected carries a "corrected" chip, whose tooltip names who corrected it and how many earlier answers are kept. Resolved questions are shown read only; correcting one is done over MCP, CLI, or `POST /api/questions/{id}/answer`, which accept it.

### 3.4 Live Leases
Shows active leases with session id and remaining time; expired leases are marked as such, and a lease whose task has a progress note (Spec 01 §2.4) shows it. A lead can force-release a lease. The task details show the progress note too ("Progress" while the task is claimed, "Earlier progress" after a release); there is no REST endpoint that writes it, since it is the lease holder's.

### 3.5 Overlap Rules
Two open tasks (effective status other than `done`) **overlap** when their `touch` lists share a path. Paths are compared after normalization: surrounding whitespace is trimmed, `\` becomes `/`, repeated `/` collapse, and leading `./` are removed (`./src/a.rs` equals `src/a.rs`; `src/` does not equal `src/a.rs`). Only identical paths count; a shared directory does not.

- A task with an empty `touch` list has **unknown** overlap. It is marked "no touch data", never shown as free of overlap.
- A task that is `in_progress` or `in_review` **occupies** its paths: its work is not merged yet. Overlaps with an occupying task are emphasized.
- Kanban cards and the task details carry an overlap badge naming the overlapping tasks and shared paths.

The analysis is a pure function of the task list (`src/overlap.rs`); the API serves it as `GET /api/overlaps`, and `task next` and `task claim` reuse it for the claim-time warning (Spec 01 §2.1).

### 3.6 Stages View
Open tasks in columns by dependency stage: stage 1 holds every open task without open prerequisites (ready, in progress, in review); a blocked task sits in stage `1 + max(stage of its open prerequisites)`. Done prerequisites are ignored. `blocked_by` edges are drawn as arrows. Selecting a task highlights every task it overlaps with and lists the shared paths; selecting it again or pressing Escape clears the selection. The role filter applies.

### 3.7 File Matrix
Rows are paths touched by two or more open tasks, most shared first, then by path; columns are the open tasks touching them. Cells of occupying tasks are marked running. Tasks without `touch` data are listed below the matrix. The matrix always covers all roles, since overlap across roles collides just the same.

### 3.8 Dashboard Session
Answers and force-releases from the dashboard are recorded under the session id `dashboard`, in the activity log as well (§3.11).

### 3.9 Tracks Tab
The first tab in the tab bar; the default tab remains the Board. Read only: tracks and tasks are created and changed by agents over CLI and MCP.

- **Rows:** one per track, sorted by track priority, then name, plus a row "no track" (at `normal`) when any task has none.
- **Row content:** priority chip, name, description; a segmented bar of done / in review / running / ready / blocked with counts; what is running (task, session, lease minutes left); the most recently finished task (by `updated_at`).
- **Role filter:** the tab follows it like the Board, so counts, bars, and warnings then cover that role's tasks only.
- **Course warning, per role.** For a role R, a track T is off course when
  1. T has at least one `ready` task of R,
  2. no task of R in T is `in_progress`, and
  3. some track of strictly lower priority has a task of R `in_progress`.

  A task without a track counts as a track at `normal`. Only `in_progress` counts as running: a task `in_review` is finished work waiting for a human; it neither makes a track busy nor keeps it from being idle. The row gets a marker and one line per affected role, e.g. "engine: nothing running, 3 ready". The rule is per role because tracks span roles; comparing across roles would warn although each role works on its most important track. With the role filter set, only that role is checked.
- **Click:** selecting a row opens the Board filtered to that track.

### 3.10 Priority Display
Fixed colors on every surface: `critical` red, `high` orange, `normal` cyan, `low` grey. Cards carry a colored left stripe and pips instead of a number: `●●●●` critical, `●●●○` high, `●●○○` normal, `●○○○` low. The task detail shows a priority chip and the track.

### 3.11 Activity Tab
The second tab in the tab bar, after Tracks. It shows what the activity log (Spec 01 §3.5) holds and answers: what needs attention, who works on what, what waits on whom, and what happened in which order. Read only: problems cannot be acknowledged or dismissed (Spec 01 §3.7), and nothing on the tab changes project state.

- **Problems panel** (top left): every current problem of the 30-day window, newest first, with the kind's icon (failures and error bursts in red, the others as warnings), its message, a repeat count, and the time since its last occurrence; "No problems in the last 30 days." when there are none. Selecting a problem narrows the logbook to the calls behind it; selecting it again, or "show all", clears that. A process problem (`old_process`) has no calls behind it and is not selectable.
- **State panel** (top right): the claimed tasks (`in_progress`) with claiming session, track, and lease minutes left (or "lease expired"), and the **blockers**: blocked tasks grouped by the open prerequisites they wait on, with each prerequisite's role and holder, or "nobody on it" when it is unclaimed. Both come from the live task list, not from the log.
- **Session chips:** one per session seen in the window, with its role, its number of calls per channel (MCP, CLI, REST), and the time since its last call; selecting one narrows the logbook to the session, "all" clears it.
- **Logbook:** the calls newest first, grouped by day; each row has time, session, channel badge, command with target and the names changed, and for failures the error. Rows behind a problem are highlighted, red for failures and bursts. A tooltip shows the agent, pid, build, and a clean-room mark. Filters: all, writes only, failures & problems, MCP, CLI, REST. The newest 500 calls are loaded.
- **Role filter:** the tab follows it like the other tabs. With a role selected, calls, sessions, problems, claims, and blockers of other roles, and calls and problems without a role (human CLI, unbound MCP, REST), are hidden; with no role selected they all show.
- **Live:** the tab reloads on the SSE event `activity` (§4) and when it is opened again.

---

## 4. REST & Event API

All endpoints exchange JSON and require the bearer token (§2). Request bodies use the MCP parameter names (Spec 03): `track` names a track, `priority` is one of `critical`, `high`, `normal`, `low` (a number is `400` with a message listing the four levels), and `track: null` on `PATCH` clears the track. Errors return `{ "error": "<message>" }` with `400` (invalid input), `404` (unknown id), or `409` (invalid transition or lost race).

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/project` | Project id, name, roles (`name`, `description`, `clean_room`), `lease_minutes`, dashboard `session`, Airlock `version` |
| `GET` | `/api/tasks?role=&status=&track=&priority=` | List tasks (with effective `status`, `track`, and `priority` as a level name). `track=<name>` keeps one track, `track=` with an empty value keeps tasks without a track; `priority=<level>` keeps tasks of at least that level |
| `POST` | `/api/tasks` | Create a task with the `task_create` parameters; the response carries the assigned id (`201`; invalid or unknown fields including `id`, unknown role, prerequisite, or track, roles sharing a prefix, a numeric `priority`: `400`) |
| `GET` | `/api/tasks/{id}` | Task details |
| `PATCH` | `/api/tasks/{id}` | Edit task content with the `task_update` parameters (no field, unknown role or field, a numeric `priority`: `400`; task past `open` without `force`, unknown prerequisite, unknown track, cycle: `409`) |
| `POST` | `/api/tasks/{id}/release` | Force-release a lease |
| `POST` | `/api/tasks/{id}/approve` | Approve a task under review |
| `POST` | `/api/tasks/{id}/reject` | Reject a task under review (`notes` required) |
| `GET` | `/api/tracks` | Tracks with counts, highest priority first: the bare array of `track_list` entries (`name`, `priority`, `description`, `created_at`, `updated_at`, `counts` with `blocked`, `ready`, `in_progress`, `in_review`, `done`). Tracks are read only over REST |
| `GET` | `/api/questions?topic=&status=` | List questions |
| `POST` | `/api/questions/{id}/answer` | Answer an open question, or correct the answer of an answered or resolved one (Spec 01 §2.2): the status stays, earlier answers move to `history`, and the question carries `corrected_at` and `corrected_by` |
| `POST` | `/api/questions/{id}/resolve` | Resolve |
| `GET` | `/api/overlaps` | Overlap and stage analysis (§3.5–3.7): `tasks` (`id`, `status`, `stage`, `unknown`, `overlaps` of `{ id, occupied, paths }`), `files` (`path`, `tasks`, `occupied_by`), `no_touch` |
| `GET` | `/api/activity?session=&role=&channel=&since=&writes=&failures=&limit=` | Calls of the activity log (Spec 01 §3.5), newest first, as an array of entries (`id`, `at`, `session`, `role`, `clean_room`, `channel`, `agent`, `command`, `target`, `changed`, `write`, `ok`, `error`, `pid`, `build`). `channel` is `mcp`, `cli`, or `rest`; `since` is `30m`, `2h`, `1d`, or unix seconds; `writes=true` keeps calls that change state, `failures=true` failed ones; `limit` defaults to 500. A bad filter is `400`. An absent log is `[]` |
| `GET` | `/api/activity/problems?session=&role=` | The current problems (Spec 01 §3.7) of the 30-day window, newest first: `kind`, `session`, `role`, `command`, `target`, `message`, `count`, `first_at`, `last_at`, `entry_ids` |
| `GET` | `/api/activity/sessions` | Sessions seen in the window, most recently active first: `session`, `role` (of its latest call), `last_at`, `calls` (`{ mcp, cli, rest }`) |
| `GET` | `/api/events` | Server-Sent Events: `task_changed`, `question_changed`, `track_changed` with data `{ "id": <entity id> }` (the track name; a rename emits it for the old and the new name); `activity` with data `{ "id": <id of the newest log entry> }` whenever calls were logged; `resync` when the client fell behind |

Claiming and completing tasks are session operations and stay with the CLI and MCP. Because other processes write to the same store, the server detects changes by polling SQLite's `PRAGMA data_version` (once per second) and emits SSE events accordingly; it also re-checks when the earliest lease expires, since leases run out without a write. The same poll follows the activity log's newest entry id for the `activity` event. The server registers itself in the process registry (Spec 01 §3.6) as kind `serve` and refreshes it every 60 seconds.

Every accepted API request is recorded in the activity log (Spec 01 §3.5) under the command `<METHOD> <route pattern>` (for example `POST /api/tasks/{id}/approve`), as session `dashboard` when it carries `X-Airlock-Client: dashboard` and `rest` otherwise, without a role. Not recorded: `/api/events`, the `/api/activity*` endpoints, `GET` requests with the dashboard header, requests refused by the security checks of §2, and paths that match no route. To summarise a request for the log, Airlock reads its body first; a logged request with a body over 1 MiB is refused with `413 request body too large` before it reaches its handler (no endpoint takes bodies anywhere near that size). The browser's `EventSource` cannot send the `Authorization` header, so the dashboard reads `/api/events` as a `fetch` stream.
