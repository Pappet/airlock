# Phase Plan: Airlock Development

Phased implementation plan. Each phase ends with `cargo test`, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --check` passing.

---

## Phase 1: Hub Store & CLI (Done)
- [x] Project setup (Rust edition 2024, MSRV 1.85, locked dependencies).
- [x] Domain model: `Task` with persisted `TaskState` and derived `TaskStatus`, `Question`, `ProjectConfig`.
- [x] Dependency graph validation (`check_dependencies`) and derived dependents.
- [x] CLI argument surface (`src/cli.rs`); commands of later phases exit with an error.
- [x] Specifications (`docs/spec/01`–`05`).
- [x] Hub location (`AIRLOCK_HOME` / platform state dir), project registry, `.airlock` marker discovery, `--project` resolution.
- [x] SQLite persistence (`rusqlite`, WAL, busy timeout, `BEGIN IMMEDIATE`, conditional transitions, lazy lease expiry).
- [x] `airlock init` including adoption of an existing marker.
- [x] Task commands: `list`, `next`, `show`, `add`, `claim`, `heartbeat`, `release`, `done`, `approve`, `reject`.
- [x] Question commands: `ask`, `list`, `answer`, `resolve`.
- [x] `--json` output.
- [x] Integration tests against a temporary `AIRLOCK_HOME`, including concurrent claims from multiple processes.

---

## Phase 2: Sync & Import
- [x] Canonical snapshot model and serializer with golden-file test (`src/snapshot.rs`).
- [x] `airlock sync`: snapshot plus `PLAN.md` / `QUESTIONS.md` views, foreign-change guard (Spec 05 §3.5).
- [x] `airlock import` with validation and confirmation.
- [x] Legacy `PLAN.md` migration — done manually (OpenSHR tasks entered one by one over MCP); a dedicated `airlock import-plan` command remains optional future work (one-time, not a round-trip path).

---

## Phase 3: MCP Server
- [x] Stdio MCP server with project/session/role binding.
- [x] Tools per Spec 03, including the restricted clean-room tool set.
- [ ] Agent testing with Claude Code and Antigravity — moved to Phase 3b as the validation of the onboarding flow.

---

## Phase 3b: Onboarding (Current, Spec 06)
- [x] Worktree bindings in the hub, `airlock use` with lock, binding-derived session and role for CLI and MCP.
- [x] Global `-C <dir>`.
- [x] Role management (`airlock role`), role instructions and reading lists.
- [x] Session briefing: `airlock start`, MCP tool `session_start`, MCP prompt `start`.
- [x] `airlock doctor` and `airlock setup claude|antigravity|dsh`.
- [x] Guided `airlock init` with templates, worktree creation, and a committed `airlock.toml` layout.
- [ ] Validation with real agents (Claude Code, Antigravity, DSH) on OpenSHR — in progress, recorded in [`docs/walkthroughs/agent-validation.md`](../walkthroughs/agent-validation.md).

---

## Phase 3c: Session Continuity
Goal: a Claude Code session that runs out of context, crashes, or is compacted is replaced without manual re-initialization. Work orders with Read/Touch/Do/Verify in [`SESSION_CONTINUITY.md`](SESSION_CONTINUITY.md).
- [ ] WO-1: `airlock setup claude-hooks` prints a SessionStart hook that runs `airlock start`; `doctor` checks it.
- [x] WO-2: progress notes on a lease (`task heartbeat --notes`, MCP `task_heartbeat.notes`), shown in the briefing, kept across release and expiry, not exported.
- WO-3 (session instance presence): dropped; shared session ids are intended, and the activity log's process registry shows live instances.
- [ ] WO-4: restart validation — e2e and MCP tests for a same-session restart; Scenario D (and B) run for real on OpenSHR and recorded.
- [ ] WO-5: directory entries in `touch` lists count as overlap for the files beneath them.
- [ ] WO-6: documentation housekeeping (`PROJECT_OVERVIEW.md` dependencies and status, tool counts, phase markers).

---

## Phase 4: Web Dashboard
- [x] `axum` server on loopback with token auth, Host/Origin checks (Spec 04 §2).
- [x] Embedded single-page dashboard (assets via `include_str!`): Kanban, review queue, questions inbox, live leases.
- [x] Overlap analysis over `touch` lists, Stages view and File matrix (Spec 04 §3.5–3.7; design: [`docs/superpowers/specs/2026-10-29-dashboard-design.md`](../superpowers/specs/2026-10-29-dashboard-design.md)).
- [x] Server-Sent Events driven by `PRAGMA data_version` polling.
- [x] Task editing: `airlock task edit`, MCP `task_update`, `PATCH /api/tasks/{id}` with whole-graph validation; tasks past `open` only with `--force`.
- [x] Overlap warning at claim time: `task next` (CLI, MCP, briefing) hands out tasks sharing files with occupying tasks last, with a warning; `task claim` never refuses but warns and records an `overlap:<id>` question for the lead. Reuses `overlap::analyze`.
- [x] Tracks and a fixed priority scale (`critical`/`high`/`normal`/`low`): `track` commands and MCP tools, `--track` on tasks, store schema v4 (integer priorities converted), snapshot format 2, `task next` by track priority, Tracks tab with per-role course warning, board swimlanes and filters, `/api/tracks` and `track_changed` (Spec 01 §2.5, Spec 04 §3.9; design: [`docs/superpowers/specs/2026-10-31-tracks-and-priority-design.md`](../superpowers/specs/2026-10-31-tracks-and-priority-design.md)).
- [x] Assigned task ids: `task add` / `task_create` / `POST /api/tasks` take no id; Airlock numbers tasks `<prefix>-<n>` per role (`role set --prefix`, default: first letter), inside the write transaction. Prevents agents from picking ids that already exist.

---

## Phase 5: Polish & Packaging
- [ ] Global installation (`cargo install --path .`) and release builds.
- [x] Role templates for `airlock init --template` — done in Phase 3b (`single`, `clean-room`, `parallel`); more templates as needed.
- [ ] User documentation and walkthroughs.
