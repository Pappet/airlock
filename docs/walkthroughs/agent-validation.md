# Agent Validation: Onboarding on OpenSHR

Validation of the onboarding flow (Spec 06) with real agent harnesses on the OpenSHR project: main checkout `~/Projects/openshr` (roles lead and spec) and the clean-room worktree `~/Projects/openshr-engine` (role engine). Each run records what worked, what failed with the exact error, and what changed in Airlock as a result.

Do not copy analysis material (decompiled output, `re/` content) into this file, task notes, or questions.

## Evidence

Runs 1 to 3 were reconstructed by reading the harnesses' transcripts by hand. Later runs take their evidence from Airlock's own activity log instead (Spec 01 §3.5): `airlock activity --since 2h` lists every call of the run in order, with channel, session, command, target, and the exact error of each refusal; `--session <id>` follows one agent, `--failures` shows the refusals, and `--problems` lists what Airlock itself flagged (clean-room sessions writing through the CLI, bursts of errors, corrected answers, `airlock mcp` processes from before an upgrade). `--json` gives the same for pasting into a run record. The Activity tab of the dashboard shows it live. The log holds no task text or answers, so it can be quoted here, but it covers only what passed through Airlock: what an agent decided or said stays in its transcript.

## Scenarios

| | Scenario | Checks |
|---|---|---|
| A | Connection | server starts; first line of `session_start` shows the right role and session; tool count (11 clean-room, 22 otherwise since Phase 3d) |
| B | Work cycle | `session_start` → claim → heartbeat → complete with notes; dependents become ready; `review: true` → approve / reject, feedback in the next briefing |
| C | Questions across the airlock | engine asks, spec answers with `spec_file` and resolves; answer appears in the engine's next briefing |
| D | Restart | agent stopped mid-task and restarted: briefing names the held task and lease; work continues without a new claim |
| E | Parallel sessions | DSH lead, DSH spec, Antigravity at once: no shared sessions, no stolen claims; `airlock doctor` clean |
| F | Guardrails | engine asks for a foreign task or `question_answer`: understandable refusal, sensible reaction |
| G | Export | `doc_sync` writes `PLAN.md` / `QUESTIONS.md`; after merging into `engine`, `airlock sync` succeeds |
| H | Agent behavior | calls `session_start` unprompted; heartbeats; useful notes; asks instead of guessing; understands Airlock errors |
| I | Handoff between sessions | at a task boundary the engine takes the next package from `task_complete` and gets answers and new tasks in its inbox, without the human copying anything; with auto-continue the Stop hook sends a session that ends its turn with news back to work (Phase 3d) |

## Status

| Scenario | Claude Code | Antigravity | DSH |
|---|---|---|---|
| A Connection | ⚠ runs 1–2: MCP down (PATH, then local scope across worktrees); fixed with per-worktree `.mcp.json` | ✅ starts in the workspace (no `-C`) | ✅ lead (3080) and spec (3081) via overlays |
| B Work cycle | ✅ run 3: claim → heartbeat (lease extended) → complete with commit notes, dependents ready; ✅ run 4: heartbeat with a progress note; ⚠ `review: true` never used | — | — |
| C Questions | ✅ run 3: 16 engine questions answered with `spec_file` and resolved; ⚠ a correction after resolving is refused and was dropped silently | — | — |
| D Restart | ✅ run 3: two lead restarts, briefing named the held task, work went on without a new claim; ✅ run 4: engine killed mid-task and `/compact`, SessionStart hook delivered the held task, lease and progress note, no new claim, no `session_start` after the compaction | — | — |
| E Parallel | ⚠ run 3: no stolen claims, but three Claude sessions in the main checkout shared `lead-1` | ⚠ shared session with DSH spec (fixed in config) | ⚠ see Antigravity |
| F Guardrails | ✅ run 3: claim of a blocked task refused, agent checked prerequisites and moved on; ⚠ engine creates and edits tasks through the CLI | — | — |
| G Export | ✅ run 3: `doc_sync` / `airlock sync` after each plan change, outputs committed | — | — |
| H Behavior | ✅ runs 1–3: asked before changing bindings; `session_start` first in every session; ⚠ run 2: wrong instruction for completing `ready` tasks; ⚠ run 3: works around refusals instead of reporting them; ⚠ run 4: calls `session_start` again although the hook delivered the briefing, and did not refresh its progress note before `/compact` | — | — |
| I Handoff | ✅ run 5: task boundary — `next` from `task_complete` claimed without `task_next`, the answer to #50 and a new package arrived in the `task_complete` inbox; ⚠ a Stop-hook nudge was not yet observed | — | — |

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

### Run 3 — 2026-10-30 to 2026-11-01, Claude Code, main checkout (lead) and engine worktree

**Method:** no supervised run; evidence comes from the Claude Code transcripts of eleven sessions (six in the main checkout, five in the engine worktree), filtered to Airlock MCP calls and `airlock` CLI commands. Totals: engine 5 `session_start`, 32 claims, 29 completions, 4 heartbeats, 16 questions, 18 CLI commands; main 6 `session_start`, 53 `task_create`, 41 `task_update`, 16 answers, 15 resolves, 15 `doc_sync`, 70 CLI commands. This run also covers the switch to tracks (1 Nov): a lead session created the tracks and assigned all tasks, another created the `ui` track with its packages, both without help.

**Observed:**
- **B:** the engine claims, works, completes with the commit hash and a summary in the notes, then calls `task_next` and claims the next package (E-74 → E-10 on 1 Nov). Completing E-67 reported `now_ready: ["E-68"]`. Long packages get a heartbeat with a longer lease (`lease_minutes` 90–240). The review path (`review: true`, approve / reject) was never used.
- **C:** the engine asked 16 questions with a spec as topic; the lead answered each with `spec_file` and resolved it. The engine fetched answers with `question_list` (19 calls) rather than waiting for the briefing, so `answered_questions` was empty in every briefing.
- **D:** two lead sessions restarted while holding a task; the briefing listed it under `claimed` and the session continued without claiming again. Before an idle stop the engine released its task explicitly (`task_release`).
- **E:** on 1 Nov the engine, two lead sessions and a UI design session worked at the same time without conflicting claims.
- **F:** claiming a blocked task returned `task E-61 is blocked by unfinished prerequisites`; the agent read the prerequisites with `task_get` and took the next ready package.
- **G:** lead sessions run `doc_sync` (MCP) or `airlock sync` (CLI) after each plan change and commit `PLAN.md`, `QUESTIONS.md` and the state file with it.
- **H:** every session opens with `session_start`; agents read `airlock … --help` when unsure of a command instead of guessing flags.

**Problems:**
1. **Shared session in the main checkout.** Three Claude Code sessions (lead work, project review, UI concept) ran `airlock mcp --role lead --session lead-1` from the same `.mcp.json`. Airlock treats them as one session: any of them can heartbeat, complete or release the others' claims, and the briefing shows no other lead sessions. Run 1b's lesson (one `--session` per harness) does not hold when the same harness is started several times in one checkout.
2. **Correcting an answer after resolving.** The lead answered question 15, resolved it, found a mistake two minutes later and answered again: `question #15 is already resolved`. It fixed the spec file, did not mention the refusal, and moved on. Airlock and the generated `QUESTIONS.md` still hold the wrong first answer.
3. **Clean-room sessions use the CLI where the MCP tool set stops.** The engine has no `task_create` / `task_update`; it ran `airlock task add` and `airlock task edit --blocked-by` in the engine worktree instead (E-67, E-6f). Nothing in the CLI limits a clean-room binding to the clean-room tool set.
4. **Ids before PR #13.** Lead and engine sessions picked task ids themselves and collided (`task E-17 already exists`, `task E-32 already exists`); one engine attempt to pass an id on the CLI was stopped by Claude Code's permission prompt. Fixed by Airlock assigning ids (PR #13).
5. **Role setup missing.** The one-time `airlock role set` from OpenSHR's `docs/PROCESS.md` had never been run, so every briefing carried `instructions: null`; the clean-room rule reached the engine only through `CLAUDE.md`. Fixed by running the setup (1 Nov).
6. **Old process after upgrade.** After installing the tracks build, one Claude session started the day before still ran the previous `airlock mcp`; it refuses the migrated store, which is safe but only visible with `pgrep`.

**Lesson:** the agents use Airlock well where it answers, and work around it where it refuses: a refused write gets fixed elsewhere or done through the CLI, and the user hears about it only by reading transcripts. Problems 1–3 and 6 would have stayed invisible without them.

### Run 4 — 2026-11-01, Claude Code, engine worktree (Scenario D with the SessionStart hook)

**Method:** supervised run of work orders WO-1, WO-2 and WO-4 (`docs/plans/SESSION_CONTINUITY.md`). The engine worktree got the hook from `airlock setup claude-hooks` merged into its untracked `.claude/settings.local.json`; the main checkout did not (problem 3). The user drove the session; evidence comes from `airlock activity --session engine-1` and the hook records in the Claude Code transcript. Two `start` entries at 14:26:40 were manual size checks from another shell and are left out. Times are UTC.

**Observed:**

| Time | Channel | Call | What happened |
|---|---|---|---|
| 14:25:31 | CLI | `start` | First session with the hook: the hook ran at startup; asked "what is your task?", the agent read the briefing (from a file, problem 1) and named E-86 as the next package |
| 14:26:57 | MCP | `session_start`, `task_claim E-86` | After the user's go-ahead |
| 14:31:20 | MCP | `task_heartbeat E-86`, `changed: notes` | Progress note of 1017 characters: done so far, files ported, next steps |
| — | — | — | Claude Code killed (not `/exit`) and opened again in the same worktree |
| 14:33:21 | CLI | `start` | Hook at `startup`, new pid; the briefing named E-86 under "Your claimed work" with 58 min of lease and the progress note; the agent checked `git diff` and `cargo check` and reported the state without claiming again |
| 14:36:06 | MCP | `session_start`, `task_heartbeat E-86` | After "continue": a second briefing although the hook had delivered one (problem 4); heartbeat without a note |
| 14:42:34 | CLI | `start` | `/compact`, hook at `compact`, build with PR #22: 4.7 KB briefing in context; "where are you?" answered from it with no tool call |

**Problems:**
1. **The briefing was too big for the hook.** At startup the briefing was 21–28 KB, about 92 % of it six answered but unresolved questions repeated in full. Claude Code does not put a hook output of that size into the context: it saved it to a file and showed a 2 KB preview, so the agent had to `cat` the file (`head -80`, then `head -150`). After a compaction that would defeat the hook. Fixed in PR #22: an answer the session already got in full is one line (id, topic, spec file, how to read it again); the compact-time briefing was 4.7 KB and went straight into context.
2. **The hook's briefing named CLI commands** (`airlock task claim E-86`), while OpenSHR's engine writes through MCP only. Fixed in PR #22: `airlock start --hints mcp`, used by `setup claude-hooks`. Hooks installed before need `setup claude-hooks` again.
3. **The hook follows the current binding.** OpenSHR's main checkout is rebound between lead and spec sessions, and the lead sessions run `--role lead --session lead-1` from their own configuration. A hook there would brief a running lead session as spec after a compaction. Not installed there; Spec 06 §6 and the quickstart now recommend the hook only for worktrees with one fixed role.
4. **Briefing twice.** After the restart the agent called `session_start` again when told to continue, probably because OpenSHR's `CLAUDE.md` says to start with it. Harmless (a read), but it shows the instruction files should say that the hook already delivers the briefing.
5. **Progress note not refreshed.** Between the restart and `/compact` the agent worked about ten minutes and sent a heartbeat without a note, so the briefing after the compaction carried the note from before the crash. The compaction summary filled the gap this time; the workflow bullet alone did not make the agent update the note.

**Changes in Airlock:** PR #22 (problems 1 and 2); Spec 06 §6 and the quickstart (problem 3). OpenSHR's `CLAUDE.md` / `docs/PROCESS.md` (problem 4) are changed on its `main`.

**Lesson:** the hook plus the progress note make a crashed or compacted session pick up its task without help, as long as the briefing fits into context. The briefing's size is part of the hook's contract, so old answers must not pile up in it.


### Run 5 — 2026-11-01 to 2026-11-02, Claude Code, main checkout (lead) and engine worktree (Scenario I)

**Method:** an evening of normal work by the user, not a staged run. The engine worktree had the Stop hook from `airlock setup claude-hooks --auto-continue`; the main checkout had none, because it is rebound between lead and spec (Run 4, problem 3). The engine was restarted at 00:29:31 onto the build with WO-2 and WO-3 (PRs #36, #37). Evidence comes from `airlock activity --since`, the `events` table, and the inbox cursors. Times are CET.

**Observed:**

| Time | Session | Call | What happened |
|---|---|---|---|
| 00:22:14 | engine | `task_complete E-116` | Old build: the agent called `task_next` and `inbox` itself (00:22:20), as OpenSHR's instructions now say, and claimed E-117 (`high`) |
| 00:26:33 | engine | `question_ask` (#50) | Event `question_asked` for the coordinating roles |
| 00:29:31 | engine | `start` | Restart onto the WO-2/WO-3 build; the SessionStart hook delivered the briefing |
| 00:29:51 | engine | `task_complete E-117` | The result named E-115 as `next`; claimed 7 s later without `task_next` |
| 00:30:32 | engine | `task_complete E-115`, `task_claim E-118` | The same, 5 s apart; both `normal`, the older first |
| 00:34:15 | lead | `inbox` → 1 | The lead read #50 when the user turned to it; no hook in the main checkout |
| 00:37:41 | lead | `question_answer #50` with `spec_file` | |
| 00:43:23 | engine | `question_ask` (#51) | Still open when this was written |
| 00:43:31 | engine | `task_complete E-118` | The inbox in the result held the answer to #50 and the new package E-120 (cursor 10 → 17); the user copied nothing |

**Not observed:** a Stop-hook nudge. When the engine ended its turn after E-118, its inbox held nothing new (its cursor was at the newest engine event, and #51 was its own), so the hook let it stop, as it should; turns without news are not logged. The lead has no hook, so the second half of Scenario I (lead nudged by a question, engine nudged by the answer) is still open. It could not have run as installed: the hook read the binding's session, and the main checkout was bound to `spec-1` while the lead ran as `lead-1` (problem 4).

**Problems:**
1. **The lead saw its own changes.** The inbox reads at 00:23:05 (3 items) and 00:41:28 (4 items) were mostly `task_ready` and `task_changed` events the lead had caused itself by creating and releasing tasks.
2. **Follow-ups reached the lead only through the user.** The engine wrote in its completion notes which follow-up work it needed; the lead learned about it only when the user passed it on.
3. **Waiting tasks parked as `low`.** Five lead tasks waited for trace captures from the user. The lead lowered their priority and changed their titles (00:38–00:43), with two refusals on the way: ``task `L-59` is in_progress; use --force to edit it anyway`` and ``task `L-60` is not claimed``. A `low` task is still handed out once nothing else is ready.
4. **The Stop hook followed the binding only.** Like the SessionStart hook (Run 4, problem 3), `hook stop` read the session bound to the worktree. In OpenSHR's main checkout, bound to `spec` while the lead runs as `lead-1` from its own `.mcp.json`, a lead hook would have watched the spec session's inbox.
5. **The main checkout's hooks ran in the engine session.** After the hooks were installed in the main checkout (2 Nov, 08:27), `/hooks` in the engine session listed two SessionStart and two Stop hooks: Claude Code runs the main checkout's `.claude/settings.local.json` hooks in linked worktrees too. The engine's `/restart` ran `start` for `engine-1` and, in the same second, for `spec-1`, so the clean-room session got the spec role's briefing (rules and the next spec package, no analysis material) and would have been nudged for the spec inbox.

**Changes in Airlock:** PR #38: events carry the session that caused them, and a session's inbox skips its own (problem 1); `task_done` with the completion notes goes to the coordinating roles (problem 2); `task hold` / `task resume` and MCP `task_hold` / `task_resume` (problem 3). PR #39 added `inbox_wait`, not used in this run. PR #40: `hook stop` and `start` take the session and role from `AIRLOCK_SESSION` / `AIRLOCK_ROLE` before the binding, so a lead session started with `AIRLOCK_SESSION=lead-1 AIRLOCK_ROLE=lead claude` gets its own nudges in the shared checkout (problem 4). PR #41: `setup claude-hooks` writes `-C "$CLAUDE_PROJECT_DIR"` instead of a path, so an inherited copy serves the worktree it runs in, and `airlock doctor` reports hooks inherited from the main checkout with a fixed `-C` (an error for a clean-room worktree) (problem 5). OpenSHR's `CLAUDE.md` / `docs/PROCESS.md` describe the task boundary (one task at a time, the next one from Airlock, no copying between sessions), the completion notes as the way to ask the lead for follow-ups, and `task hold`.

**Lesson:** at the task boundary the handoff works without the human: the engine went through four packages in priority order and received an answer and a new package without anyone copying text. What still needed the human was the other direction, the engine's requests to the lead; `task_done` covers that now, and the Stop hook still has to prove itself in a run with hooks on both sides.
