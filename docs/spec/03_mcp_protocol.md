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

### 1.1 Transport and Protocol

- stdio transport: one JSON-RPC 2.0 message per line on stdin/stdout. stdout carries nothing but protocol messages; the startup line (project, session, role) and diagnostics go to stderr. The server exits when stdin closes.
- Protocol revisions `2025-11-25`, `2025-06-18`, `2025-03-26`, and `2024-11-05`. `initialize` echoes the client's requested revision if supported, otherwise offers the newest. The result carries `capabilities.tools`, `serverInfo`, and `instructions` describing the claim/heartbeat/complete workflow.
- Methods: `initialize`, `ping`, `tools/list`, `tools/call`, `prompts/list`, `prompts/get`. The server advertises the `tools` and `prompts` capabilities; its one prompt, `start`, is the session briefing (Spec 06 §3.2). Notifications (e.g. `notifications/initialized`) get no response, and neither do responses sent by the client (the server issues no requests). JSON-RPC batches are not supported.
- The server does not track the lifecycle: calls before `initialize` are served as well. Clients are expected to initialize first; the leniency only keeps local tooling simple.
- Errors: unparsable input → `-32700`; a non-object message, or a message with an `id` but without a string `method` that is not a response → `-32600`; unknown method → `-32601`; unknown or hidden tool, or non-object `arguments` → `-32602`.

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

`session_start`, `task_next`, `task_get`, `task_claim`, `task_heartbeat`, `task_release`, `task_complete`, `question_ask`, `question_list`

restricted to the bound role's tasks (including `task_get`) and the session's own questions (`question_list`). `task_create`, `task_update`, `question_answer`, `question_resolve`, and `doc_sync` are not listed at all. This keeps an agent from accidentally answering its own questions or browsing other roles' work; it is a guardrail, not a security boundary (Spec 05 §4). Task ids and role names are coordination metadata, not material from the analysis side, so naming them would be harmless; clean-room errors nevertheless name only the session's own role ("task `L-1` is not available to role `engine`"), keeping the session's view limited to its own work.

Without `--role`, all tools are exposed and `role` parameters are required where listed.

---

## 3. Tools

A successful call returns its result as `structuredContent` (always a JSON object) and, for clients without structured output, the same JSON as a text content block. Invalid or missing arguments and failed operations are returned as tool results with `isError: true` and a one-line message (e.g. ``task `E-6d` is claimed by engine-2 (lease expires in 12 min)``), so the model can correct itself.

### 3.0 `session_start`
The session briefing (Spec 06 §3): role, rules, reading list, claimed work, next package, answered questions. Agents call it first. Part of the clean-room tool set.
- **Parameters:** none
- **Returns:** the structured briefing in `structuredContent`; the text content is the briefing as Markdown rather than JSON.

### 3.1 `task_next`
Returns the next ready task for a role (Spec 01 §2.1 selection order) without claiming it.
- **Parameters:** `role` (string; required unless bound)
- **Returns:** `{ task: { id, title, blocked_by, read, touch, do, verify, notes?, overlaps? } }`, or `{ task: null }` if none is ready. `notes` appears when present, e.g. review feedback after a rejection. `overlaps` (`[{ id, occupied, paths }]`) appears when no ready task is free of overlap with running work and this one is handed out anyway. Kept compact — the work package is meant to replace reading the whole plan.

### 3.2 `task_get`
- **Parameters:** `task_id` (string)
- **Returns:** the full task including effective `status`, dependents, and lease info.

### 3.3 `task_claim`
- **Parameters:** `task_id` (string), `lease_minutes` (integer, optional; default from project config)
- **Returns:** `{ task_id, lease_expires_at }`, plus `overlaps` and `question_id` when the task shares files with running work. The claim is not refused; the question for the lead is the session's own, so clean-room sessions see its answer in `question_list` and the briefing (Spec 01 §2.1).

### 3.4 `task_heartbeat`
- **Parameters:** `task_id` (string), `lease_minutes` (integer, optional)
- **Returns:** `{ task_id, lease_expires_at }`

### 3.5 `task_release`
- **Parameters:** `task_id` (string)
- **Returns:** `{ task_id }`. Only the lease holder can release via MCP.

### 3.6 `task_complete`
- **Parameters:** `task_id` (string), `notes` (string, optional), `review` (boolean, optional; default `false`)
- **Returns:** `{ task_id, state, now_ready: [task ids] }`. Only the lease holder can complete via MCP.

### 3.7 `task_create`
- **Parameters:** `id`, `title`, `role`, `do`, `verify` (strings, required); `read`, `touch`, `blocked_by` (string arrays, optional); `priority` (integer, optional)
- **Returns:** the created task.

### 3.8 `task_update`
- **Parameters:** `task_id` (string, required); `title`, `role`, `do`, `verify` (strings, optional); `read`, `touch`, `blocked_by` (string arrays, optional; replace the list, `[]` empties it); `priority` (integer, optional); `force` (boolean, optional; default `false`)
- **Returns:** the updated task. Omitted fields are kept; at least one field must be given. Tasks past `open` need `force` (Spec 01 §2.1); changes to `blocked_by` are checked against the whole graph.

### 3.9 `question_ask`
- **Parameters:** `topic` (string), `question` (string)
- **Returns:** `{ question_id }`

### 3.10 `question_list`
- **Parameters:** `topic` (string, optional), `status` (`open | answered | resolved`, optional)
- **Returns:** `{ questions: [...] }`, newest first, including answers. Clean-room sessions see only their own questions.

### 3.11 `question_answer`
- **Parameters:** `question_id` (integer), `answer` (string), `spec_file` (string, optional)
- **Returns:** the updated question.

### 3.12 `question_resolve`
- **Parameters:** `question_id` (integer), `spec_file` (string, optional)
- **Returns:** the updated question.

### 3.13 `doc_sync`
Equivalent to `airlock sync` without `--force`, writing into the project root the server resolved at startup (requires a server started inside the project, not only via `--project`).
- **Returns:** `{ written: [paths], unchanged: [paths] }`, or an error if the snapshot was changed outside this hub.
