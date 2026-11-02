# Changelog

All notable changes to Airlock are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- User guide (`docs/user-guide/`: where to start, concepts, troubleshooting)
  and walkthroughs for a clean-room project with Claude Code and for several
  agents on one codebase; the quickstart covers 0.2.

### Changed
- `airlock mcp` reads its role from `AIRLOCK_ROLE` like `start`, `inbox`, and
  the Claude Code hooks, so a harness started with `AIRLOCK_SESSION` and
  `AIRLOCK_ROLE` acts as one identity in a shared checkout.

## [0.2.0] - 2026-11-02

Handoff between sessions (Phase 3d): sessions learn what changed for their
role at the boundary between two tasks and when their turn ends.

### Upgrading
- The store migrates to schema 8 on first use; older binaries then refuse
  it. Restart every running session (`airlock mcp`, `airlock serve`) after
  installing.
- Re-run `airlock setup claude-hooks` wherever Claude Code hooks were
  installed and merge its `hooks` key: hooks now name the worktree with
  `$CLAUDE_PROJECT_DIR`. `airlock doctor` reports hooks that still carry a
  fixed path (an error in a clean-room worktree).

### Added
- Events written in the same transaction as each change, and a per-session
  inbox: `airlock inbox [--peek]`, MCP `inbox` (store schema 7).
- `task_complete` / `task done` return the inbox and the next package;
  `task_claim` warns about a more important ready task and a second lease.
- `airlock setup claude-hooks --auto-continue`: an opt-in Claude Code Stop
  hook that sends a session back to work when its inbox has news, at most
  `auto_continue_limit` times an hour (default 10). It reads the session and
  role from `AIRLOCK_SESSION` / `AIRLOCK_ROLE` before the binding, so it
  also works for a second role in a shared checkout.
- `task hold` / `task resume` (MCP `task_hold` / `task_resume`) for tasks
  that wait for something outside Airlock; held tasks are never handed out.
  The lease holder can hold its own claimed task in one step.
- `task_done` news with the completion notes for the coordinating roles.
- `inbox_wait` (MCP) and `airlock inbox --wait` for sessions that run
  unattended.

### Changed
- A session's inbox skips the changes it made itself (store schema 8).

### Fixed
- `setup claude-hooks` names the worktree with `-C "$CLAUDE_PROJECT_DIR"`
  instead of a path. Claude Code also runs the main checkout's local hooks in
  sessions of linked worktrees, so a clean-room engine session received the
  main checkout's briefing and would have been nudged for its inbox.
  `airlock doctor` reports such inherited hooks (from the main checkout's
  `settings.local.json` and `settings.json`); re-run `setup claude-hooks`
  where hooks were installed before.
- The Markdown briefing aims to stay under 8 KB, so Claude Code's
  SessionStart hook puts it into context instead of a file: when it would be
  larger, new and corrected answers are listed by reference with the command
  to read them (the work package, rules and workflow are never shortened).

## [0.1.0] - 2026-11-01

First release: the hub store, the CLI, the MCP server, onboarding, the
dashboard, the activity log, and session continuity.

### Hub store and CLI
- One SQLite (WAL) store per project under the platform state directory,
  found from the repository by a `.airlock` marker; safe for many processes
  at once (immediate transactions, conditional state transitions).
- Tasks as work packages (*Read, Touch, Do, Verify*) with the lifecycle
  `open → in_progress → (in_review →) done`; claim, heartbeat, release,
  approve, reject; `blocked`/`ready` derived from `blocked_by`.
- Leases with expiry; progress notes on a lease (`task heartbeat --notes`)
  that the next session reads in its briefing.
- Task ids assigned per role (`E-7`, `S-2`), listed in numeric order.
- Tracks with a fixed priority scale (`critical`, `high`, `normal`, `low`);
  `task next` orders by track priority, task priority, then age, and hands
  out tasks that share files with running work last.
- Questions `open → answered → resolved`, linked to the spec file that
  codifies the answer; answers can be corrected, earlier versions are kept.
- `airlock sync` / `airlock import`: a byte-stable snapshot
  (`docs/.airlock-state.json`) plus generated `docs/PLAN.md` and
  `docs/spec/QUESTIONS.md`; a guard refuses to overwrite snapshots written
  by another machine.

### MCP server
- `airlock mcp` over stdio with 18 tools, bound to a role; clean-room roles
  get a restricted tool set.

### Onboarding and session continuity
- `airlock init --template clean-room|parallel|single`, `airlock use <role>`,
  `airlock start`, `airlock doctor`, and `airlock setup claude|antigravity|dsh`.
- `airlock setup claude-hooks`: a Claude Code SessionStart hook that
  re-delivers the briefing after a restart or `/compact`; briefings stay
  short by summarising answers already shown.

### Dashboard and activity log
- `airlock serve`: a loopback-only, token-protected dashboard with tracks,
  a board with swimlanes per track, dependency stages, a file-overlap
  matrix, a review queue, a questions inbox, a lease monitor, and live
  updates over SSE; a REST API underneath.
- An activity log next to the store that records every MCP, CLI, and REST
  call without free text, pruned after 30 days; `airlock activity` and the
  dashboard's Activity tab show what needs attention.

[Unreleased]: https://github.com/Pappet/airlock/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/Pappet/airlock/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Pappet/airlock/releases/tag/v0.1.0
