# Spec 02: CLI Interface

## 1. Overview

The CLI gives human developers, shell scripts, and CI runners direct access to all coordination features. The argument definitions live in `src/cli.rs`; this document describes their behavior.

---

## 2. Global Behavior

### 2.1 Global Options

| Option | Env | Meaning |
|---|---|---|
| `--project <id-or-name>` | `AIRLOCK_PROJECT` | Select the project explicitly (see 2.2) |
| `--session <id>` | `AIRLOCK_SESSION` | Session id for claims, questions, and answers; default `human:$USER` |
| `--json` | — | Print machine-readable JSON instead of human-readable text |

Global options may appear before or after the subcommand.

### 2.2 Project Resolution

Every command except `init` operates on one project, resolved in this order:
1. `--project` / `AIRLOCK_PROJECT`: a project uuid, or a project name that is unique in the hub.
2. The nearest `.airlock` marker found by walking up from the current directory.

The directory containing the marker is the **project root** for this invocation; `sync` and `import` resolve relative paths against it. With `--project` and no marker in the current directory tree, commands that need a project root (`sync`, `import`) fail. A marker whose id is unknown to the hub fails with a hint to run `airlock init` (see Spec 05 §2.3). Airlock never inspects git internals to locate state.

### 2.3 Output and Exit Codes

- Human output goes to stdout; diagnostics go to stderr.
- With `--json`, a successful command prints exactly one JSON document on stdout. Effective task status is included as `status` next to the stored `state`. On failure, stdout is empty and the message goes to stderr.
- Exit codes: `0` success, `1` operational error (unknown task, lost claim race, invalid transition, unknown project), `2` usage error (clap).

---

## 3. Command Reference

### 3.1 `airlock init [--name <name>] [--template <name>]`

- **No marker in the current directory:** creates a project with a new uuid in the hub (`project.json` from the template, empty database) and writes `.airlock` into the current directory. `--name` defaults to the directory name and must be unique in the hub. Fails if a parent directory already holds a marker (no nested projects).
- **Marker present, project known to the hub:** no-op; prints the project id and name.
- **Marker present, project unknown to the hub** (fresh clone on a new machine): creates the hub project *with the marker's id*. If the configured snapshot file exists, prints a hint to run `airlock import`.

Commit `.airlock` to the repository.

### 3.2 Task Operations

#### `airlock task list [--role <role>] [--status <status>]`
Lists tasks as a table (id, status, role, priority, claimed by, title). `<status>` is one of `blocked`, `ready`, `in_progress`, `in_review`, `done` (effective status, Spec 01 §2.1).

#### `airlock task next --role <role>`
Shows the next ready task for the role (selection order: Spec 01 §2.1) as a compact work package: id, title, prerequisites, **Read**, **Touch**, **Do**, **Verify**. Does not claim it. Exits `1` with a clear message if no task is ready.

#### `airlock task show <id>`
Shows one task in full, including dependents and lease information.

#### `airlock task add <id> "<title>" --role <role> --do "<instructions>" --verify "<command>" [options]`
```bash
airlock task add E-6d "Missiles implementation" --role engine \
  --read docs/spec/missiles.md --read docs/spec/combat.md \
  --touch src/missiles.rs \
  --do "Implement shot clock and volley firing cycle" \
  --verify "cargo test missiles" \
  --blocked-by L-1,L-2 --priority 10
```
`--read` and `--touch` are repeatable; `--blocked-by` takes a comma-separated list of **existing** task ids. Task ids consist of ASCII letters, digits, `-`, `_`, and `.`. Fails on an invalid or duplicate id, an unknown role, or an unknown prerequisite.

#### `airlock task claim <id> [--lease <minutes>]`
Claims a `ready` task (or one whose lease expired) for the current session and starts a lease. Fails if another session holds a live lease or the task is blocked.

#### `airlock task heartbeat <id> [--lease <minutes>]`
Extends the current session's lease from now. Fails if the session does not hold the lease.

#### `airlock task release <id> [--force]`
Returns a claimed task to `open`. Without `--force`, only the lease holder may release.

#### `airlock task done <id> [--notes <text>] [--review] [--force]`
Finishes a claimed task: `done`, or `in_review` with `--review`. Without `--force`, only the lease holder may finish it. Prints the ids of tasks that became ready.

#### `airlock task approve <id> [--notes <text>]` / `airlock task reject <id> --notes <text>`
Resolves a task under review: `approve` → `done`; `reject` → `open` with the feedback stored in `notes`.

### 3.3 Question Operations

#### `airlock question ask "<question>" --topic <spec-or-subsystem>`
Submits a question; prints its id.

#### `airlock question list [--topic <topic>] [--status <open|answered|resolved>]`
Lists questions, newest first.

#### `airlock question answer <id> "<answer>" [--spec <file>]`
Answers or revises the answer to a question that is not resolved.

#### `airlock question resolve <id> [--spec <file>]`
Closes an answered question, optionally recording the spec file that now codifies the answer.

### 3.4 Servers and Export

#### `airlock mcp [--role <role>]`
Runs the MCP server over stdio (Spec 03). `--role` binds the server to one role.

#### `airlock serve [--port <port>]`
Serves the dashboard and REST/SSE API on `127.0.0.1:<port>` (default 3000; Spec 04).

#### `airlock sync [--force]`
Writes the canonical snapshot and Markdown views into the project root (Spec 05 §3). Refuses to overwrite a snapshot changed outside this hub unless `--force` is given (Spec 05 §3.5).

#### `airlock import [--from <path>] [--yes]`
Replaces the project's tasks, questions, and roles with a snapshot (default: the configured `state_file` in the project root). Validates before writing (Spec 05 §3.4). Asks for confirmation on a terminal unless `--yes` is given; without a terminal, `--yes` is required. Markdown files are never parsed for reconstruction.
