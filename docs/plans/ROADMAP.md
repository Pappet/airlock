# Airlock Roadmap

## Vision
A dependable coordination tool for disciplined software development with AI agents and human engineers working side by side.

## Milestones

### Milestone 1: Local Task CLI (v0.1.0)
- Single-binary CLI for managing work packages and questions.
- One shared store for all worktrees of a project on a machine — no merge conflicts over plan files.
- Canonical JSON snapshot and generated Markdown views as replacement for hand-edited plans.

### Milestone 2: Agent Protocol & MCP (v0.2.0)
- Native MCP server with role and session binding.
- Compact work package retrieval (`task_next`).
- Question routing between implementation and specification sessions; restricted clean-room tool set.

### Milestone 3: Web Dashboard (v0.3.0)
- Embedded, loopback-only web UI with token auth.
- Kanban board, dependency view, review queue, live lease monitor.

### Milestone 4: Multi-Machine & Team Sync (v0.4.0)
- Design and implement state synchronization between hubs on different machines (v0.x supports one machine per project state; see Spec 05 §2.4). Requires its own spec: identity, conflict model for concurrent edits, transport.
- Plugin hooks for CI/CD status reporting.
