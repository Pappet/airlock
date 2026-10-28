# Spec 02: CLI Interface

## 1. Overview

The CLI provides human developers, terminal scripts, and CI runners with direct command-line access to all task coordination features.

---

## 2. Command Reference

### 2.1 Initialization

```bash
airlock init [--template <name>]
```
- Creates `.airlock/` directory and `.airlock.json` (or `.airlock.toml`) configuration if not present.
- Initializes database schema in `.airlock/airlock.db`.

### 2.2 Task Operations

#### `airlock task list`
```bash
airlock task list [--role <role>] [--status <status>] [--json]
```
Displays tasks in a formatted table or as raw JSON.

#### `airlock task next`
```bash
airlock task next --role <role> [--json]
```
Fetches the highest-priority `Ready` task for the specified role. Returns the task in concise, context-optimized Markdown or JSON, showing:
- Task ID & Title
- Prerequisites & Dependents
- **Read**: files to consult
- **Touch**: files to edit
- **Do**: implementation instructions
- **Verify**: verification command

#### `airlock task claim`
```bash
airlock task claim <task-id> [--session <session-id>] [--duration <minutes>]
```
Claims a task, setting its status to `InProgress` and recording the session ID and lease timestamp.

#### `airlock task done`
```bash
airlock task done <task-id> [--notes <summary>]
```
Marks a task as `Done`. Automatically checks dependent tasks: any task whose `blocked_by` dependencies are now fully satisfied transitions from `Blocked` to `Ready`.

#### `airlock task add`
```bash
airlock task add <id> "<title>" --role <role> \
  --read "docs/spec/foo.md" \
  --touch "src/foo.rs" \
  --do "Implement foo" \
  --verify "cargo test foo" \
  [--blocked-by "L-1,L-2"]
```
Creates a new work package.

---

### 2.3 Airlock Question Operations

#### `airlock question ask`
```bash
airlock question ask "<question>" --topic <spec-or-subsystem> [--session <id>]
```
Submits a question across the airlock.

#### `airlock question list`
```bash
airlock question list [--topic <topic>] [--status <open|answered|resolved>]
```
Lists open or answered questions.

#### `airlock question answer`
```bash
airlock question answer <id> "<answer>" [--session <id>]
```
Answers an open question.

---

### 2.4 Server & Integration

#### `airlock mcp`
Starts the stdio Model Context Protocol (MCP) server for direct LLM agent integration.

#### `airlock serve [--port <port>]`
Starts the local HTTP server hosting the REST API, Server-Sent Events (SSE), and embedded web dashboard.

#### `airlock sync`
Deterministically exports the current database state into configured project files (e.g. `docs/PLAN.md` and `docs/spec/QUESTIONS.md`).
