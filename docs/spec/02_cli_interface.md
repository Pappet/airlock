# Spec 02: CLI Interface

## 1. Overview

The CLI gives human developers, shell scripts, and CI runners direct access to all coordination features. The argument definitions live in `src/cli.rs`; this document describes their behavior.

---

## 2. Global Behavior

### 2.1 Global Options

| Option | Env | Meaning |
|---|---|---|
| `-C <dir>` | — | Run as if started in `<dir>` (Spec 06 §2.4) |
| `--project <id-or-name>` | `AIRLOCK_PROJECT` | Select the project explicitly (see 2.2) |
| `--session <id>` | `AIRLOCK_SESSION` | Session id for claims, questions, and answers; default: the worktree binding's session (Spec 06 §2.3), else `human:$USER`. `airlock activity` takes it as a filter |
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
- Every command except `init`, `doctor`, `mcp`, `serve`, `setup`, and `activity` is recorded in the activity log (§2.4).
- Exit codes: `0` success, `1` operational error (unknown task, lost claim race, invalid transition, unknown project), `2` usage error (clap).

### 2.4 Activity Log and Agent Recognition

Once the project is resolved, each CLI invocation leaves an entry in the activity log (Spec 01 §3.5): session, role, command, target, the names of the arguments given, and the result or error. A failure to write the entry prints `airlock: activity log: <error>` to stderr and never changes the command's outcome or exit code. The CLI cannot tell an agent from the user by its session id alone, since a worktree binding gives both the same session. The entry's `agent` field is therefore taken from the process environment:

| Variable | Result |
|---|---|
| `AI_AGENT` | the text before its first `_` (`claude-code_2.1` gives `claude-code`) |
| `CLAUDECODE=1` | `claude-code`, when `AI_AGENT` is absent or empty |

Only these two variables are recognised. Other harnesses, among them DSH and Antigravity, have not been verified to set either; their CLI calls show as plain CLI calls. A harness that sets neither shows as a plain CLI call; the log then has no `agent` for it, and its calls are still listed and still count toward problems (except `use`, which counts only from a recognised agent, Spec 01 §3.7).

---

## 3. Command Reference

### 3.1 `airlock init [--name <name>] [--template <default|single|clean-room|parallel>] [--agents <n>] [--yes]`

- **No marker in the current directory:** creates a project with a new uuid in the hub (`project.json` with the template's roles, empty database) and writes `.airlock` into the current directory. `--name` defaults to the directory name and must be unique in the hub. Fails if a parent directory already holds a marker (no nested projects).
- **Marker present, project known to the hub:** applies `airlock.toml` if present (creates missing worktrees, binds unbound ones); otherwise a no-op that prints the project id and name.
- **Marker present, project unknown to the hub** (fresh clone on a new machine): creates the hub project *with the marker's id* and applies `airlock.toml`. If the configured snapshot file exists, prints a hint to run `airlock import`.

Templates, the layout file, commits, and worktree creation are specified in Spec 06 §7. Without a template, init never commits: commit `.airlock` to the repository yourself.

### 3.2 Task Operations

#### `airlock task list [--role <role>] [--status <status>] [--track <name> | --no-track] [--priority <level>]`
Lists tasks as a table (id, status, role, priority, track, claimed by, title). `<status>` is one of `blocked`, `ready`, `in_progress`, `in_review`, `done` (effective status, Spec 01 §2.1). `--track` keeps the tasks of one track (an unknown track is an error that names the defined tracks), `--no-track` those without a track (the two exclude each other). `--priority` means "at least this level": `--priority high` lists `high` and `critical` tasks.

#### `airlock task next [--role <role>]`
Shows the next ready task for the role (default: the worktree binding's role) (selection order, track priority first: Spec 01 §2.1) as a compact work package: id, title, track (when set) and priority, prerequisites, **Read**, **Touch**, **Do**, **Verify**. Does not claim it. With `--json` it prints the task with its `track` (the track name, `null` when the task has none) and `priority` (`critical`, `high`, `normal`, or `low`). Exits `1` with a clear message if no task is ready. Tasks sharing files with running work are handed out last (Spec 01 §2.1); if one is returned anyway, a warning naming the shared paths goes to stderr and `--json` carries `overlaps: [{ id, occupied, paths }]`.

#### `airlock task show <id>`
Shows one task in full, including dependents and lease information.

#### `airlock task add "<title>" --role <role> --do "<instructions>" --verify "<command>" [options]`
```bash
airlock task add "Missiles implementation" --role engine \
  --read docs/spec/missiles.md --read docs/spec/combat.md \
  --touch src/missiles.rs \
  --do "Implement shot clock and volley firing cycle" \
  --verify "cargo test missiles" \
  --blocked-by L-1,L-2 --track Core --priority high
```
Airlock assigns the id: the role's prefix and the next free number (Spec 01 §2.1, e.g. `Added E-7 (ready)`). `--read` and `--touch` are repeatable; `--blocked-by` takes a comma-separated list of **existing** task ids. `--priority` is `critical`, `high`, `normal` (default), or `low`; a number is rejected. `--track` names an **existing** track (`airlock track add`, §3.3). Fails on an unknown role, an unknown prerequisite, an unknown track (the error names the defined tracks), or roles sharing a prefix.

A script that adds dependent tasks reads each new id from the JSON output:
```bash
spec=$(airlock --json task add "Write missiles spec" --role spec --do "…" --verify "…" | jq -r .id)
airlock task add "Missiles implementation" --role engine --blocked-by "$spec" --do "…" --verify "…"
```

#### `airlock task edit <id> [--title <t>] [--role <r>] [--priority <level>] [--track <name> | --clear-track] [--do <i>] [--verify <v>] [--read <f>…] [--touch <f>…] [--blocked-by <ids>] [--force]`
```bash
airlock task edit E-6d --touch src/missiles.rs --touch src/combat.rs --blocked-by L-1
airlock task edit E-6d --clear-blocked-by --priority low
airlock task edit E-6d --track Core        # or --clear-track
```
Changes the content of a task; omitted fields keep their values, at least one field must be given. `--read`, `--touch`, and `--blocked-by` replace their whole list; `--clear-read`, `--clear-touch`, and `--clear-blocked-by` empty it; `--clear-track` takes the task out of its track. The id, state, claim, lease, and notes cannot be edited. A task that is `in_progress`, `in_review`, or `done` needs `--force`; the claim and lease survive the edit. Fails on an unknown role, an unknown track, an unknown prerequisite, or a dependency cycle (the whole graph is checked, Spec 01 §2.1).

#### `airlock task claim <id> [--lease <minutes>]`
Claims a `ready` task (or one whose lease expired) for the current session and starts a lease. Fails if another session holds a live lease or the task is blocked. If the task shares files with an `in_progress` or `in_review` task, the claim still succeeds: a warning goes to stderr, an overlap question is recorded for the lead (Spec 01 §2.1), and `--json` adds `overlaps` and `question_id` (or `question_error` if the question could not be recorded).

#### `airlock task heartbeat <id> [--lease <minutes>] [--notes <text>]`
Extends the current session's lease from now. Fails if the session does not hold the lease. `--notes` replaces the task's progress note for a successor (done so far, remaining, next step; Spec 01 §2.4); `--notes ""` removes it, and without `--notes` the note stays. `task show` prints it as `**Progress:**`.

#### `airlock task release <id> [--force]`
Returns a claimed task to `open`. Without `--force`, only the lease holder may release.

#### `airlock task done <id> [--notes <text>] [--review] [--force]`
Finishes a claimed task: `done`, or `in_review` with `--review`. Without `--force`, only the lease holder may finish it. Prints the ids of tasks that became ready.

#### `airlock task approve <id> [--notes <text>]` / `airlock task reject <id> --notes <text>`
Resolves a task under review: `approve` → `done`; `reject` → `open` with the feedback stored in `notes`.

### 3.3 Track Operations

Tracks are named lines of work with their own priority (Spec 01 §2.5). Any session may manage them; clean-room MCP sessions cannot (Spec 03 §2.1).

#### `airlock track list`
Lists tracks, highest priority first, then by name, with their task counts per effective status (`blocked`, `ready`, `in_progress`, `in_review`, `done`). With `--json` it prints the bare array of `{ name, priority, description, created_at, updated_at, counts }`.

#### `airlock track add <name> [--priority <level>] [--description <text>]`
```bash
airlock track add Comfort --priority low --description "Things we COULD build into the engine"
```
Creates a track. The name consists of letters, digits, `-`, and `_`, is case-sensitive, and must not exist yet. `--priority` defaults to `normal`.

#### `airlock track edit <name> [--name <new>] [--priority <level>] [--description <text>]`
Renames or changes a track; omitted fields keep their values and at least one must be given. A rename updates every task of the track in one transaction.

#### `airlock track remove <name>`
Removes a track. Refused while any task (in any state) uses it: move or clear those tasks first (`task edit --track` / `--clear-track`; tasks past `open` need `--force`).

### 3.4 Question Operations

#### `airlock question ask "<question>" --topic <spec-or-subsystem>`
Submits a question; prints its id.

#### `airlock question list [--topic <topic>] [--status <open|answered|resolved>]`
Lists questions, newest first.

#### `airlock question answer <id> "<answer>" [--spec <file>]`
Answers a question. On a question that already has an answer (`answered` or `resolved`) this is a **correction**: the new answer replaces the current one, the earlier answer is kept in the question's history, the status stays (a resolved question stays resolved), and the question records who corrected it and when (Spec 01 §2.2). A given `--spec` replaces the recorded spec file; without it, the recorded one stays. `--json` prints the question with `history`, `corrected_at`, and `corrected_by` where they apply. The session that asked sees the correction in its next briefing (Spec 06 §3.1).

#### `airlock question resolve <id> [--spec <file>]`
Closes an answered question, optionally recording the spec file that now codifies the answer.

### 3.5 Servers and Export

#### `airlock mcp [--role <role>]`
Runs the MCP server over stdio (Spec 03). `--role` binds the server to one role; without it, the worktree binding's role applies (Spec 06 §2.3).

#### `airlock serve [--port <port>]`
Serves the dashboard and REST/SSE API on `127.0.0.1:<port>` and `[::1]:<port>` (default 3000; `0` picks a free port; Spec 04). Prints `Dashboard: http://127.0.0.1:<port>/#token=<token>` on stdout and runs until stopped. The token changes on every start.

#### `airlock sync [--force]`
Writes the canonical snapshot and Markdown views into the project root (Spec 05 §3). Refuses to overwrite a snapshot changed outside this hub unless `--force` is given (Spec 05 §3.5).

#### `airlock import [--from <path>] [--yes]`
Replaces the project's tasks, tracks, questions, and roles with a snapshot (default: the configured `state_file` in the project root). Validates before writing (Spec 05 §3.4). Asks for confirmation on a terminal unless `--yes` is given; without a terminal, `--yes` is required. Markdown files are never parsed for reconstruction.

### 3.6 Onboarding (Spec 06)

| Command | Purpose |
|---|---|
| `airlock use [<role>] [--lock] [--force] [--clear]` | Bind this worktree to a role and session, or show the binding |
| `airlock start [--role <role>]` | Print the session briefing (role, rules, claimed work, other sessions' claims of the role, next package); `--role` / `AIRLOCK_ROLE` for a second role in a shared checkout |
| `airlock role list` / `role set <name> …` / `role remove <name>` | Manage roles, their instructions, and their reading list |
| `airlock doctor` | Check binary, project, worktrees, bindings, and harness configurations; exits `1` on errors |
| `airlock setup <claude\|antigravity\|dsh> [--role <role>]` | Print the MCP configuration of this worktree for a harness |

### 3.7 `airlock activity`

```
airlock activity [--session <id>] [--role <role>] [--channel <mcp|cli|rest>]
                 [--since <30m|2h|1d|unix seconds>] [--writes] [--failures]
                 [--limit <n>] [--problems] [--json]
```

Prints the activity log (Spec 01 §3.5), newest first, grouped under a date heading marked UTC (`── 2026-11-01 (UTC)`), one line per call: time (UTC), session, channel, command, target, the names changed, and for failures the error. It is the terminal view of the dashboard's Activity tab (Spec 04 §3.11) and the source of evidence for validation runs (`docs/walkthroughs/agent-validation.md`).

- `--session` is the global option (§2.1), so `AIRLOCK_SESSION` narrows the output too; a worktree binding does not (without either, all sessions are shown).
- `--role`, `--channel`, `--since` (`30m`, `2h`, `1d`, or unix seconds; the unit is minutes, hours, or days ago), `--writes` (only calls that change state), and `--failures` (only failed calls) each narrow the result; `--limit` caps the number of calls (default 50).
- `--problems` lists the problems of the whole 30-day window instead of calls (Spec 01 §3.7), narrowed only by `--session` and `--role`; the other filters do not apply to problems. Each line starts with the time of the problem's latest entry, `YYYY-MM-DD HH:MM UTC`.
- `--json` prints the entries (or problems) as one array with the fields of Spec 01 §3.5.
- With nothing to show it prints `No activity.` (`No problems.` for `--problems`; `[]` with `--json`). Reading the log is itself not logged.
