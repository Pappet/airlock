# Changelog

All notable changes to Airlock are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

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

[Unreleased]: https://github.com/Pappet/airlock/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Pappet/airlock/releases/tag/v0.1.0
