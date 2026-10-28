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

## Phase 2: Sync & Import (Next)
- [x] Canonical snapshot model and serializer with golden-file test (`src/snapshot.rs`).
- [ ] `airlock sync`: snapshot plus `PLAN.md` / `QUESTIONS.md` views, foreign-change guard (Spec 05 §3.5).
- [ ] `airlock import` with validation and confirmation.
- [ ] Legacy `PLAN.md` migration (`airlock import-plan`; one-time, not a round-trip path).

---

## Phase 3: MCP Server
- [ ] Stdio MCP server with project/session/role binding.
- [ ] Tools per Spec 03, including the restricted clean-room tool set.
- [ ] Agent testing with Claude Code and Antigravity.

---

## Phase 4: Web Dashboard
- [ ] `axum` server on loopback with token auth, Host/Origin checks (Spec 04 §2).
- [ ] Embedded single-page dashboard (assets via `rust-embed`): Kanban, review queue, questions inbox, live leases.
- [ ] Server-Sent Events driven by `PRAGMA data_version` polling.

---

## Phase 5: Polish & Packaging
- [ ] Global installation (`cargo install --path .`) and release builds.
- [ ] Role templates for `airlock init --template` (Clean-Room RE, Web App Fullstack, Research & Spec).
- [ ] User documentation and walkthroughs.
