# Tracks and a Fixed Priority Scale Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Tasks get a four-level priority and an optional track (a named stream of work with its own priority); `task next` follows track priority; the dashboard shows whether work runs in the right track.

**Architecture:** `Priority` and `Track` are core types. The hub store gets schema v4 (priority values converted in place to ranks, a `tracks` table, `tasks.track` with `ON UPDATE CASCADE`). CLI, MCP, and REST are thin layers over new `Store` track methods; ordering lives in `app::next_ready`. The dashboard (plain ES module) gets priority visuals, swimlanes, filters, and a Tracks tab, computed client-side from `/api/tasks` and `/api/tracks`.

**Tech Stack:** Rust 2024 (MSRV 1.85), rusqlite, clap, axum, serde; dashboard in vanilla JS/CSS (SPECTRUM tokens).

**Spec:** `docs/superpowers/specs/2026-10-31-tracks-and-priority-design.md`

## Global Constraints

- Priority levels exactly `critical`, `high`, `normal`, `low` (lowercase on every JSON surface); default `normal`; order `critical` > `high` > `normal` > `low`.
- No numeric priority is accepted on any surface (CLI, MCP, REST); the error lists the four levels.
- Storage ranks: `low` 0, `normal` 1, `high` 2, `critical` 3.
- Migration of old integers: `>= 30` → `high`, `10..=29` → `normal`, `< 10` → `low`; never `critical`.
- The term is `track` everywhere; `topic` stays for questions only.
- Track names: letters, digits, `-`, `_` (role-name rule), case preserved, unique exactly.
- A task without track counts as a `normal` track for ordering and for the course warning.
- Track writes: every non-clean-room session; clean-room tool set unchanged (no track tools).
- Dashboard: no forms for tracks or tasks; read-only.
- `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` green after every task.

## Review Focus

1. **Renaming a track that tasks use** — every task follows the new name, `task next` order unchanged, old name gone; test in Task 2.
2. **A store opened by the old binary after migration / an unknown future schema** — opening a v4 store with priorities outside 0–3 (hand-edited) must fail loudly, not panic; test in Task 1.
3. **An agent passing a number as priority over MCP (`"priority": 22`)** — tool error naming the four levels, nothing created; test in Task 4.
4. **Importing a v1 snapshot from git after upgrading** — priorities converted with the mapping, no tracks, `sync` afterwards writes v2; test in Task 6.
5. **Course warning with tasks in review and tasks without track** — review does not count as running; no-track counts as `normal`; covered by the pure function test in Task 8.

---

### Task 1: Priority scale replaces integer priority (all layers)

**Files:**
- Modify: `src/core.rs` (new `Priority`; `Task.priority: Priority`)
- Modify: `src/store.rs` (SCHEMA_V4 part 1, row mapping, insert/update, `NewTask`, `TaskEdit`)
- Modify: `src/cli.rs`, `src/app.rs` (`--priority <level>`, `task_table`, `next_ready` order on `Priority`)
- Modify: `src/mcp.rs` (`priority` as string enum in `task_create` / `task_update` schemas and parsing)
- Modify: `src/web.rs` (`CreateTask.priority`, `EditTask.priority`)
- Modify: `src/snapshot.rs` (`TaskRecord.priority: Priority`), `src/views.rs` (prints the name), `src/overlap.rs` test helper
- Test: `tests/model.rs`, `tests/store.rs`, `tests/cli.rs`, `tests/mcp.rs`, `tests/web.rs`, `tests/common/mod.rs`

**Interfaces:**
- Produces: `core::Priority { Critical, High, Normal, Low }` — `Copy, Ord` with `Critical` greatest, serde `rename_all = "lowercase"`, `Default = Normal`, `clap::ValueEnum`; `Priority::rank(self) -> i64` (low 0 … critical 3); `Priority::from_rank(i64) -> Option<Priority>`; `Priority::from_legacy(i64) -> Priority` (migration mapping); `Priority::as_str(self) -> &'static str`; `impl FromStr` with error `"invalid priority `{s}`: use critical, high, normal, or low"`.
- Produces: `Task.priority: Priority`, `NewTask.priority: Priority`, `TaskEdit.priority: Option<Priority>`.

- [ ] **Step 1: Write failing tests**

`tests/model.rs`:
```rust
#[test]
fn priority_orders_and_parses() {
    use airlock::core::Priority::*;
    assert!(Critical > High && High > Normal && Normal > Low);
    assert_eq!(Priority::default(), Normal);
    assert_eq!("high".parse::<Priority>().unwrap(), High);
    let err = "22".parse::<Priority>().unwrap_err().to_string();
    assert!(err.contains("critical, high, normal, or low"), "{err}");
    assert_eq!(serde_json::to_string(&Critical).unwrap(), "\"critical\"");
    for (old, new) in [(40, High), (30, High), (29, Normal), (10, Normal), (9, Low), (0, Low), (-3, Low)] {
        assert_eq!(Priority::from_legacy(old), new, "{old}");
    }
    assert_eq!((0..4).map(|r| Priority::from_rank(r).unwrap().rank()).collect::<Vec<_>>(), [0, 1, 2, 3]);
    assert_eq!(Priority::from_rank(4), None);
}
```
`tests/store.rs`: `version_3_priorities_are_migrated` — build a version-3 database by hand with `rusqlite`: execute a copy of the v1–v3 DDL (`tasks`, `task_deps`, `questions`, `exports`, `bindings`, as in `SCHEMA_V1`–`SCHEMA_V3` of `src/store.rs`), insert tasks with priorities 40, 22, 5, −1, set `PRAGMA user_version = 3`; then `Store::open` and assert `high, normal, low, low`, `track == None`, and that `add_track` works.
`tests/store.rs`: `out_of_range_priority_fails_to_load` — raw `UPDATE tasks SET priority = 9`, then `store.tasks(0)` is `Err`.
`tests/cli.rs`: `task add --priority high` parses to `Priority::High`; `--priority 5` fails to parse.
`tests/mcp.rs`: in `full_tool_set…` add a `task_create` call with `"priority": 22` → tool error contains `critical, high, normal, or low`; the existing create call uses `"priority": "high"`.
`tests/web.rs`: `create_list_show_task` posts `"priority": "high"` and asserts `list[0]["priority"] == "high"`; `create_with_invalid_input_is_400` adds a body with `"priority": 3` → 400.
Update all existing tests that pass integers (`--priority 5`, `"priority": 3`, `priority: -3`, `priority: 0` in `tests/common/mod.rs`) to levels, keeping the ordering they test (e.g. `"5"` vs `"9"` → `normal` vs `high`).

- [ ] **Step 2: Run tests, verify they fail** — `cargo test` fails to compile on `Priority`.

- [ ] **Step 3: Implement**

- `Priority` in `src/core.rs` as specified above.
- `SCHEMA_V4` in `src/store.rs` (appended to `MIGRATIONS`):
```sql
UPDATE tasks SET priority = CASE WHEN priority >= 30 THEN 2 WHEN priority >= 10 THEN 1 ELSE 0 END;
CREATE TABLE tracks (
    name        TEXT PRIMARY KEY,
    priority    INTEGER NOT NULL DEFAULT 1 CHECK (priority BETWEEN 0 AND 3),
    description TEXT NOT NULL DEFAULT '',
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
) STRICT;
ALTER TABLE tasks ADD COLUMN track TEXT REFERENCES tracks (name) ON UPDATE CASCADE;
```
- `task_from_row`: read `priority` via `Priority::from_rank`, mapping `None` to `FromSqlConversionFailure`. Writes use `.rank()`.
- CLI `TaskSpec.priority: Priority` (`#[arg(long, value_enum, default_value_t)]`), `TaskEditSpec.priority: Option<Priority>`; `task_table` prints `as_str()`.
- MCP schemas: `"priority": { "type": "string", "enum": ["critical","high","normal","low"] }`; parse with a helper `opt_priority(args, key) -> Result<Option<Priority>>` that rejects non-strings with the `FromStr` message.
- REST bodies deserialize `Priority` directly (serde error → 400).
- `next_ready` sorts `b.task.priority.cmp(&a.task.priority)` (unchanged shape).
- `views.rs` prints `task.priority.as_str()`.

- [ ] **Step 4: Run** `cargo test && cargo clippy --all-targets -- -D warnings` — all pass.

- [ ] **Step 5: Commit** — `feat: four-level priority scale replaces integer priority`

---

### Task 2: Tracks in the store

**Files:**
- Modify: `src/core.rs` (`Track`, `check_track_name`, `Task.track`)
- Modify: `src/store.rs` (track CRUD, `tasks.track` read/write, `NewTask.track`, `TaskEdit.track`)
- Test: `tests/store.rs`, `tests/model.rs`

**Interfaces:**
- Consumes: `Priority` (Task 1), schema v4 (Task 1).
- Produces:
  - `core::Track { name: String, priority: Priority, description: String, created_at: i64, updated_at: i64 }` (serde).
  - `core::check_track_name(&str) -> Result<(), String>` — error `"invalid track name `{n}`: use letters, digits, `-`, or `_`"`.
  - `Task.track: Option<String>` (`#[serde(default)]`), `NewTask.track: Option<String>`, `TaskEdit.track: Option<Option<String>>` (`Some(None)` clears).
  - `Store::tracks(&self) -> Result<Vec<Track>>` — sorted by priority desc, then name.
  - `Store::track(&self, name) -> Result<Track>` — `NotFound("no track `{name}`")`.
  - `Store::add_track(&mut self, name, priority: Priority, description: &str, now) -> Result<Track>` — fails `"track `{name}` already exists"`.
  - `Store::edit_track(&mut self, name, edit: TrackEdit, now) -> Result<Track>`; `pub struct TrackEdit { pub name: Option<String>, pub priority: Option<Priority>, pub description: Option<String> }`; empty edit → `"nothing to change"`; rename to an existing name fails.
  - `Store::remove_track(&mut self, name, now) -> Result<()>` — refused `"track `{name}` is used by {n} task(s)"`.

- [ ] **Step 1: Write failing tests** (`tests/store.rs`)

```rust
#[test]
fn tracks_are_created_listed_and_validated() {
    // add core (critical), comfort (low), infra (default normal)
    // tracks() order: core, infra, comfort
    // duplicate name → "already exists"; "Core fidelity" → "invalid track name"
}

#[test]
fn tasks_reference_existing_tracks() {
    // new_task with track Some("comfort") → task.track == Some("comfort")
    // track Some("nope") → error containing "no track `nope`"
    // edit_task track Some(None) clears; Some(Some("core")) sets
}

#[test]
fn renaming_a_track_moves_its_tasks() {
    // tasks T-1 (comfort), T-2 (comfort, done), T-3 (no track)
    // edit_track("comfort", name: Some("polish")) → T-1, T-2 have track "polish", T-3 None
    // updated_at of T-1 unchanged; store.track("comfort") is NotFound
    // rename to an existing track name → error
}

#[test]
fn tracks_in_use_cannot_be_removed() {
    // remove_track with a done task using it → "used by 1 task(s)"; after clearing → Ok, gone
}
```
`tests/model.rs`: `check_track_name` accepts `core`, `comfort-2`, `Engine_UI`; rejects `""`, `a b`, `ä`, `a.b`.

- [ ] **Step 2: Run** `cargo test --test store` — fails to compile.

- [ ] **Step 3: Implement** the interfaces above. Unknown-track check on add/edit inside the write transaction with message `"no track `{name}`; defined tracks: a, b"` (or `"…; no tracks defined (airlock track add)"`). Rename relies on `ON UPDATE CASCADE`.

- [ ] **Step 4: Run** `cargo test` — pass.

- [ ] **Step 5: Commit** — `feat: tracks in the hub store`

---

### Task 3: `task next` follows track priority

**Files:**
- Modify: `src/app.rs` (`next_ready`), callers in `src/mcp.rs:236`, `src/briefing.rs:116`, `src/app.rs:369`
- Test: `#[cfg(test)] mod tests` in `src/app.rs` (`next_ready` is `pub(crate)`; the CLI for tracks only arrives in Task 4)

**Interfaces:**
- Consumes: `Store::tracks()` (Task 2).
- Produces: `next_ready<'a>(tasks: &'a [Task], tracks: &[Track], role: &str) -> Option<TaskView<'a>>`; sort key: track priority (missing/unknown track → `Normal`), task priority, `created_at`, `id`; overlap fallback unchanged.

- [ ] **Step 1: Write the failing test** — `next_ready_prefers_the_most_important_track` in `src/app.rs`: tracks `core` critical and `comfort` low (built as `Track` values); engine tasks (all ready, built like `tests/common::task`) `E-1` comfort/high, `E-2` core/low, `E-3` no track/critical, `E-4` track `gone` (unknown)/normal created earlier than `E-3`. Removing each returned task in turn yields `E-2`, `E-3`, `E-4`, `E-1` (no-track and unknown track both rank as `normal`; `E-3` beats `E-4` on task priority).

- [ ] **Step 2: Run** `cargo test --lib next_ready_prefers` — FAIL.

- [ ] **Step 3: Implement** the new signature; callers load `store.tracks()?`.

- [ ] **Step 4: Run** `cargo test` — pass.

- [ ] **Step 5: Commit** — `feat: task next hands out the most important track first`

---

### Task 4: CLI and MCP for tracks and task tracks

**Files:**
- Modify: `src/cli.rs` (`Command::Track(TrackCommand)`; `TaskSpec.track`, `TaskEditSpec.track` / `clear_track`; `TaskCommand::List` gains `track`, `no_track`, `priority`)
- Modify: `src/app.rs` (handlers, `track` table output, `task_table` shows track)
- Modify: `src/mcp.rs` (`track_list`, `track_create`, `track_update`, `track_remove`; `track` on `task_create` / `task_update`; not in the clean-room set)
- Modify: `src/briefing.rs` (work package shows `**Track:** … · **Priority:** …`)
- Test: `tests/cli.rs`, `tests/onboarding.rs` or `tests/e2e.rs`, `tests/mcp.rs`

**Interfaces:**
- Consumes: Store track API (Task 2), `next_ready` (Task 3).
- Produces (CLI): `airlock track list`, `track add <name> [--priority <p>] [--description <d>]`, `track edit <name> [--name <n>] [--priority <p>] [--description <d>]`, `track remove <name>`; `task add … [--track <t>]`; `task edit … [--track <t> | --clear-track]`; `task list [--track <t> | --no-track] [--priority <p>]` (at least `p`).
- Produces (JSON of `track list` / `track_list`): array of `{ name, priority, description, created_at, updated_at, counts: { blocked, ready, in_progress, in_review, done } }` — a shared `app::TrackView` used by MCP and REST (Task 5).
- Produces (MCP): `track_create { name, priority?, description? }`, `track_update { name, new_name?, priority?, description? }`, `track_remove { name }`, `track_list {}`; `task_update` `track: null` clears.

- [ ] **Step 1: Write failing tests**
  - `tests/cli.rs`: parses `track add comfort --priority low --description "Could build"`, `task list --track core --priority high`, `task edit E-1 --clear-track`; `--track x --clear-track` conflicts.
  - `tests/e2e.rs` `tracks_from_the_cli`: add `core` critical + `comfort` low; `task add … --track comfort` → JSON `track == "comfort"`; `track list --json` order and `counts.ready == 1` for comfort; `task list --priority high` filters; `track remove comfort` fails while used; `track edit comfort --name polish` then `task show E-1 --json` has `track == "polish"`; human `track list` contains `critical  core`.
  - `tests/mcp.rs`: full session lists 18 tools (`tool_names(1).len()`); `track_create` then `task_create` with `track` → result `track` set; `task_update` with `"track": null` clears; `track_create` with `"priority": 2` → error naming the levels; clean-room `tools/list` unchanged (no `track_*`).
  - `tests/onboarding.rs` (briefing test): next package text contains `**Track:** core · **Priority:** high`.

- [ ] **Step 2: Run** — FAIL.

- [ ] **Step 3: Implement.** Track names validated with `check_track_name`; descriptions free text.

- [ ] **Step 4: Run** `cargo test` — pass.

- [ ] **Step 5: Commit** — `feat: track commands and task tracks in CLI and MCP`

---

### Task 5: REST and live events

**Files:**
- Modify: `src/web.rs` (`GET /api/tracks`; `TaskFilter` gains `track: Option<String>`, `priority: Option<Priority>`; `CreateTask.track`; `EditTask.track: Option<Option<String>>` with a small hand-written `deserialize_with` so an absent field keeps the track and `null` clears it — no new dependency; `Digest` gains `tracks` and emits `track_changed`)
- Test: `tests/web.rs`

**Interfaces:**
- Consumes: `TrackView` (Task 4), Store track API.
- Produces: `GET /api/tracks` → `TrackView[]`; `/api/tasks?track=<name>`, `?track=` (empty = no track), `?priority=<level>` (at least); SSE `event: track_changed` with `{"id":"<name>"}`.

- [ ] **Step 1: Write failing tests** (`tests/web.rs`)
  - `tracks_endpoint_and_filters`: create tracks via CLI, tasks via POST with `track`; `GET /api/tracks` order and counts; `?track=comfort`, `?track=`, `?priority=high` return the right ids; POST with unknown `track` → 400; PATCH `{"track": null}` clears, PATCH `{}` without fields → 400 as before.
  - extend `events_report_task_and_question_changes`: after `airlock track add core`, read `event: track_changed\ndata: {"id":"core"}`.

- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test` — pass.
- [ ] **Step 5: Commit** — `feat: tracks over REST and live events`

---

### Task 6: Snapshot v2, import of v1, PLAN.md

**Files:**
- Modify: `src/snapshot.rs` (`FORMAT_VERSION = 2`; `Snapshot.tracks: Vec<Track>` sorted by name; `TaskRecord.track: Option<String>`)
- Modify: `src/sync.rs` (`load_snapshot` accepts v1 and v2: v1 parsed into a private `SnapshotV1` with integer priorities, converted with `Priority::from_legacy`, `tracks: []`; `check_import` validates task tracks against `snapshot.tracks` and track names)
- Modify: `src/store.rs` (`replace_all` deletes and inserts tracks; tracks before tasks; tracks carry `created_at`, `updated_at = now`)
- Modify: `src/views.rs` (task line `- **Status:** … · **Track:** core · **Priority:** high`; `**Track:**` omitted when none)
- Modify: the call sites of `Snapshot::new` (add `&tracks`)
- Test: `tests/snapshot.rs`, `tests/views.rs`, `tests/e2e.rs` (sync/import)

**Interfaces:**
- Consumes: `Track`, `Priority`, Store track API.
- Produces: `Snapshot::new(project_id, config, tasks, tracks: &[Track], questions) -> Snapshot`.

- [ ] **Step 1: Write failing tests**
  - `tests/snapshot.rs`: snapshot JSON has `"format_version": 2`, `"tracks": [...]` sorted by name, tasks with `"track"` and `"priority": "high"`; round trip through `replace_all` keeps tracks and task tracks.
  - `tests/snapshot.rs` `version_1_snapshots_import_with_converted_priorities`: hand-written v1 JSON with priorities 40 / 22 / 5 imports as high / normal / low, no tracks; `check_import` rejects a v2 snapshot whose task names an undefined track (`"task `E-1` has track `x`, which the snapshot does not define"`).
  - `tests/views.rs`: PLAN.md line contains `**Track:** core · **Priority:** high`.
  - existing byte-stability test in `tests/e2e.rs` still passes.

- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test` — pass.
- [ ] **Step 5: Commit** — `feat: tracks in the snapshot (format 2) and PLAN.md`

---

### Task 7: Dashboard — priority visuals, swimlanes, filters

**Files:**
- Modify: `src/web/app.js` (state: `tracks`, `groupByTrack` (localStorage key `airlock-group-by-track`, default `true`, try/catch), `trackFilter` (`null` | name | `""` for no track), `minPriority` (`null` | level); `load()` fetches `/api/tracks`; `card()` priority stripe + pips; `boardView()` swimlanes; filter controls; drawer chip + track; review queue honours filters)
- Modify: `src/web/style.css` (priority colour classes `.p-critical` `--error`, `.p-high` `--tertiary`, `.p-normal` `--secondary`, `.p-low` grey; `.pips`; swimlane grid; switch)
- Modify: `src/web/index.html` only if a new header slot is needed for the filters
- Test: `tests/web.rs` (asset smoke checks only)

**Interfaces:**
- Consumes: `GET /api/tracks`, task fields `track`, `priority` (Tasks 4–5).
- Produces (JS, used by Task 8): `PRIORITIES = ["critical","high","normal","low"]`; `priorityRank(level) -> 3..0`; `trackOf(task) -> Track | null`; `trackRank(task)` (no track → normal); `matchesFilters(task)` combining role, track, and min-priority filters; `priorityChip(level)`, `pips(level)` element builders.

- [ ] **Step 1: Write failing test** — `tests/web.rs` `dashboard_assets_know_tracks`: `GET /app.js` body contains `"/api/tracks"` and `airlock-group-by-track`; `GET /style.css` contains `.p-critical`.
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement.**
  - Card: left stripe class `p-<level>`, pips `●●●●`/`●●●○`/`●●○○`/`●○○○` with `title="priority <level>"` replacing `P${priority}`.
  - Board grouped: one lane per track in track order (priority desc, name), then a "no track" lane at the `normal` position; lanes with no visible tasks are omitted; cells sorted by task priority desc, then `created_at`.
  - Board flat: cells sorted by track rank, task priority, `created_at` (the `task next` order).
  - Filters in the section strip next to the role pills: track select (all / each track / no track), min-priority select (all / ≥ low … ≥ critical), switch "group by track".
  - Drawer label: `role · status`, then `priorityChip`, and `track` (or "no track").
- [ ] **Step 4: Verify** — `cargo test`; then run `airlock serve` against a scratch hub with tracks and tasks of each level and check board grouped/flat, filters, and drawer in the browser (Claude in Chrome or manual).
- [ ] **Step 5: Commit** — `feat(dashboard): priority on cards, track swimlanes and filters`

---

### Task 8: Dashboard — Tracks tab with the course warning

**Files:**
- Modify: `src/web/app.js` (tab `["tracks", "Tracks"]` first in `TABS`; `tracksView()`; pure `courseWarnings(tasks, tracks) -> Map<trackKey, Array<{ role, ready }>>`)
- Modify: `src/web/style.css` (row grid, segmented bar, warning marker)
- Test: `tests/web.rs` (asset check). The repo has no JS test runner, so `courseWarnings` stays a pure function and is verified in Step 4 against a prepared scratch hub.

**Interfaces:**
- Consumes: helpers from Task 7.

- [ ] **Step 1: Write failing test** — `tests/web.rs` `dashboard_has_tracks_tab`: `app.js` contains `courseWarnings` and `"tracks", "Tracks"`.
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement.**
  - Rows: tracks in priority order plus "no track" (shown at `normal` if any task lacks a track); respect the role filter.
  - Row content: priority chip, name, description; bar segments done / in review / running (`in_progress`) / ready / blocked with counts; running tasks as `▶ id title · session · N min`; last finished task (max `updated_at` among done).
  - `courseWarnings`: for each role R (only the filtered role when set) and track T: T flagged for R when ready(T,R) ≥ 1, in_progress(T,R) = 0, and some track U with rank(U) < rank(T) has in_progress(U,R) ≥ 1; `in_review` never counts; no-track tasks form the "no track" key at `normal`. Row shows orange marker and `R: nothing running, N ready` per flagged role.
  - Clicking a row sets `trackFilter` to it and switches to the board.
- [ ] **Step 4: Verify** — `cargo test`; scratch hub scenario: `core` critical with ready engine task and nothing running, `comfort` low with an engine task in progress, plus a lead task ready in `core` and a lead task in review in `comfort` → `core` shows exactly `engine: nothing running, 1 ready` (not lead); setting role filter `lead` shows no warning.
- [ ] **Step 5: Commit** — `feat(dashboard): Tracks tab with per-role course warning`

---

### Task 9: Specs, README, quickstart, phase plan

**Files:**
- Modify: `docs/spec/01_architecture_and_data_model.md` (Task struct: `priority: Priority`, `track`; new §2.x Track; selection order §2.1; migration note)
- Modify: `docs/spec/02_cli_interface.md` (`track` commands, `task add/edit/list` options)
- Modify: `docs/spec/03_mcp_protocol.md` (four track tools, `track` and priority enum on task tools, clean-room set unchanged)
- Modify: `docs/spec/04_web_dashboard_and_api.md` (`/api/tracks`, filters, `track_changed`, Tracks tab, board swimlanes/filters, priority visuals)
- Modify: `docs/spec/05_git_sync_and_airlock.md` (snapshot format 2, v1 import)
- Modify: `docs/spec/06_onboarding.md` (briefing shows track/priority)
- Modify: `README.md`, `docs/walkthroughs/quickstart.md` (create a track, `--track`, `--priority high`), `docs/plans/PHASE_PLAN.md` (Phase 4 item)
- Test: `grep -rn "priority" docs/spec README.md docs/walkthroughs` shows no integer priorities left.

- [ ] **Step 1:** Update the documents.
- [ ] **Step 2:** Run the grep; every hit uses a level name.
- [ ] **Step 3: Commit** — `docs: tracks and the priority scale in specs, README, quickstart`

---

### After merge (rollout, not part of the PR)

- `cargo install --locked --path .`
- OpenSHR (`~/Projects/openshr`, `main`): `CLAUDE.md` and `docs/PROCESS.md` say how tracks are created (`airlock track add`), that every new task gets `--track` and a level, and that `task next` follows track priority. Commit there; the engine worktree gets it with its next `git merge main`.
