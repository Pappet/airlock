# Airlock

A local-first coordinator for multi-agent and human development sessions: tasks, questions, and leases in one store shared by every worktree of a project.

> **Status: Phase 3b.** The hub store, the task and question CLI, `sync`/`import`, the MCP server, and onboarding (`use`, `start`, `doctor`, `setup`) work. The dashboard is specified in [`docs/spec/`](docs/spec/) but not built yet; `airlock serve` exits with "not implemented yet". Progress: [`docs/plans/PHASE_PLAN.md`](docs/plans/PHASE_PLAN.md).

---

## Why

Several AI agent sessions and humans working on one project — often in separate git worktrees — usually coordinate through a hand-edited `PLAN.md`. That file conflicts on every merge, forces agents to read the whole plan to find one task, and gives clean-room sessions no structured way to ask questions.

Airlock replaces it with:
- **One shared store per project**, outside the repository, so all worktrees and sessions on a machine see the same state. Plan files in git become generated exports.
- **Compact work packages** (*Read, Touch, Do, Verify*) handed to an agent one at a time instead of the whole plan.
- **Leases**: a claimed task belongs to one session; crashed sessions' leases expire.
- **Derived readiness**: tasks become ready when their prerequisites are done — no manual bookkeeping.
- **A question channel** between implementation and specification sessions, with an audit trail in git.
- **CLI for humans, MCP for agents**, plus a local web dashboard.
- **Guided onboarding**: worktrees bound to roles, a session briefing agents read first, `doctor` and `setup` for harness configuration.

## Features (specified)

- **Hub store:** SQLite (WAL) per project under the platform state directory, referenced from the repository by a one-line `.airlock` marker. Safe concurrent use from many processes via immediate transactions and conditional state transitions.
- **Task lifecycle:** `open → in_progress → (in_review →) done`, with claim, heartbeat, release, approve, and reject; `blocked`/`ready` derived from `blocked_by`.
- **Questions:** `open → answered → resolved`, optionally linked to the spec file that codifies the answer.
- **Deterministic export:** `airlock sync` writes a byte-stable JSON snapshot (`docs/.airlock-state.json`, the only import source) plus `docs/PLAN.md` and `docs/spec/QUESTIONS.md` views. Merge conflicts in generated files are cleared by re-running `airlock sync`; a guard refuses to overwrite snapshots written by another machine.
- **MCP tools:** `task_next`, `task_claim`, `task_heartbeat`, `task_complete`, `question_ask`, … with role binding and a restricted tool set for clean-room roles.
- **Onboarding:** `airlock use <role>` binds a worktree to a role and a stable session; `airlock start` / MCP `session_start` brief an agent on its rules, claims, and next package; `airlock doctor` checks the setup; `airlock setup claude|antigravity|dsh` prints harness configurations.
- **Dashboard:** loopback-only, token-protected Kanban board, review queue, and questions inbox.

Scope of v0.x: one machine per project state. The clean-room features structure and record the exchange between sessions; actual isolation of a clean-room session must come from its environment (separate user, container, or sandbox) — see [Spec 05 §4](docs/spec/05_git_sync_and_airlock.md#4-the-airlock-boundary).

---

## Quickstart

### Prerequisites
- Rust 1.85+ (edition 2024)

### Build
```bash
git clone https://github.com/Pappet/airlock.git
cd airlock
cargo build --release
```

### Workflow
```bash
airlock init                                  # register the project, writes .airlock

airlock task add E-6d "Missiles implementation" --role engine \
  --read docs/spec/missiles.md --touch src/missiles.rs \
  --do "Implement shot clock and volley firing cycle" \
  --verify "cargo test missiles"

export AIRLOCK_SESSION=engine-agent-01
airlock task next --role engine               # show next ready work package
airlock task claim E-6d                       # take it under a lease
airlock task done E-6d --notes "All unit tests passing."

airlock question ask "How does blast radius degrade?" --topic missiles.md
airlock sync                                  # export snapshot and views for git
```

More: [Quickstart walkthrough](docs/walkthroughs/quickstart.md).

---

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## Contribution

1. Develop on a feature branch (`feat/...`, `fix/...`) and merge into `main` via pull request. No direct commits to `main`.
2. Behavioral changes update the affected spec in `docs/spec/` in the same change; architectural changes also update `README.md` and `PROJECT_OVERVIEW.md`.
3. Dependencies are locked in `Cargo.lock`.
4. `cargo test`, `cargo clippy`, and `cargo fmt --check` pass before a PR; CI (`.github/workflows/ci.yml`) enforces this on Linux, macOS, Windows, and the MSRV.

---

## License

[MIT](LICENSE)
