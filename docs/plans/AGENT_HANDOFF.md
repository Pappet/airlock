# Work Orders: Handoff Between Sessions (Phase 3d)

Self-contained work orders, each sized for one agent session, in Airlock's own
*Read / Touch / Do / Verify* form. Written 1 Nov 2026 against `main` after PR #33
(store schema 6, 18 MCP tools, 9 of them for clean-room roles). Review findings
from #34 are folded in.

## The problem

On OpenSHR the human still carries messages between sessions by hand: the last
output of the engine session is pasted into the lead session so the lead can answer
its questions, and the lead's last output is pasted back into the engine session
because it says that a new task exists. Two things go wrong with that:

1. **Work order.** The engine often finishes several tasks in one turn. It plans
   "E-5, then E-6, then E-7" up front, so a more important task the lead created in
   the meantime is only noticed at the end of the turn, if at all.
2. **The airlock boundary.** Pasting the lead's whole output into the clean-room
   session carries whatever the lead had in front of it, possibly analysis
   material. Airlock exists so that only work packages, answers, and review
   feedback cross that boundary.

Everything the human copies is already in the store: a question was asked, an answer
was given, a task was created, a task was rejected. What is missing is that the
*other* session learns about it, at the right moment.

## Principles

- **Finish first, then look.** A session is never interrupted mid-task. It checks
  for news at the boundary between two tasks, and when its turn ends.
- **The boundary between tasks is the important moment, not the end of the turn.**
  News arrives in the result of `task_complete`, where the agent looks anyway, so it
  works in every harness (Claude Code, DSH, Antigravity) without hooks.
- **`task_next` decides the order.** After every completed task the agent takes the
  next package from Airlock instead of from its own plan. `task_next` already orders
  by track priority, task priority, then age (Spec 01 §2.1).
- **Only what may cross the airlock.** An inbox carries ids, titles, priorities,
  topics, and answers — what the receiving role could already read through its own
  tools. Never the other side's output.
- **Guardrails, not enforcement.** As in Phase 3c: session ids stay self-declared;
  warnings guide cooperating agents and are documented as such.
- **The human stays in charge.** Automatic continuation is opt-in per worktree,
  capped, and visible in the activity log.

## Rules for every order

The rules of [`SESSION_CONTINUITY.md`](SESSION_CONTINUITY.md#rules-for-every-order)
apply unchanged: one branch and pull request per order, specs updated in the same
change, `cargo test` / clippy / fmt / `node --check src/web/app.js` green, MSRV 1.85,
no new dependencies, every new command or tool through the activity-log path with free
text recorded as a key only.

## Sequence

| Order | Depends on | Schema |
|---|---|---|
| WO-1 Events and the inbox | — | migration 7 (the store is at 6) |
| WO-2 News at the task boundary | WO-1 | — |
| WO-3 Stop hook for Claude Code | WO-1 | — |
| WO-4 Waiting for work (`inbox_wait`) | WO-1 | — |
| WO-5 Validation on OpenSHR and docs | WO-2, WO-3 (WO-4 if done) | — |

Order of work: WO-1, WO-2, WO-3, WO-5; WO-4 when the first four have proven
themselves.

---

## WO-1: Events and the inbox

**Why.** "What is new for me?" cannot be answered reliably from timestamps on
`tasks` and `questions`: a task becomes ready when a *prerequisite* is completed, a
priority change does not create a task, and a rejection leaves no marker on the
role. An append-only event list written in the same transaction as the change gives
every session an ordered, gap-free feed and a cursor into it.

**Read.**
- `docs/spec/01_architecture_and_data_model.md` §2.1 (lifecycle, readiness), §2.2
  (questions), §2.5 (tracks), §3.1–3.2 (store, concurrency), §3.5 (activity log)
- `docs/spec/02_cli_interface.md`, `docs/spec/03_mcp_protocol.md` §2.1 (clean-room
  tool set)
- `src/store.rs`: `MIGRATIONS`, `write`, `add_task`, `edit_task`, `claim`, `release`,
  `complete`, `approve`/`reject` (`set_state`), `ask`, `answer`, `edit_track`
- `src/mcp.rs` `CLEAN_ROOM_TOOLS`, `tool_definitions`, `run_tool`
- `src/activity.rs` `summarize_cli` / `summarize_json`

**Touch.** `src/store.rs`, `src/core.rs`, `src/cli.rs`, `src/app.rs`, `src/mcp.rs`,
`src/activity.rs`, `tests/store.rs`, `tests/mcp.rs`, `tests/e2e.rs`, `tests/activity.rs`,
`docs/spec/01…` (§3.1: `airlock.db` also holds `events` and `inbox_cursors`), `02…`, `03…`,
`05…` (§3.2: `events` and `inbox_cursors` join the activity log and the process
registry as machine-local session state that is never exported).

**Do.**
1. Migration 7 (keeping the store's `STRICT` tables):
   ```sql
   CREATE TABLE events (
     seq INTEGER PRIMARY KEY AUTOINCREMENT,
     at INTEGER NOT NULL,
     kind TEXT NOT NULL,
     role TEXT NOT NULL,          -- the role the event is for
     task_id TEXT, question_id INTEGER, track TEXT
   ) STRICT;
   CREATE INDEX events_role_seq ON events(role, seq);
   CREATE TABLE inbox_cursors (
     session TEXT NOT NULL,
     role TEXT NOT NULL,
     seq INTEGER NOT NULL,                  -- read up to here
     nudged_seq INTEGER NOT NULL DEFAULT 0, -- WO-3: last event a Stop hook nudged for
     PRIMARY KEY (session, role)
   ) STRICT;
   ```
   Events carry **no text**. They are session state: never exported by `sync`
   (Spec 05 §3.2), pruned after 30 days on write, like the activity log.
2. Written inside the same write transaction as the change (`Store::write`), so an
   event exists if and only if the change committed:

   | Kind | When | For role |
   |---|---|---|
   | `task_ready` | a task is created ready; becomes ready through a completed prerequisite (`now_ready`); returns to open by `release` or `reject` while its prerequisites are done | the task's role |
   | `task_changed` | `task edit` changes priority, track, or the work package of an `open` task | the task's role |
   | `track_changed` | a track's priority changes | every role with an `open` task on that track |
   | `task_rejected` | `reject` | the task's role |
   | `task_in_review` | `complete` with review | every non-clean-room role except the task's own |
   | `question_asked` | `ask` (also the automatic `overlap:` question) | every non-clean-room role except the asker's |
   | `question_answered` | `answer`, including corrections | the asker's role |

   An expired lease writes nothing (expiry is derived, Spec 01 §2.4); the task shows
   up again through `task_next`, not through the inbox. Document that.
3. Reading the inbox: `Store::inbox(session, role, now) -> Inbox` returns the events
   for `role` with `seq` greater than the session's cursor, joined with what the role
   may read: for tasks id, title, priority, track, status (a task that is no longer
   ready is dropped from `task_ready`); for questions id, topic, and — for
   `question_answered` — the answer and its spec file; for `track_changed` the track
   and its new priority. Duplicates of one task collapse into its latest event.
   The cursor is kept per session **and role**, so an unbound reader with `--role`
   never shares a cursor across roles. Reading advances the cursor to the highest `seq` read (`peek` reads without
   advancing; WO-3 needs it). A session without a cursor starts at the current
   maximum: a new session gets its briefing, not the history.
4. Surfaces:
   - CLI `airlock inbox [--peek] [--json]` for the bound or `--role` role.
   - MCP tool `inbox` (optional `peek`), added to `CLEAN_ROOM_TOOLS`: 19 tools, 10
     for clean-room roles. Description: "What changed for your role since you last
     looked: new or reprioritized ready tasks, answers to your questions, rejected
     reviews; for coordinating roles also new questions and tasks in review."
   - No REST endpoint and no dashboard view in this order (the dashboard already
     shows the state itself).
5. Activity log: `inbox` is logged with its item count in `target`, never the items.

**Verify.**
- `tests/store.rs`: `events_are_written_with_their_change` (each row of the table
  produces exactly its event; a failed transition writes none),
  `inbox_reads_each_event_once_per_session` (two sessions of one role each get it
  once; `peek` does not advance), `inbox_drops_tasks_that_are_no_longer_ready`,
  `a_new_session_starts_at_the_current_end`, `events_hold_no_text`.
- `tests/mcp.rs`: a clean-room session lists `inbox` and receives an answer to its own
  question; a coordinating session receives `question_asked` and `task_in_review`.
- `tests/e2e.rs`: `sync` output is byte-identical before and after events were written.
- `tests/activity.rs`: an `inbox` call logs the count only.

---

## WO-2: News at the task boundary

**Why.** This is the order that changes the engine's behavior: after every task it
learns what changed and takes the most important ready task, even when it is in the
middle of a long turn.

**Read.** WO-1 result; `src/store.rs` `complete`; `src/app.rs` (the `task next`
ordering at the `ready.sort_by` call); `src/mcp.rs` `task_complete`, `task_claim`;
`src/briefing.rs` (Workflow section); `docs/spec/03_mcp_protocol.md` §3.3 (`task_claim`) and §3.6 (`task_complete`);
`docs/spec/06_onboarding.md` §3.1; `docs/walkthroughs/quickstart.md` (CLAUDE.md section).

**Touch.** `src/app.rs`, `src/mcp.rs`, `src/briefing.rs`, `src/store.rs` (only if the
ordering moves there), `tests/mcp.rs`, `tests/e2e.rs`, `tests/onboarding.rs`,
`docs/spec/01…`, `02…`, `03…`, `06…`, `docs/walkthroughs/quickstart.md`.

**Do.**
1. Move the `task next` ordering into one function used by CLI, MCP, briefing, and
   the claim check below (it lives in `app.rs` today).
2. `task_complete` / `task done` return, after the completion is committed:
   - `inbox`: the session's inbox (WO-1), read and advanced;
   - `next`: the result of `task_next` for the session's role — id, title, track,
     priority, overlaps — or `null` when nothing is ready.
   The CLI prints both under the completion line ("Since your last look: …",
   "Next: E-15 (critical, Navigation) Wind gusts"). A failure to read either never
   fails the completion; it is reported as `inbox_error` / `next_error`.
3. `task_claim` / `task claim` warn, never refuse:
   - `higher_priority`: ready tasks of the role that rank **strictly higher** than the
     claimed one by track priority or task priority (age alone does not count).
   - `holding`: other tasks this session already holds a lease on ("one task at a
     time", Spec 01).
4. Briefing, Workflow section, replacing "Claim before you start; work on one task at a
   time":
   - "Work on one task at a time. After each `task_complete`, take the next task from
     its `next` field or from `task_next`; never plan task ids ahead and never claim a
     second task while you hold one."
   - "Read the `inbox` part of every `task_complete` result before you go on: a more
     important task or an answer may have arrived."
   The quickstart's CLAUDE.md section gets the same two bullets.

**Verify.**
- `tests/mcp.rs` `a_more_important_task_created_mid_turn_comes_next`: session claims
  E-1 (normal); another session creates E-2 (critical); completing E-1 returns E-2 as
  `next` and `task_ready E-2` in `inbox`.
- `tests/mcp.rs` `claiming_past_a_more_important_task_warns`: `higher_priority` names
  it; same-priority older tasks do not warn. `claiming_a_second_task_warns`: `holding`.
- `tests/e2e.rs`: `task done` prints the inbox and the next line.
- `tests/onboarding.rs`: the briefing contains the new Workflow bullets.

---

## WO-3: Stop hook for Claude Code

**Why.** WO-2 covers the boundary between tasks. When a session *ends its turn* —
the engine waits for an answer, the lead has answered everything — the human still
has to type "continue". A Claude Code Stop hook can tell the session to keep going
when its inbox is not empty, which replaces most of the copying.

**Read.** WO-1 result; `src/setup.rs` (`Harness::ClaudeHooks`), `src/doctor.rs`
`check_claude_hooks`; the Claude Code hooks reference for `Stop`
(`stop_hook_active`, `decision: "block"`, `reason`); Spec 06 §6.

**Touch.** `src/cli.rs`, `src/app.rs` (new `hook` command), `src/setup.rs`,
`src/doctor.rs`, `src/store.rs` (writing `nudged_seq`), `tests/onboarding.rs`, `docs/spec/02…`,
`06…`, `docs/walkthroughs/quickstart.md`.

**Do.**
1. `airlock hook stop`: reads the hook JSON from stdin (only `cwd` and
   `stop_hook_active`; Claude Code's `session_id` is a different id space and is
   ignored), resolves the worktree's binding, and `peek`s the inbox (WO-1) for the
   binding's session and role. If it holds events newer than the last
   nudge of this session, it prints
   `{"decision":"block","reason":"Airlock: <n> new for <role>: E-15 ready (critical); answer to #34. Call inbox, then continue."}`
   and records the nudge in `inbox_cursors.nudged_seq` (WO-1's migration). Otherwise it prints nothing and exits 0, so the
   session stops. The reason names references only; the agent reads the content
   through its own `inbox` call.
   Nudge state belongs to the binding's session id. Two Claude Code sessions in one
   worktree share it (as in Scenario E's shared `lead-1`): a nudge for one suppresses
   the same events for the other. That follows from self-declared session ids and is
   intended; document it in Spec 06.
2. Loop guards: never nudge twice for the same events (`nudged_seq`); at most
   `auto_continue_limit` nudges per session per hour (project setting, default 10;
   `0` disables the hook's effect); any error exits 0 silently, so a broken hub never
   traps a session.
3. `airlock setup claude-hooks --auto-continue` adds the `Stop` hook next to the
   SessionStart hook. Without the flag nothing changes (opt-in per worktree).
   `doctor` reports whether auto-continue is on for the worktree.
4. Activity log: each nudge is logged (`hook stop`, item count), so the dashboard's
   Activity tab shows how often sessions were sent back to work.
5. Lead side: nothing special. A `question_asked` event nudges the lead once; if the
   lead cannot answer from the specs and decisions, it leaves the question open and
   says so in its turn. The same event never nudges again, so an unanswerable question
   does not keep the lead busy.

**Verify.**
- `tests/onboarding.rs` `stop_hook_continues_once_per_news`: empty inbox → no output;
  a new answer → block with a reason naming `#<id>`; the same answer again → no output;
  the limit reached → no output; an unbound directory → no output, exit 0.
- `setup claude-hooks --auto-continue` emits both hooks; `doctor` recognizes them.
- `cargo test`, clippy, fmt green.

---

## WO-4: Waiting for work (`inbox_wait`)

**Why.** WO-3 helps a session that just finished a turn. A session with nothing to do
still sits at its prompt until someone types. A blocking wait lets it pick up the next
event within seconds, so engine and lead can play question-and-answer without the
human in between.

**Read.** WO-1 result; `src/web.rs` (`PRAGMA data_version` polling for SSE);
`src/mcp.rs` request handling and the heartbeat thread; client tool-call timeouts
(DSH `toolCallTimeoutMs` 300000 in `airlock setup dsh`, Claude Code MCP timeout).

**Touch.** `src/mcp.rs`, `src/cli.rs`, `src/app.rs`, `tests/mcp.rs`, `docs/spec/02…`,
`03…`, `06…`.

**Do.**
1. MCP `inbox_wait` (clean-room allowed: 20 tools, 11 for clean-room roles) with
   `timeout_secs` (default 240, max 280, below the 300 s DSH timeout): returns the
   inbox as soon as it is non-empty, otherwise an empty inbox at the timeout. Polls
   `PRAGMA data_version` once per second like the dashboard watcher (`src/web.rs`); no busy loop on the
   database file.
2. CLI `airlock inbox --wait [--timeout <secs>]` with the same behavior.
3. Briefing, Workflow: "When `task_next` has nothing for you and you have no open
   question to wait for, call `inbox_wait` instead of ending your turn — only if your
   instructions say you run unattended." The opt-in stays with the role's
   instructions in `airlock.toml` / CLAUDE.md, not with Airlock.

**Verify.**
- `tests/mcp.rs` `inbox_wait_returns_when_a_task_becomes_ready` (another connection
  creates a task; the wait returns well before its timeout),
  `inbox_wait_times_out_empty` (short timeout).
- Clean-room tool list contains `inbox_wait`.

---

## WO-5: Validation on OpenSHR and documentation

**Why.** The orders change how agents behave; only a real run shows whether they do.

**Do.**
1. Scenario I in `docs/walkthroughs/agent-validation.md` (a new row in its Scenarios and
   Status tables), run on OpenSHR with Claude
   Code in both worktrees, auto-continue on:
   - The engine works on a `normal` task; the lead creates a `critical` one. Expected:
     the engine finishes its task, sees `task_ready` in the `task_complete` result,
     and takes the critical task next — without the human copying anything.
   - The engine asks a question and ends its turn; the lead is nudged by the Stop
     hook, answers, ends its turn; the engine is nudged and continues.
   - Evidence from `airlock activity` (nudges, inbox calls, claim order), recorded as
     in Run 4.
2. Docs: Spec status lines, README feature table (inbox), `PROJECT_OVERVIEW.md`,
   `PHASE_PLAN.md` Phase 3d ticked. OpenSHR `CLAUDE.md` / `docs/PROCESS.md`: the two
   Workflow bullets from WO-2 and "do not paste the lead's output into the engine
   session; Airlock carries what may cross".

---

## Explicitly not ordered

- **Interrupting a running task** for a more important one. Finish first is the rule.
- **A dispatcher** that starts headless sessions (`claude -p`) when work is ready and
  no session of the role is connected. It depends on the harness and needs cost and
  parallelism limits; revisit after WO-4.
- **MCP server-to-client notifications** as the delivery path. Clients do not turn
  them into model turns today; the inbox works over plain tool results.
- **Escalation status for questions** ("needs a human"). Unanswered questions stay
  open and are visible in the dashboard; revisit if the lead leaves too many open.
- **Inbox in the dashboard.** The dashboard shows the state; a per-session feed adds
  nothing for the human.
