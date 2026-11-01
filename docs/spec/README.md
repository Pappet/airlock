# Airlock Specifications

Behavioral and architectural specifications for **Airlock**. Where a spec and the code disagree, the spec describes the intended behavior and the code is the bug — unless the spec is updated in the same change.

## Specification Index

| Document | Topic | Description |
|---|---|---|
| [01_architecture_and_data_model.md](01_architecture_and_data_model.md) | Architecture & Data Model | Entities (tasks, tracks, priority scale, questions and answer corrections), derived status, lifecycles, leases and progress notes, hub store, migration, concurrency rules, activity log, process registry, problems |
| [02_cli_interface.md](02_cli_interface.md) | CLI | Project and session resolution, commands (including `airlock activity`), output and exit codes |
| [03_mcp_protocol.md](03_mcp_protocol.md) | MCP | Server binding, clean-room tool set, tool schemas |
| [04_web_dashboard_and_api.md](04_web_dashboard_and_api.md) | Web UI & REST/SSE API | Local-only security model, Tracks and Activity tabs, Kanban board, review queue, endpoints |
| [05_git_sync_and_airlock.md](05_git_sync_and_airlock.md) | Git Sync & Airlock Boundary | Worktree model, bootstrap, snapshot format, import, merge rule, boundary guarantees and limits |
| [06_onboarding.md](06_onboarding.md) | Onboarding | Worktree bindings, `-C`, session briefing (with the Claude Code SessionStart hook), role management, doctor, harness setup, guided init |
