# Airlock

A universal, local-first multi-agent workflow coordinator and task management hub.

---

## High Level Project Overview

Airlock coordinates multi-agent AI and human developer workflows with:
- **Zero git merge conflicts**: Replaces fragile monolithic markdown roadmaps with atomic task stores and lock-free concurrency.
- **Clean-room airlock isolation**: Provides a verified communication channel for asking questions and delivering specifications between isolated development sessions (e.g. reverse engineering vs. clean-room implementations).
- **Dual MCP & CLI interface**: First-class support for both AI agents (via the Model Context Protocol over stdio) and human engineers (via an intuitive command-line interface).
- **Token-efficient work packages**: Delivers sub-200-token self-contained instructions (*Read, Touch, Do, Verify*) directly to coding agents, saving up to 90% of session context tokens.
- **Embedded Web Dashboard**: Embedded zero-dependency Kanban board and session observer.

---

## Features

*Status: scaffold — the data models are implemented; the hub store, sync layer, MCP server, and web dashboard are specified (see `docs/spec/`) but not yet implemented. Progress is tracked in [`docs/plans/PHASE_PLAN.md`](docs/plans/PHASE_PLAN.md).*

- **Concurrent Multi-Worktree Coordination**: Uses local SQLite in Write-Ahead Logging (WAL) mode to permit concurrent read/write operations across distinct git branches and worktrees with zero race conditions.
- **Role-Based Work Routing**: Assign tasks to designated roles (e.g. `lead`, `spec`, `engine`, `frontend`, `qa`).
- **Airlock Question & Answer Queue**: Engine sessions submit questions; lead sessions and human developers answer them without leaking non-compliant code.
- **Session Leases & Auto-Release**: Prevents duplicate work by leasing tasks to active sessions with automatic timeout recovery.
- **Deterministic Documentation Sync**: Exports live database state into a canonical JSON snapshot (`docs/.airlock-state.json`, the lossless backup and import source) plus clean, version-controlled markdown views (`docs/PLAN.md` and `docs/spec/QUESTIONS.md`). Merge conflicts in generated files are resolved by re-running `airlock sync` — never by hand.
- **Native MCP Tools**: Exposes `task_next`, `task_claim`, `task_complete`, `question_ask`, and `question_answer` directly to AI agent platforms (Antigravity, Claude Code, Cursor).

---

## Quickstart Guide

### Prerequisites
- Rust 1.85+ (Edition 2024 compatible)

### Installation
```bash
git clone https://github.com/Pappet/airlock.git
cd airlock
cargo build --release
```

### Basic Workflow
```bash
# 1. Initialize Airlock in your project repository
airlock init

# 2. Add a structured work package
airlock task add E-6d "Missiles implementation" \
  --role engine \
  --read "docs/spec/missiles.md" \
  --touch "src/missiles.rs" \
  --do "Implement shot clock and volley firing cycle" \
  --verify "cargo test missiles"

# 3. In an agent session: fetch next task
airlock task next --role engine

# 4. Claim task
airlock task claim E-6d --session "engine-agent-01"

# 5. Mark task complete
airlock task done E-6d --notes "All unit tests passing."

# 6. Ask a question across the airlock boundary
airlock question ask "How does blast radius degrade?" --topic "missiles.md"
```

For more details, see the [Quickstart Walkthrough](file:///home/peter/Projects/airlock/docs/walkthroughs/quickstart.md).

---

## Contribution

Contributions are welcome! Please follow these project rules:
1. All changes must be developed on a dedicated feature branch (`feat/...`, `fix/...`) and merged via Pull Request into `main`. Direct commits to `main` are strictly prohibited.
2. All architectural and behavioral changes must update `README.md` and `PROJECT_OVERVIEW.md` in the exact same commit.
3. Dependencies must be explicitly version-locked in `Cargo.lock`.
4. Run `cargo test` and `cargo clippy` before submitting a PR.

---

## License

This project is licensed under the [MIT License](file:///home/peter/Projects/airlock/LICENSE).
