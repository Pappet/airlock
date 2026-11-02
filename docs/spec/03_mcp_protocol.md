# Spec 03: MCP Protocol (Model Context Protocol)

## 1. Overview

`airlock mcp` runs a Model Context Protocol server over stdio, so AI assistants (Claude Code, Antigravity, Cursor, …) interact with Airlock through typed tool calls instead of parsing CLI output. Tools map one-to-one onto the operations in Spec 01 and share their transition rules. The implementation is `src/mcp.rs`.

Example client configuration (Claude Code `.mcp.json`):

```json
{
  "mcpServers": {
    "airlock": {
      "command": "airlock",
      "args": ["mcp", "--role", "engine", "--session", "engine-1"]
    }
  }
}
```

The example shows the explicit form. In a worktree bound with `airlock use` (Spec 06 §2.3), `"args": ["mcp"]` suffices: role and session come from the binding, and `airlock setup claude` prints exactly that.

### 1.1 Transport and Protocol

- stdio transport: one JSON-RPC 2.0 message per line on stdin/stdout. stdout carries nothing but protocol messages; the startup line (project, session, role) and diagnostics go to stderr. The server exits when stdin closes.
- Protocol revisions `2025-11-25`, `2025-06-18`, `2025-03-26`, and `2024-11-05`. `initialize` echoes the client's requested revision if supported, otherwise offers the newest. The result carries `capabilities.tools`, `serverInfo`, and `instructions` describing the claim/heartbeat/complete workflow.
- Methods: `initialize`, `ping`, `tools/list`, `tools/call`, `prompts/list`, `prompts/get`. The server advertises the `tools` and `prompts` capabilities; its one prompt, `start`, is the session briefing (Spec 06 §3.2). Notifications (e.g. `notifications/initialized`) get no response, and neither do responses sent by the client (the server issues no requests). JSON-RPC batches are not supported.
- The server does not track the lifecycle: calls before `initialize` are served as well. Clients are expected to initialize first; the leniency only keeps local tooling simple.
- `initialize` records the client's `clientInfo.name` as the `agent` of the server's activity log entries (§1.2).
- Errors: unparsable input → `-32700`; a non-object message, or a message with an `id` but without a string `method` that is not a response → `-32600`; unknown method → `-32601`; unknown or hidden tool, or non-object `arguments` → `-32602`.

### 1.2 Activity Log and Process Registry

Every `tools/call` and every `prompts/get` of the `start` prompt is recorded in the activity log (Spec 01 §3.5), as one entry per call: the tool name as `command` (`prompt start` for the prompt), the session and role of the server, whether the role is clean-room, `channel: mcp`, the `clientInfo.name` from `initialize` as `agent`, a `target` (task id, question id, track name) where the arguments name one, and the argument names as `changed`. Argument values are not recorded except the short enumerated ones of Spec 01 §3.5; `do`, `verify`, notes, question and answer text, and descriptions never reach the log. Failures are recorded with their message, including calls to a tool hidden from a clean-room role and unknown tools. Writing the log never changes a response: a failure only prints `airlock: activity log: <error>` on stderr. Protocol methods other than these two (`ping`, `tools/list`, …) are not recorded. No tool exposes the log, and the clean-room tool set (§2.1) is unchanged.

At startup, a server that resolved its project registers itself in the process registry (Spec 01 §3.6) with its session and role, refreshes the entry every 60 seconds, and removes it when stdin closes.

---

## 2. Server Binding

- **Project:** resolved once at startup from `--project`/`AIRLOCK_PROJECT` or the `.airlock` marker above the server's working directory (Spec 02 §2.2). Startup fails if no project resolves. The project configuration (roles) is re-read on every tool call, so an `airlock import` takes effect without a restart.
- **Session:** `--session`/`AIRLOCK_SESSION`, otherwise the worktree binding's session (Spec 06 §2.3), otherwise `mcp-<8 hex digits>` generated at startup. Every mutating tool acts as this session; tools take no session parameter.
- **Role (optional):** `--role <role>` binds the server to one role, otherwise the worktree binding's role applies; startup fails for an unknown role, and a locked binding refuses a different `--role`. A bound server:
  - fills the `role` of `task_next` from the binding and rejects a different one;
  - runs the session tools `task_claim`, `task_heartbeat`, `task_release`, and `task_complete` only on tasks of the bound role;
  - leaves `task_create` and `task_update` unrestricted — their `role` names the role the work is *for*, so a bound lead session can create and edit engine tasks.

### 2.1 Clean-Room Tool Set

If the bound role has `clean_room: true` in the project config, the server exposes only:

`session_start`, `task_next`, `inbox`, `inbox_wait`, `task_get`, `task_claim`, `task_heartbeat`, `task_release`, `task_complete`, `question_ask`, `question_list`

restricted to the bound role's tasks (including `task_get`) and the session's own questions (`question_list`). `task_create`, `task_update`, `task_hold`, `task_resume`, `track_list`, `track_create`, `track_update`, `track_remove`, `question_answer`, `question_resolve`, and `doc_sync` are not listed at all. This keeps an agent from accidentally answering its own questions or browsing other roles' work; it is a guardrail, not a security boundary (Spec 05 §4). Task ids, role names, and track names (which appear in the work packages of the role's own tasks) are coordination metadata, not material from the analysis side, so naming them would be harmless; clean-room errors nevertheless name only the session's own role ("task `L-1` is not available to role `engine`"), keeping the session's view limited to its own work.

Without `--role`, all tools are exposed and `role` parameters are required where listed.

---

## 3. Tools

A successful call returns its result as `structuredContent` (always a JSON object) and, for clients without structured output, the same JSON as a text content block. Invalid or missing arguments and failed operations are returned as tool results with `isError: true` and a one-line message (e.g. ``task `E-6d` is claimed by engine-2 (lease expires in 12 min)``), so the model can correct itself.

### 3.0 `session_start`
The session briefing (Spec 06 §3): role, rules, reading list, claimed work, next package, answered questions, changed answers. Agents call it first. Part of the clean-room tool set.
- **Parameters:** none
- **Returns:** the structured briefing in `structuredContent`; the text content is the briefing as Markdown rather than JSON. Next to `answered_questions`, `structuredContent` carries `corrected_answers`: the session's resolved questions whose answer was corrected since its previous briefing (Spec 06 §3.1), each with the new `answer`, `corrected_at`, `corrected_by`, and `history`. The Markdown shows them under "Answer changed".

### 3.1 `task_next`
Returns the next ready task for a role (Spec 01 §2.1 selection order: track priority, task priority, age) without claiming it.
- **Parameters:** `role` (string; required unless bound)
- **Returns:** `{ task: { id, title, track?, priority, blocked_by, read, touch, do, verify, notes?, progress?, overlaps? } }`, or `{ task: null }` if none is ready. `track` appears when the task has one and `priority` is one of `critical`, `high`, `normal`, `low`. `notes` appears when present, e.g. review feedback after a rejection; `progress` when an earlier holder left a progress note. `overlaps` (`[{ id, occupied, paths }]`) appears when no ready task is free of overlap with running work and this one is handed out anyway. Kept compact — the work package is meant to replace reading the whole plan.

### 3.1a `inbox`
What changed for the role since this session last looked (Spec 01 §2.6): ready, changed, or rejected tasks of the role, answers to its questions, changed track priorities; for coordinating roles also open questions and tasks in review from other roles. Part of the clean-room tool set; a clean-room session never sees questions or reviews.
- **Parameters:** `role` (string; required unless bound), `peek` (boolean, optional: show without marking as read)
- **Returns:** `{ items: [{ kind, at, task?: { id, title, priority, track, status }, question?: { id, topic, question?, answer?, spec_file?, corrected? }, track?: { name, priority } }] }`, oldest first, one item per task, question, or track. A session's first call returns no items and starts its cursor at the newest event.

### 3.1b `inbox_wait`
Waits until the role's inbox holds something, then returns it like `inbox` (read and marked); returns it empty when `timeout_secs` have passed. Meant for sessions that run unattended: instead of ending its turn with nothing to do, such a session waits for the next task or answer (Spec 06 §3.1). The server reads the store again only when another connection committed (`PRAGMA data_version`, checked once a second); while it waits, it answers no other request. Part of the clean-room tool set.
- **Parameters:** `role` (string; required unless bound), `timeout_secs` (integer 1–280, default 240: below DSH's 300-second tool-call timeout, Spec 06 §6)
- **Returns:** `{ items: [...] }` as `inbox`.

### 3.2 `task_get`
- **Parameters:** `task_id` (string)
- **Returns:** the full task including effective `status`, dependents, and lease info, and `progress` when the task has a progress note (Spec 01 §2.4).

### 3.3 `task_claim`
- **Parameters:** `task_id` (string), `lease_minutes` (integer, optional; default from project config)
- **Returns:** `{ task_id, lease_expires_at }`, plus `overlaps` and `question_id` (or `question_error`) when the task shares files with running work, `higher_priority` (`[{ id, title, priority, track? }]`: ready tasks of the role that rank strictly higher by track or task priority) and `holding` (ids of other tasks this session holds a lease on) when there are any. None of these refuses the claim. The claim is not refused; the question for the lead is the session's own, so clean-room sessions see its answer in `question_list` and the briefing (Spec 01 §2.1).

### 3.4 `task_heartbeat`
- **Parameters:** `task_id` (string), `lease_minutes` (integer, optional), `notes` (string, optional): the running note for the session's successor — done so far, remaining, next step. It replaces the previous note; an empty string removes it; without `notes` the note stays (Spec 01 §2.4). There is no separate tool for it, so the clean-room tool set is unchanged.
- **Returns:** `{ task_id, lease_expires_at, progress }` (`progress` is `null` when the task has no note)

### 3.4a `task_hold` / `task_resume`
Put an `open` task on hold because it waits for something outside Airlock, or take it off hold (Spec 01 §2.1, *On Hold*). Not in the clean-room tool set.
- **Parameters:** `task_hold`: `task_id` (string), `reason` (string, required); `task_resume`: `task_id` (string)
- **Returns:** `{ task_id, hold }` / `{ task_id }`. `task_next` passes a held task over and `task_claim` refuses it with the reason; `task_get` and task lists carry `hold`.

### 3.5 `task_release`
- **Parameters:** `task_id` (string)
- **Returns:** `{ task_id }`. Only the lease holder can release via MCP.

### 3.6 `task_complete`
- **Parameters:** `task_id` (string), `notes` (string, optional), `review` (boolean, optional; default `false`)
- **Returns:** `{ task_id, state, now_ready: [task ids], inbox: { items }, next }`. Only the lease holder can complete via MCP. After the completion is committed, the result carries what changed for the task's role since the session last looked (`inbox`, as from the `inbox` tool, read and marked) and what `task_next` hands out now for that role (`next`, a work package or `null`), so a session that runs several tasks in one turn sees a more important task or an answer at the boundary between two tasks (Spec 01 §2.1). A failure to read either is reported as `inbox_error` / `next_error`; it never undoes the completion.

### 3.7 `task_create`
- **Parameters:** `title`, `role`, `do`, `verify` (strings, required); `read`, `touch`, `blocked_by` (string arrays, optional); `priority` (`critical | high | normal | low`, optional; default `normal`); `track` (string, optional; an existing track, see `track_create`). There is no `id` parameter: Airlock assigns it (Spec 01 §2.1), and a call that passes `id` fails with a message saying so.
- **Returns:** the created task, including its new `id`.

### 3.8 `task_update`
- **Parameters:** `task_id` (string, required); `title`, `role`, `do`, `verify` (strings, optional); `read`, `touch`, `blocked_by` (string arrays, optional; replace the list, `[]` empties it); `priority` (`critical | high | normal | low`, optional); `track` (string or `null`, optional; `null` takes the task out of its track, an absent field keeps it); `force` (boolean, optional; default `false`)
- **Returns:** the updated task. Omitted fields are kept; at least one field must be given. Tasks past `open` need `force` (Spec 01 §2.1); changes to `blocked_by` are checked against the whole graph. An unknown track is an error that names the defined tracks; a number as priority is an error that lists the four levels.

### 3.9 Track tools
Tracks are named lines of work with their own priority (Spec 01 §2.5). None of the four tools is part of the clean-room set (§2.1); they are open to every other session, bound or not.

| Tool | Parameters | Returns |
|---|---|---|
| `track_list` | none | `{ tracks: [...] }`, highest priority first, then by name. Each entry: `name`, `priority`, `description`, `created_at`, `updated_at`, `counts` (`{ blocked, ready, in_progress, in_review, done }`, effective status) |
| `track_create` | `name` (string, required); `priority` (`critical \| high \| normal \| low`, optional; default `normal`); `description` (string, optional) | the created track |
| `track_update` | `name` (string, required; the current name); `new_name`, `description` (strings, optional); `priority` (optional) | the updated track. Omitted fields are kept; at least one field must be given. A rename updates every task of the track |
| `track_remove` | `name` (string, required) | `{ removed: <name> }`. Refused while any task, in any state, uses the track |

`track_list` wraps its array in an object because `structuredContent` is always an object; the CLI (`track list --json`) and REST (`GET /api/tracks`) return the bare array.

### 3.10 `question_ask`
- **Parameters:** `topic` (string), `question` (string)
- **Returns:** `{ question_id }`

### 3.11 `question_list`
- **Parameters:** `topic` (string, optional), `status` (`open | answered | resolved`, optional)
- **Returns:** `{ questions: [...] }`, newest first, including answers. Clean-room sessions see only their own questions.

### 3.12 `question_answer`
- **Parameters:** `question_id` (integer), `answer` (string), `spec_file` (string, optional)
- **Returns:** the updated question. Answering a question that already has an answer, `answered` or `resolved`, is a **correction** (Spec 01 §2.2): the new answer replaces the current one, `history` lists the earlier answers (each `{ answer, answered_by, answered_at, spec_file }`, oldest first), `corrected_at` and `corrected_by` are set, and the status is unchanged. A question that was never corrected carries none of the three fields. The tool is not part of the clean-room set, so only non-clean-room sessions can answer or correct. The successful call is logged with `correction` among its `changed` names, which raises an `answer_corrected` problem (Spec 01 §3.7).

### 3.13 `question_resolve`
- **Parameters:** `question_id` (integer), `spec_file` (string, optional)
- **Returns:** the updated question.

### 3.14 `doc_sync`
Equivalent to `airlock sync` without `--force`, writing into the project root the server resolved at startup (requires a server started inside the project, not only via `--project`).
- **Returns:** `{ written: [paths], unchanged: [paths] }`, or an error if the snapshot was changed outside this hub.
