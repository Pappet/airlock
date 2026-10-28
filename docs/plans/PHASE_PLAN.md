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

## Phase 4: Web Dashboard
- [ ] `axum` server on loopback with token auth, Host/Origin checks (Spec 04 §2).
- [ ] Embedded single-page dashboard (assets via `rust-embed`): Kanban, review queue, questions inbox, live leases.
- [ ] Server-Sent Events driven by `PRAGMA data_version` polling.

---

## Phase 5: Polish & Packaging
- [ ] Global installation (`cargo install --path .`) and release builds.
- [x] Role templates for `airlock init --template` — done in Phase 3b (`single`, `clean-room`, `parallel`); more templates as needed.
- [ ] User documentation and walkthroughs.
