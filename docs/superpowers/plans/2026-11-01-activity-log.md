# Activity Log and Answer Corrections Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Record every Airlock call from MCP, CLI and REST, show it in a dashboard Activity tab with problems, state and blockers on top, and let answers be corrected after resolving, with the asker told in the next briefing.

**Architecture:** A separate SQLite file `activity.db` next to each project's `airlock.db` holds log entries and the registry of running `airlock mcp` / `airlock serve` processes, so logging never touches the store's `data_version`, locks or schema. Each channel records at its single entry point (MCP `tools_call` / `prompts_get`, CLI `app::run`, REST middleware); a log failure prints a warning and never fails the operation. Problems are computed from entries and live processes by one pure function; answer corrections are a store change (schema v5, snapshot format 3).

**Tech Stack:** Rust (rusqlite, axum, clap, serde), embedded vanilla JS dashboard.

**Spec:** `docs/superpowers/specs/2026-11-01-activity-log-design.md`

## Global Constraints

- No free-text bodies in the log: never store `do`, `verify`, notes, question or answer text, descriptions, titles. `changed` holds field names; only these short enumerated values may be attached as `name: value`: `priority`, `track`, `role`, `status`, `review`, `lease_minutes`, `clean_room`, `lock`.
- `error` is the message verbatim, cut to 300 characters.
- Retention: entries older than 30 days are deleted, on write.
- A failure to write a log entry never fails the operation; it prints `airlock: activity log: <error>` to stderr.
- Dashboard reads (requests with `X-Airlock-Client: dashboard` and method GET) and `/api/events` are not logged; `airlock activity` and `/api/activity*` are not logged.
- No MCP tool exposes the log.
- The term for the new tab is **Activity**; problem kinds are `failure`, `clean_room_cli`, `error_burst`, `answer_corrected`, `old_process`.
- Answer corrections: allowed for whoever may answer; a resolved question stays resolved; earlier answers are kept.
- Dashboard stays read-only: no acknowledging problems.

## Review Focus

1. `activity.db` unwritable (e.g. a directory in its place) → every command still succeeds and prints one warning — test in Task 3.
2. Several processes logging at once → no lost entries, no `database is locked` failures — test in Task 1 (parallel writers).
3. A `task add` with a long `--do`, and an answer text → neither text appears anywhere in `activity.db` — tests in Tasks 2 and 3.
4. A clean-room MCP session calling a hidden tool (`task_create`) → logged as a failure with `unknown tool` — test in Task 2.
5. Answering an *open* question is a normal answer, not a correction; a version-2 snapshot imports with empty history — tests in Task 5.

---

### Task 1: Activity log store, process registry, build identity

**Files:**
- Create: `build.rs`, `src/activity.rs`
- Modify: `src/lib.rs` (add `pub mod activity;`), `Cargo.toml` (`build = "build.rs"` only if needed)
- Test: `tests/activity.rs`

**Interfaces:**
- Produces (all in `airlock::activity`):
  - `pub const RETENTION_SECS: i64 = 30 * 86_400; pub const LIVE_SECS: i64 = 180; pub const TOUCH_SECS: u64 = 60; pub const ERROR_MAX: usize = 300;`
  - `pub const BRIEFING_COMMANDS: &[&str] = &["session_start", "prompt start", "start"];`
  - `#[serde(rename_all = "lowercase")] pub enum Channel { Mcp, Cli, Rest }`
  - `pub struct Entry { pub id: u64, pub at: i64, pub session: String, pub role: Option<String>, pub clean_room: bool, pub channel: Channel, pub agent: Option<String>, pub command: String, pub target: Option<String>, pub changed: Vec<String>, pub write: bool, pub ok: bool, pub error: Option<String>, pub pid: u32, pub build: String }` (Serialize/Deserialize/Clone/Debug/PartialEq; `id` is 0 before recording)
  - `pub struct Filter { pub session: Option<String>, pub role: Option<String>, pub channel: Option<Channel>, pub since: Option<i64>, pub writes_only: bool, pub failures_only: bool, pub limit: Option<usize> }` (Default)
  - `pub struct Process { pub id: u64, pub pid: u32, pub build: String, pub kind: String, pub session: Option<String>, pub role: Option<String>, pub started_at: i64, pub last_seen: i64 }` (`kind`: `"mcp"` or `"serve"`)
  - `pub struct ActivityLog` with `open(path: &Path) -> Result<Self>`, `path_for(database: &Path) -> PathBuf` (sibling `activity.db`), `record(&mut self, entry: &Entry) -> Result<u64>`, `entries(&self, filter: &Filter) -> Result<Vec<Entry>>` (newest first), `latest_id(&self) -> Result<u64>`, `last_briefing(&self, session: &str) -> Result<Option<i64>>`, `register(&mut self, kind: &str, session: Option<&str>, role: Option<&str>, now: i64) -> Result<u64>`, `touch(&mut self, id: u64, now: i64) -> Result<()>`, `unregister(&mut self, id: u64) -> Result<()>`, `processes(&mut self, now: i64) -> Result<Vec<Process>>` (live only; deletes rows with `last_seen < now - LIVE_SECS`)
  - `pub fn record_quietly(database: &Path, entry: &Entry)`
  - `pub fn build() -> &'static str` (value of `env!("AIRLOCK_BUILD")`, e.g. `0.1.0+1793526864`), `pub fn build_time(build: &str) -> i64` (seconds after `+`, 0 when absent or unparsable)
  - `pub fn agent_from_env(var: impl Fn(&str) -> Option<String>) -> Option<String>`
  - `pub fn truncate_error(message: &str) -> String`

- [ ] **Step 1: Write failing tests in `tests/activity.rs`**

```rust
#[test] fn record_and_read_back_newest_first()       // two entries, entries(&Filter::default()) returns them newest first with ids 2, 1
#[test] fn filters_by_session_role_channel_writes_failures_and_since()
#[test] fn prunes_entries_older_than_thirty_days()  // record at t, then at t + RETENTION_SECS + 1 → only the second remains
#[test] fn last_briefing_is_latest_briefing_command_of_that_session() // "task_claim" ignored, "session_start" at 100 and "start" at 200 → Some(200); other session → None
#[test] fn processes_are_live_until_they_stop_touching() // register at 0, processes(LIVE_SECS) has it, processes(LIVE_SECS + 1) is empty and the row is gone
#[test] fn parallel_writers_lose_nothing()          // 8 threads × 25 record() on the same file through separate ActivityLog::open → 200 entries
#[test] fn record_quietly_survives_an_unwritable_path() // database path whose sibling activity.db is a directory: no panic
#[test] fn build_time_parses_the_suffix()           // build_time("0.1.0+1700000000") == 1700000000; build_time("0.1.0") == 0
#[test] fn agent_from_env_recognises_claude_code()  // AI_AGENT="claude-code_2-1-316_agent" → Some("claude-code"); CLAUDECODE="1" alone → Some("claude-code"); nothing → None
#[test] fn truncate_error_keeps_300_chars()         // 400 'x' → 300 chars; on a char boundary for multi-byte text
```

- [ ] **Step 2: Run** `cargo test --test activity` — Expected: FAIL (module missing).

- [ ] **Step 3: Implement.** `build.rs` emits `cargo:rustc-env=AIRLOCK_BUILD={CARGO_PKG_VERSION}+{unix seconds now}` and `cargo:rerun-if-changed=src` (a rebuild of the sources gets a new build time; rebuilding unchanged sources may keep the old one — fine). `ActivityLog::open` uses WAL, `busy_timeout` 5 s, its own `user_version` migrations; tables `entries` (columns as `Entry`, `changed` as JSON text) with an index on `at`, and `processes`. `record` prunes `at < entry.at - RETENTION_SECS` in the same transaction and truncates `error` with `truncate_error`. `record_quietly` opens `path_for(database)`, records, and on any error prints `airlock: activity log: {e:#}` to stderr. `agent_from_env`: `AI_AGENT` → text before the first `_`; else `CLAUDECODE == "1"` → `claude-code`; else None.

- [ ] **Step 4: Run** `cargo test --test activity` — Expected: PASS. Then `cargo clippy --all-targets -- -D warnings`.

- [ ] **Step 5: Commit** `feat: activity log store and process registry`

---

### Task 2: Record MCP calls; register MCP processes

**Files:**
- Modify: `src/mcp.rs`, `src/activity.rs` (summary helper)
- Test: `tests/mcp.rs`

**Interfaces:**
- Consumes: Task 1.
- Produces: `pub fn summarize_json(command: &str, args: &serde_json::Map<String, Value>) -> (Option<String>, Vec<String>)` in `activity.rs` — target from the first present of `task_id`, `question_id`, `id`, `name` (numbers as `#<n>` for `question_id`); changed = every other argument key, with `name: value` only for the enumerated keys in Global Constraints; `new_name` is recorded as plain `name` (the new name is not attached). `pub fn is_write(command: &str) -> bool` — false for the MCP reads `session_start`, `prompt start`, `task_next`, `task_get`, `question_list`, `track_list`; the CLI reads `start`, `task list`, `task next`, `task show`, `track list`, `question list`, `role list`; and every REST command starting with `GET `; true otherwise.

- [ ] **Step 1: Failing tests in `tests/mcp.rs`** (drive the real binary as the file already does; read the log with `airlock::activity::ActivityLog` at `<hub>/projects/<id>/activity.db`, or via `airlock activity --json` once Task 7 exists — use the library here):

```rust
#[test] fn every_tool_call_is_logged_with_session_role_and_result()
    // lead session: task_claim ok, task_claim of a done task fails → two entries,
    // channel "mcp", session "lead-1", role Some("lead"), second ok=false with the store's error text
#[test] fn clean_room_hidden_tool_is_logged_as_failure()
    // engine (clean-room) calls task_create → entry command "task_create", ok=false, error contains "unknown tool"
#[test] fn session_start_and_prompt_are_logged_as_briefings()
    // session_start → command "session_start"; prompts/get start → command "prompt start"; neither is a write
#[test] fn mcp_log_holds_no_free_text()
    // task_create with do = "SECRET-DO-TEXT" and question_answer with answer "SECRET-ANSWER"; the activity.db file bytes contain neither string
#[test] fn mcp_process_registers_while_running()
    // while the server runs, ActivityLog::processes(now) has one row kind "mcp" with the session; after stdin closes, the row is gone
#[test] fn agent_is_taken_from_client_info()
    // initialize with clientInfo.name "claude-code" → later entries carry agent Some("claude-code")
```

- [ ] **Step 2: Run** `cargo test --test mcp` — Expected: the six new tests FAIL.

- [ ] **Step 3: Implement.** In `Server`: store `agent: Option<String>` from `initialize`'s `clientInfo.name`; in `tools_call`, after computing the result (including the "unknown tool" and "arguments must be an object" paths), build an `Entry` (`channel: Mcp`, `role`/`clean_room` from the server, `pid: std::process::id()`, `build: activity::build()`) and call `record_quietly(&self.project.database, &entry)`; `ok` is false for a JSON-RPC error and for `isError: true`, with the error text. `prompts_get` for `start` records `prompt start`. `run` registers the process (`kind "mcp"`) before the loop, spawns a thread that calls `touch` every `TOUCH_SECS` (its own `ActivityLog` connection; failures ignored), and unregisters after the loop ends. Registration failure prints the quiet warning and the server runs on.

- [ ] **Step 4: Run** `cargo test --test mcp` — Expected: PASS.

- [ ] **Step 5: Commit** `feat: log MCP calls and register MCP servers`

---

### Task 3: Record CLI calls; register `airlock serve`

**Files:**
- Modify: `src/app.rs`, `src/activity.rs`, `src/web.rs` (registration only)
- Test: `tests/e2e.rs` (or a new `tests/activity_cli.rs` driving the binary)

**Interfaces:**
- Consumes: Tasks 1–2 (`Entry`, `record_quietly`, `is_write`, `agent_from_env`, `register`/`touch`/`unregister`).
- Produces: `pub fn summarize_cli(command: &airlock::cli::Command) -> (Option<String>, Vec<String>)` in `activity.rs` — target = task id / question `#<id>` / track name / role name of the command; changed = the flags given (by long name, without `--`), enumerated ones as `name: value`.

- [ ] **Step 1: Failing tests** (real binary, `AIRLOCK_HOME` temp hub, as in `tests/mcp.rs`):

```rust
#[test] fn cli_commands_are_logged_with_binding_session_and_role()
    // `use engine --session engine-1` in a worktree, then `task list` and `task add …` there →
    // entries "task list" (write=false) and "task add" (write=true), session "engine-1", role Some("engine"), clean_room true, channel "cli"
#[test] fn cli_failure_is_logged_with_exact_error()
    // `task claim E-99` → ok=false, error "no task `E-99`"
#[test] fn agent_shell_is_recorded()
    // run with AI_AGENT=claude-code_x → agent Some("claude-code"); without the variable (env_remove AI_AGENT and CLAUDECODE) → None
#[test] fn cli_log_holds_no_free_text()
    // task add --do "SECRET-DO-TEXT" --verify "SECRET-VERIFY" → activity.db bytes contain neither
#[test] fn unwritable_activity_log_never_fails_a_command()
    // replace activity.db with a directory; `task list` exits 0 and stderr contains "airlock: activity log:"
#[test] fn activity_command_is_not_logged()  // after Task 7 exists this asserts `airlock activity` adds no entry; until then mark #[ignore] with a note and un-ignore in Task 7
```

- [ ] **Step 2: Run** the new tests — Expected: FAIL.

- [ ] **Step 3: Implement.** In `app::run`, after the project is resolved and the session is known, and for every command except `init`, `doctor`, `mcp`, `serve`, `setup` and `activity`: take `command.name()`, `summarize_cli`, the binding's role (or the `--role` of `start` / `task next`), `clean_room` from the config, `agent_from_env(|k| std::env::var(k).ok())`; run the command; record `ok`/`error` (`format!("{e:#}")`) with `record_quietly`; return the command's result unchanged. `airlock serve` registers `kind "serve"`, touches every `TOUCH_SECS` on a tokio interval, unregisters on shutdown.

- [ ] **Step 4: Run** `cargo test` — Expected: PASS.

- [ ] **Step 5: Commit** `feat: log CLI calls and register the dashboard server`

---

### Task 4: Record REST calls

**Files:**
- Modify: `src/web.rs`, `src/web/app.js` (header only)
- Test: `tests/web.rs`

**Interfaces:**
- Consumes: Tasks 1–2 (`summarize_json`, `is_write`).
- Produces: header name `X-Airlock-Client`, value `dashboard`. REST command names: `GET /api/tasks`, `POST /api/tasks`, `PATCH /api/tasks/{id}`, `POST /api/tasks/{id}/release`, … — method plus the route pattern.

- [ ] **Step 1: Failing tests in `tests/web.rs`:**

```rust
#[tokio::test] async fn rest_writes_are_logged_as_rest_session()
    // PATCH /api/tasks/{id} {"priority":"high"} without the header → entry session "rest", role None, channel "rest",
    // command "PATCH /api/tasks/{id}", target the id, changed ["priority: high"]
#[tokio::test] async fn dashboard_reads_are_not_logged_but_its_writes_are()
    // GET /api/tasks with X-Airlock-Client: dashboard → no entry; POST …/release with the header → entry session "dashboard"
#[tokio::test] async fn rest_failure_is_logged_with_error_message()
    // PATCH unknown track → 409, entry ok=false, error = the ApiError message
#[tokio::test] async fn events_stream_is_never_logged()
```

- [ ] **Step 2: Run** `cargo test --test web` — Expected: FAIL.

- [ ] **Step 3: Implement** a second middleware layer inside `guard` (rejected requests stay unlogged): skip `/api/events`, `/api/activity*`, non-`/api/` paths, and GETs carrying `X-Airlock-Client: dashboard`; buffer the request body (limit 1 MiB) to take top-level JSON keys for `summarize_json`; take the matched route (`axum::extract::MatchedPath`) for the command; on a non-2xx response buffer the body and take its `error` field. Session `dashboard` with the header, else `rest`; role None; `clean_room` false. In `app.js`, `api()` and the SSE fetch send `X-Airlock-Client: dashboard`.

- [ ] **Step 4: Run** `cargo test --test web` — Expected: PASS.

- [ ] **Step 5: Commit** `feat: log REST calls`

---

### Task 5: Answer corrections

**Files:**
- Modify: `src/store.rs` (SCHEMA_V5, `answer`, question loading), `src/core.rs` (`Question`), `src/snapshot.rs`, `src/views.rs`, `src/mcp.rs`, `src/app.rs`, `src/web.rs` (only the `changed: correction` mark), `src/web/app.js` (Questions inbox marker)
- Test: `tests/store.rs`, `tests/snapshot.rs`, `tests/views.rs`

**Interfaces:**
- Produces:
  - `pub struct AnswerVersion { pub answer: String, pub answered_by: Option<String>, pub answered_at: Option<i64>, pub spec_file: Option<String> }` in `core.rs`
  - `Question` gains `pub corrected_at: Option<i64>`, `pub corrected_by: Option<String>`, `pub history: Vec<AnswerVersion>` (oldest first) — all `#[serde(default, skip_serializing_if = …)]` so v2 snapshots and older JSON still load
  - `Store::answer` unchanged in signature; on a question with an answer (status `answered` or `resolved`) it moves the current answer into `history`, sets the new one, sets `corrected_at`/`corrected_by`, keeps the status
  - `pub fn is_correction(store: &Store, id: u64) -> bool` in `activity.rs` (question exists and has an answer) — the three channel wrappers call it *before* an answer call and add `correction` to `changed`
  - snapshot `FORMAT_VERSION = 3`, `SUPPORTED_VERSIONS = [1, 2, 3]`

- [ ] **Step 1: Failing tests:**

```rust
// tests/store.rs
#[test] fn correcting_a_resolved_answer_keeps_status_and_history()
    // ask, answer "first", resolve, answer "second" by "lead-1" → status Resolved, answer "second",
    // corrected_by Some("lead-1"), history == [AnswerVersion { answer: "first", .. }]
#[test] fn answering_an_open_question_is_not_a_correction()  // corrected_at None, history empty
#[test] fn two_corrections_keep_both_earlier_answers()        // history len 2, oldest first
// tests/snapshot.rs
#[test] fn snapshot_round_trips_answer_history()               // export → import → identical questions
#[test] fn version_two_snapshot_imports_without_history()
// tests/views.rs
#[test] fn questions_md_marks_corrections_and_lists_earlier_answers()
    // contains "corrected" with the date and "Earlier answer" with the first text
```

and in `tests/mcp.rs`: `question_answer_on_resolved_question_corrects_it` (no error; entry `changed` contains `"correction"`).

- [ ] **Step 2: Run** `cargo test` — Expected: the new tests FAIL.

- [ ] **Step 3: Implement.** SCHEMA_V5: `ALTER TABLE questions ADD COLUMN corrected_at INTEGER; ALTER TABLE questions ADD COLUMN corrected_by TEXT; CREATE TABLE answer_history (question_id INTEGER NOT NULL REFERENCES questions(id), seq INTEGER NOT NULL, answer TEXT NOT NULL, answered_by TEXT, answered_at INTEGER, spec_file TEXT, PRIMARY KEY (question_id, seq)) STRICT;`. Import (`replace_all`) writes history rows. `QUESTIONS.md`: after the answer line, `- **Corrected** (<by>, <YYYY-MM-DD>)` and one `- **Earlier answer** (<by>): …` per history entry, newest earlier first. Remove the "already resolved" refusal from `answer` only; `resolve` keeps its rules. Dashboard Questions inbox shows a "corrected" chip when `corrected_at` is set.

- [ ] **Step 4: Run** `cargo test` — Expected: PASS.

- [ ] **Step 5: Commit** `feat: correct answers after resolving, with history`

---

### Task 6: Problems and the "Answer changed" briefing

**Files:**
- Create: `src/problems.rs`
- Modify: `src/lib.rs`, `src/briefing.rs`, `src/mcp.rs`, `src/app.rs`
- Test: `tests/problems.rs`, `tests/onboarding.rs` (briefing)

**Interfaces:**
- Consumes: Tasks 1, 5.
- Produces:
  - `#[serde(rename_all = "snake_case")] pub enum ProblemKind { Failure, CleanRoomCli, ErrorBurst, AnswerCorrected, OldProcess }`
  - `pub struct Problem { pub kind: ProblemKind, pub session: Option<String>, pub role: Option<String>, pub command: Option<String>, pub target: Option<String>, pub message: String, pub count: usize, pub first_at: i64, pub last_at: i64, pub entry_ids: Vec<u64> }`
  - `pub const CLEAN_ROOM_FLAGGED: &[&str] = &["task add", "task edit", "task approve", "task reject", "track add", "track edit", "track remove", "question answer", "question resolve", "sync", "import", "role set", "role remove", "use"];`
  - `pub const BURST_COUNT: usize = 5; pub const BURST_WINDOW_SECS: i64 = 600;`
  - `pub fn problems(entries: &[Entry], processes: &[Process], own_build: &str) -> Vec<Problem>` — sorted by `last_at` descending
  - `briefing::build(store, project, session, role, previous_briefing: Option<i64>, now)` — new parameter; `Briefing` gains `pub corrected_answers: Vec<Question>`

- [ ] **Step 1: Failing tests:**

```rust
// tests/problems.rs (pure, entries built in code)
#[test] fn failures_collapse_per_session_and_command()   // 3 failed "task claim" by engine-1 → one Failure, count 3, message = latest error
#[test] fn clean_room_cli_write_is_flagged_with_or_without_agent()
    // cli "task add" clean_room=true agent None → CleanRoomCli; same with clean_room=false → nothing; "task claim" clean_room=true → nothing; mcp "task_create" → not this kind
#[test] fn five_failures_in_ten_minutes_are_a_burst()    // 5 within 600 s → one ErrorBurst count 5; 5 spread over 700 s → no burst
#[test] fn successful_correction_is_a_problem_failed_one_is_not()
#[test] fn older_live_process_is_an_old_process()        // processes with builds "+100" and "+200", own "+200" → one OldProcess for the +100 one; own "+300" → both
// tests/onboarding.rs
#[test] fn briefing_shows_answer_changed_until_the_next_briefing()
    // engine asks; lead answers + resolves; engine session_start; lead corrects;
    // engine session_start → markdown contains "Answer changed" and the new answer;
    // engine session_start again → no "Answer changed"
#[test] fn corrected_answer_still_unresolved_is_marked_not_repeated()  // appears once, under answered questions, with "(corrected)"
```

- [ ] **Step 2: Run** `cargo test --test problems --test onboarding` — Expected: FAIL.

- [ ] **Step 3: Implement.** Grouping keys: Failure by (session, command); CleanRoomCli by session, message `"<session> (clean-room) writes through the CLI: <distinct commands, comma-separated>"`; ErrorBurst per maximal cluster of one session's failures where some `BURST_COUNT` of them fit in `BURST_WINDOW_SECS`; AnswerCorrected one per (session, target) from ok entries whose `changed` contains `correction`, message `"answer to question <target> corrected"`; OldProcess for each process with `build_time(build) < max(build_time of all processes, build_time(own_build))`, message `"old airlock process: PID <pid>, <session or kind>, started <YYYY-MM-DD HH:MM>, build <build>"`. Briefing: callers pass `ActivityLog::last_briefing(session)` read *before* the current briefing is recorded (log errors → `None`); `corrected_answers` = questions asked by the session, status Resolved, `corrected_at > previous_briefing` (all corrections when `None`); markdown heading `## Answer changed` listing `#<id> (<topic>)` and the new answer; answered-but-unresolved corrected questions keep appearing under the existing answered heading with ` (corrected)` after the id.

- [ ] **Step 4: Run** `cargo test` — Expected: PASS.

- [ ] **Step 5: Commit** `feat: problems from the activity log; answer changed in the briefing`

---

### Task 7: `airlock activity`, REST endpoints, live event

**Files:**
- Modify: `src/cli.rs`, `src/app.rs`, `src/web.rs`
- Test: `tests/cli.rs`, `tests/web.rs`, plus un-ignore Task 3's `activity_command_is_not_logged`

**Interfaces:**
- Consumes: Tasks 1, 6.
- Produces:
  - CLI: `airlock activity [--session <s>] [--role <r>] [--channel mcp|cli|rest] [--since <30m|2h|1d|unix seconds>] [--problems] [--writes] [--failures] [--limit <n, default 50>] [--json]`. Text row: `HH:MM  <session>  <CHANNEL>  <command> <target> <changed…>  [✖ <error>]`, day headers `── YYYY-MM-DD`. `--problems` prints one line per problem. `--json` prints the `Entry` / `Problem` arrays.
  - REST: `GET /api/activity` (query `session`, `role`, `channel`, `since`, `writes`, `failures`, `limit`; default limit 500) → `Entry[]`; `GET /api/activity/problems` → `Problem[]`; `GET /api/activity/sessions` → `[{ session, role, last_at, calls: { mcp, cli, rest } }]` over the retention window.
  - SSE event `activity` with `{ "id": <latest entry id> }`; the watcher polls `ActivityLog::latest_id` alongside the store digest.

- [ ] **Step 1: Failing tests:** `activity_cli_parses_filters` (tests/cli.rs, parse only: `--since 2h`, `--channel cli`, `--problems`); `activity_json_lists_entries_and_problems` (binary: after a failed claim, `airlock --json activity --failures` has one entry and `--problems` one Failure); `api_activity_filters_and_problems` and `sse_emits_activity_event` (tests/web.rs).

- [ ] **Step 2: Run** — Expected: FAIL.

- [ ] **Step 3: Implement** as specified; `--since` parses `<n>m|h|d` or plain seconds, invalid input errors with the accepted forms.

- [ ] **Step 4: Run** `cargo test` — Expected: PASS.

- [ ] **Step 5: Commit** `feat: activity command, endpoints and live event`

---

### Task 8: Dashboard Activity tab

**Files:**
- Modify: `src/web/app.js`, `src/web/style.css`, `src/web/index.html` (only if markup is needed)
- Test: manual in headless Chrome (no JS runner in the repo); keep logic in small pure functions (`groupByDay(entries)`, `sessionChips(sessions)`, `blockers(tasks)`) and check them in node as for `courseWarnings`

**Interfaces:**
- Consumes: Task 7 endpoints and event; existing task data for State and blockers.

- [ ] **Step 1: Build the tab** after the agreed mockup (`.superpowers/brainstorm/520217-1793526864/content/activity-combined.html`): tab "Activity" placed after Tracks; top strip with **Problems** (left: icon ✖ for failure/error_burst, ⚠ for the others, message, `· <count>×` when > 1, time) and **State** (right: claimed tasks with session, track, lease minutes left; **Blockers**: blocked tasks grouped by their unfinished prerequisites, each with the prerequisite's role and holder or "nobody on it"); **session bar** of chips (session, role, MCP/CLI/REST counts, "vor N min"; "all" chip; click filters); **logbook** grouped by day ("Today", "Yesterday", then dates), rows `time · session · channel badge · command target changed`, failed rows red with the error, problem rows highlighted; filter pills `all · writes only · failures & problems · MCP · CLI · REST`. Clicking a problem filters the logbook to its `entry_ids`. Follows the role filter (entries/problems without role hidden while a role is selected). Live: refetch on `activity` events. Channel colours: MCP green, CLI orange, REST violet (SPECTRUM tokens).

- [ ] **Step 2: Verify** with `airlock serve` on a scratch hub seeded through CLI and MCP calls (including a failure, a clean-room CLI `task add`, and a correction) and headless Chrome screenshots at desktop and 390 px width; node check of the pure helpers.

- [ ] **Step 3: Commit** `feat(dashboard): Activity tab`

---

### Task 9: Documentation

**Files:**
- Modify: `docs/spec/01_architecture_and_data_model.md`, `02_cli_interface.md`, `03_mcp_protocol.md`, `04_web_dashboard_and_api.md`, `05_git_sync_and_airlock.md`, `06_onboarding.md`, `README.md`, `docs/walkthroughs/agent-validation.md`

- [ ] **Step 1:** Spec 01: activity log (fields, no-free-text rule, retention, separate `activity.db`), process registry and build identity, answer history and correction rules, problem kinds. Spec 02: `airlock activity`, correction via `question answer`, recognised agent variables (`AI_AGENT`, `CLAUDECODE`). Spec 03: logging of tool calls and prompts, `question_answer` corrects answered/resolved questions, `clientInfo.name` as agent. Spec 04: Activity tab, `/api/activity*`, `X-Airlock-Client`, SSE `activity`, corrected chip in Questions. Spec 05: snapshot format 3 with answer history, `QUESTIONS.md` correction lines. Spec 06: briefing section "Answer changed". README: one paragraph on the Activity tab. agent-validation.md: later runs take their evidence from `airlock activity`.

- [ ] **Step 2: Run** `cargo test` (doc tests and spec hygiene, if any) — Expected: PASS.

- [ ] **Step 3: Commit** `docs: activity log and answer corrections in specs and README`
