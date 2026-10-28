# Spec 03: MCP Protocol (Model Context Protocol)

## 1. Overview

`airlock mcp` runs a Model Context Protocol server over stdio, so AI assistants (Claude Code, Antigravity, Cursor, …) interact with Airlock through typed tool calls instead of parsing CLI output. Tools map one-to-one onto the operations in Spec 01 and share their transition rules.

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

---

## 2. Server Binding

- **Project:** resolved once at startup from `--project`/`AIRLOCK_PROJECT` or the `.airlock` marker above the server's working directory (Spec 02 §2.2). Startup fails if no project resolves.
- **Session:** `--session`/`AIRLOCK_SESSION`, otherwise `mcp-<random>` generated at startup. Every mutating tool acts as this session; tools take no session parameter.
- **Role (optional):** `--role <role>` binds the server to one role. Bound servers fill `role` parameters automatically and reject calls for other roles.

### 2.1 Clean-Room Tool Set

If the bound role has `clean_room: true` in the project config, the server exposes only:

`task_next`, `task_get`, `task_claim`, `task_heartbeat`, `task_release`, `task_complete`, `question_ask`, `question_list`

restricted to the bound role's tasks and the session's own questions. `task_create`, `question_answer`, `question_resolve`, and `doc_sync` are not listed at all. This keeps an agent from accidentally answering its own questions or browsing other roles' work; it is a guardrail, not a security boundary (Spec 05 §4).

Without `--role`, all tools are exposed and `role` parameters are required where listed.

---

## 3. Tools

Errors are returned as MCP tool errors with a one-line message (e.g. `task E-6d is claimed by engine-2 until 14:05`).

### 3.1 `task_next`
Returns the next ready task for a role (Spec 01 §2.1 selection order) without claiming it.
- **Parameters:** `role` (string; required unless bound)
- **Returns:** `{ id, title, blocked_by, read, touch, do, verify }`, or `null` if none is ready. Kept compact — the work package is meant to replace reading the whole plan.

### 3.2 `task_get`
- **Parameters:** `task_id` (string)
- **Returns:** the full task including effective `status`, dependents, and lease info.

### 3.3 `task_claim`
- **Parameters:** `task_id` (string), `lease_minutes` (integer, optional; default from project config)
- **Returns:** `{ task_id, lease_expires_at }`

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

### 3.8 `question_ask`
- **Parameters:** `topic` (string), `question` (string)
- **Returns:** `{ question_id }`

### 3.9 `question_list`
- **Parameters:** `topic` (string, optional), `status` (`open | answered | resolved`, optional)
- **Returns:** matching questions including answers. Clean-room sessions see only their own questions.

### 3.10 `question_answer`
- **Parameters:** `question_id` (integer), `answer` (string), `spec_file` (string, optional)
- **Returns:** the updated question.

### 3.11 `question_resolve`
- **Parameters:** `question_id` (integer), `spec_file` (string, optional)
- **Returns:** the updated question.

### 3.12 `doc_sync`
Equivalent to `airlock sync` without `--force`, writing into the project root the server resolved at startup.
- **Returns:** `{ written: [paths] }`, or an error if the snapshot was changed outside this hub.
