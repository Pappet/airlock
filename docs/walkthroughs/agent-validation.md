# Agent Validation: Onboarding on OpenSHR

Validation of the onboarding flow (Spec 06) with real agent harnesses on the OpenSHR project: main checkout `~/Projects/openshr` (roles lead and spec) and the clean-room worktree `~/Projects/openshr-engine` (role engine). Each run records what worked, what failed with the exact error, and what changed in Airlock as a result.

Do not copy analysis material (decompiled output, `re/` content) into this file, task notes, or questions.

## Scenarios

| | Scenario | Checks |
|---|---|---|
| A | Connection | server starts; first line of `session_start` shows the right role and session; tool count (9 clean-room, 13 otherwise) |
| B | Work cycle | `session_start` → claim → heartbeat → complete with notes; dependents become ready; `review: true` → approve / reject, feedback in the next briefing |
| C | Questions across the airlock | engine asks, spec answers with `spec_file` and resolves; answer appears in the engine's next briefing |
| D | Restart | agent stopped mid-task and restarted: briefing names the held task and lease; work continues without a new claim |
| E | Parallel sessions | DSH lead, DSH spec, Antigravity at once: no shared sessions, no stolen claims; `airlock doctor` clean |
| F | Guardrails | engine asks for a foreign task or `question_answer`: understandable refusal, sensible reaction |
| G | Export | `doc_sync` writes `PLAN.md` / `QUESTIONS.md`; after merging into `engine`, `airlock sync` succeeds |
| H | Agent behavior | calls `session_start` unprompted; heartbeats; useful notes; asks instead of guessing; understands Airlock errors |

## Status

| Scenario | Claude Code | Antigravity | DSH |
|---|---|---|---|
| A Connection | ⚠ runs 1–2: MCP down (PATH, then local scope across worktrees); fixed with per-worktree `.mcp.json` | ✅ starts in the workspace (no `-C`) | ✅ lead (3080) and spec (3081) via overlays |
| B Work cycle | — | — | — |
| C Questions | — | — | — |
| D Restart | — | — | — |
| E Parallel | — | ⚠ shared session with DSH spec (fixed in config) | ⚠ see Antigravity |
| F Guardrails | — | — | — |
| G Export | — | — | — |
| H Behavior | ✅ runs 1–2: asked before changing bindings; ⚠ run 2: wrong instruction for completing `ready` tasks | — | — |

## Runs

### Run 1 — 2026-10-28, Claude Code, engine worktree

**Observed:** `session_start` requested; the MCP server did not start. The agent fell back to `airlock start` via the CLI and produced an accurate briefing-based status, then asked before changing anything.

**Problems:**
1. `.mcp.json` called `airlock` by name; `~/.cargo/bin` was not on Claude Code's `PATH` (desktop-launched client), so the server never started. `airlock doctor` only checked the shell's `PATH` and did not notice.
2. `.mcp.json` (written from the old PROCESS.md recipe) set `--session engine-1`, while the worktree was bound as `engine@openshr-engine`: CLI and MCP would have acted as two sessions in one worktree.
3. The engine worktree had been bound before `airlock init --template clean-room`, so it stayed unlocked although `airlock.toml` says `lock = true`; init deliberately did not touch existing bindings.
4. Not Airlock: the task store predates engine-side completions made under the old PLAN.md workflow (E-6c3, E-6d follow-ups). Reconciled by the agent verifying and completing them with commit references in the notes.

**Agent behavior:** proposed to fix the binding itself (`airlock use engine --session engine-1 --lock`) but asked first — correct, since the "Coordination (Airlock)" section forbids agents changing the setup.

**Setup fix:** `airlock -C ~/Projects/openshr-engine use engine --session engine-1 --lock`; `.mcp.json` from `airlock setup claude` (absolute path, `args: ["mcp"]`).

**Changes in Airlock (branch `fix/validation-run-1`):**
- `doctor` warns about bare command names and suggests the absolute path.
- `doctor` warns when a per-directory configuration sets a `--session` or `--role` that differs from the worktree's binding, except for another role the layout places in the same worktree.
- `init` adds the layout's lock to an existing binding of a listed role; `doctor` reports bindings that contradict `airlock.toml`.

### Run 1b — 2026-10-28, DSH and Antigravity, main checkout

**Observed:** DSH on 3080 had no Airlock tools. The DSH `/restart` plugin restarts the exact original command line, which predated adding `--patch` to the start script; restarting once through the script fixed it. Antigravity started `airlock mcp` without arguments in `~/Projects/openshr`, inherited the checkout's binding (spec, `spec-1`) and so shared its session with the DSH spec instance.

**Setup fix:** DSH lead via the start script with `--patch ~/.dsh/airlock/openshr-lead.yml`, spec manually on 3081; Antigravity with an explicit `--role` and its own `--session`.

**Lesson:** where several harnesses work in one checkout, each needs its own `--session`; a binding provides only one default session per directory.

### Run 2 — 2026-10-28, Claude Code, main checkout (lead) and engine worktree

**Observed:** a lead session in the main checkout read the new `CLAUDE.md` section and started with the briefing — but through the CLI, which used the checkout's binding (spec, `spec-1`). It did not rebind (correct), asked whether `L-19`, held by `lead@openshr`, was orphaned, and told the engine to finish `E-6c3`/`E-6d` with a bare `airlock task done`.

**Problems:**
1. `airlock start` had no `--role`: in a shared checkout the CLI could only brief the bound role.
2. The briefing did not show other sessions' claims of the same role; `lead@openshr` was the live DSH lead instance, which the agent could not tell from a stale lease.
3. The suggested fix `claude mcp add --scope local … --role lead` (also what `airlock setup claude` recommended) broke the engine worktree: Claude Code keys its local scope by repository, so the lead entry applied in `openshr-engine` too; the locked engine binding refused it (`this worktree is locked to role 'engine'; '--role lead' is refused`) — the lock worked as intended, but the server did not start.
4. `airlock setup claude` mixed explanatory text and JSON on stdout; redirected into `.mcp.json` (as intended for `setup dsh`), it produced invalid JSON. Claude Code then reported conflicting scopes (project and local).
5. Not Airlock: the instruction for the engine was wrong — a `ready` task must be claimed before `task done`, and without `--review` it goes straight to done instead of to the lead.

**Setup fix:** removed the Claude Code local-scope entry; `.mcp.json` per worktree — main checkout with `--role lead --session lead-1`, engine with `args: ["mcp"]` (locked binding `engine-1`).

**Changes in Airlock (branch `fix/validation-run-2`):**
- `airlock setup` writes only the file content to stdout (hints to stderr); `setup claude` produces a per-worktree `.mcp.json` and no longer suggests the local scope.
- `doctor` checks Claude Code local-scope entries against every worktree of the repository, warns about them when there are several worktrees, reports a server defined in both `.mcp.json` and the local scope, and reports configuration files that are not valid JSON.
- `airlock start --role` and `AIRLOCK_ROLE` (also for `task next`); a locked binding refuses a different role.
- The briefing lists tasks of the same role held by other sessions, with session and remaining lease time.

