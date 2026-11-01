# Work Orders: Session Continuity (Phase 3c) and Follow-ups

Six self-contained work orders, each sized for one agent session, in Airlock's own
*Read / Touch / Do / Verify* form. They come out of a review of the codebase at
commit `ac1a4e9` against the problem that started Airlock: a Claude Code session
runs out of context, a new one is opened, and the new one has to be told again who
it is, which worktree it works in, and where the old one stopped.

What already exists and is **not** to be rebuilt: worktree bindings as the durable
agent identity (`bindings` table, Spec 06 §2), the stable session id
`<role>@<directory>`, the briefing with claimed work first (`src/briefing.rs`), and
its three deliveries (`airlock start`, MCP `session_start`, MCP prompt `start`).
The orders close the gaps around them.

## Rules for every order

- One branch per order (`feat/…`, `fix/…`, `docs/…`), merged into `main` by pull request.
- Behavioral changes update the affected spec in `docs/spec/` **in the same change**;
  the spec sections to touch are named in each order. Architectural changes also
  update `README.md` and `PROJECT_OVERVIEW.md`.
- `cargo test`, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --check`
  pass before the PR. MSRV 1.85, edition 2024, `Cargo.lock` locked; no new dependencies
  unless an order says so.
- Commit prefixes as in the history: `feat:`, `fix:`, `test:`, `docs:`.
- Airlock never executes a task's `verify`; nothing in these orders changes that.
- Session ids stay self-declared. No order adds authentication or OS-level isolation
  (Spec 05 §4.2). Where an order adds a check, it is a guardrail for cooperating
  agents and is documented as such.

## Sequence

| Order | Depends on | Schema |
|---|---|---|
| WO-1 SessionStart hook | — | — |
| WO-2 Progress notes on a lease | — | migration 4 |
| WO-3 Session instance presence | — | migration 5 (or 4 if it lands before WO-2) |
| WO-4 Restart validation | WO-1, WO-2 (WO-3 for the duplicate-tab check) | — |
| WO-5 Directory entries in `touch` | — | — |
| WO-6 Documentation housekeeping | — (last, so it records the final state) | — |

WO-1, WO-2, WO-5, and WO-6 can run in parallel. WO-2 and WO-3 both add a migration;
whichever merges second renumbers its migration constant.

---

## WO-1: Claude Code SessionStart hook from `airlock setup`

**Why.** The briefing exists, but Claude Code only sees it if the agent remembers to
call `session_start` or the user types `/mcp__airlock__start`. A SessionStart hook
runs `airlock start` whenever a session begins, resumes, is cleared, or is
compacted, and Claude Code adds the hook's stdout to the model's context. The
`compact` case is the real gain: after a compaction the briefing is re-injected, so
the role, rules, held task, and progress survive even without opening a new tab.

**Read.**
- `docs/spec/06_onboarding.md` §3.2 (Delivery), §5 (doctor), §6 (setup)
- `src/setup.rs` (whole file; note how `from_binding`, `local_args`, and the
  stdout/stderr split work)
- `src/doctor.rs` `check_harnesses` and `collect` (how `.mcp.json` entries are
  checked today)
- `docs/walkthroughs/agent-validation.md` Run 1 problem 1 (bare command names and
  `PATH`) and Run 2 problem 4 (mixed stdout)
- `docs/walkthroughs/quickstart.md` §4 "Connecting an AI agent over MCP"
- Claude Code hooks reference: SessionStart event, matchers `startup`, `resume`,
  `clear`, `compact`; hook stdout becomes context; settings files
  `.claude/settings.json` (committed) and `.claude/settings.local.json` (per
  checkout, not committed)

**Touch.** `src/cli.rs` (`Harness` enum), `src/setup.rs`, `src/doctor.rs`,
`tests/onboarding.rs`, `docs/spec/06_onboarding.md`, `docs/spec/02_cli_interface.md`
(setup line), `docs/walkthroughs/quickstart.md`, `README.md` (feature bullet).

**Do.**
1. Add a harness value `claude-hooks` to `airlock setup`. It prints a complete
   `.claude/settings.local.json` whose only key is `hooks`:
   ```json
   {
     "hooks": {
       "SessionStart": [
         {
           "matcher": "startup|resume|clear|compact",
           "hooks": [
             { "type": "command", "command": "<bin> -C <root> start", "timeout": 15 }
           ]
         }
       ]
     }
   }
   ```
   `<bin>` is the running binary's canonical path and `<root>` the worktree root,
   both shell-quoted (paths with spaces must work). `-C <root>` is passed even though
   hooks run in the session's directory, so the hook keeps working when Claude Code
   is started from a subdirectory. With `--role <other>` (second role in a shared
   checkout, same rule as the existing `setup claude`), append `--role <other>`.
2. Keep the stdout/stderr contract of `setup`: stdout carries only the JSON. The
   stderr hint says: save as `<root>/.claude/settings.local.json` if that file does
   not exist, otherwise merge the `hooks` key into it; and that the file is local
   to the worktree and not committed. `--json` output carries `role`, `session`,
   `root`, `command`, `args`, `hint` like the other harnesses.
3. Do **not** change `setup claude` to include hooks: `.mcp.json` cannot carry
   them, and a user who already has a `settings.local.json` must merge, not
   overwrite.
4. `airlock doctor`: for every worktree, read `.claude/settings.json` and
   `.claude/settings.local.json`; for every SessionStart hook whose command
   mentions `airlock`, report (same wording style as the `.mcp.json` checks):
   - a bare command name instead of an absolute path (warn, suggest the path);
   - a `-C` pointing at a directory that is not this worktree (warn);
   - `--role` contradicting a locked binding (error);
   - a file that is not valid JSON (error).
   A worktree with a `.mcp.json` for Airlock but no SessionStart hook gets an
   informational `ok`-level line naming `airlock setup claude-hooks`, not a warning:
   the hook is optional.
5. Quickstart §4: add the hook step right after `airlock setup claude > .mcp.json`
   and say what it does. The "Coordination (Airlock)" section for `CLAUDE.md` keeps
   its first bullet (the hook does not exist for other harnesses).
6. Spec 06: §3.2 gains the hook as a fourth delivery; §6 gains `claude-hooks`; §5
   gains the hook checks. Spec 02's command table gets the new harness value.

**Verify.**
- `tests/onboarding.rs`: extend `setup_prints_harness_configurations` or add
  `setup_prints_claude_hooks`: stdout parses as JSON, has exactly the `hooks` key,
  the command starts with the canonical binary path, contains `-C <root>` and
  `start`, and contains `--role` only when `--role` was given for a non-bound role.
  Extend `setup_writes_only_file_content_to_stdout` to the new harness.
- `tests/onboarding.rs`: a doctor test with a `.claude/settings.local.json`
  containing a bare `airlock start` hook and one with `--role lead` in a worktree
  locked to `engine`: the first warns, the second errors; a valid hook produces no
  warning.
- Manual, recorded in `docs/walkthroughs/agent-validation.md` as part of WO-4:
  start Claude Code in the engine worktree with the hook installed; the first
  assistant turn shows knowledge of the briefing without being asked; run
  `/compact`; the briefing is present again.
- `cargo test`, clippy, fmt green.

---

## WO-2: Progress notes on a lease

**Why.** `notes` is written only by `done`, `approve`, and `reject`
(`src/store.rs`, `set_state` callers). A session that is about to lose its context
has no place to leave "done so far / remaining / next step" for its successor. The
briefing of the successor shows the work package, but not where the predecessor
stopped. ChatGPT's claim that notes cover this is wrong.

**Read.**
- `docs/spec/01_architecture_and_data_model.md` §2.1 (Task, lifecycle table, leases §2.4)
- `docs/spec/02_cli_interface.md` `task heartbeat`, `task show`
- `docs/spec/03_mcp_protocol.md` §3.2 `task_get`, §3.4 `task_heartbeat`
- `docs/spec/05_git_sync_and_airlock.md` §3.2 (session state is not exported)
- `docs/spec/06_onboarding.md` §3.1 (briefing content)
- `src/store.rs`: `MIGRATIONS`, `SCHEMA_V3`, `SELECT_TASKS`, `heartbeat`,
  `release`, `complete`, `set_state`, `task_from_row`, `released_if_expired`
- `src/core.rs` `Task`; `src/briefing.rs` `WorkPackage`, `ClaimedTask`, `package`
- `src/snapshot.rs` (which task fields are exported) and `tests/snapshot.rs`
- `src/web/app.js` task detail rendering

**Touch.** `src/store.rs`, `src/core.rs`, `src/cli.rs`, `src/app.rs`, `src/mcp.rs`,
`src/briefing.rs`, `src/web/app.js`, `tests/store.rs`, `tests/e2e.rs`,
`tests/mcp.rs`, `tests/onboarding.rs`, `docs/spec/01…`, `02…`, `03…`, `05…`, `06…`,
`docs/walkthroughs/quickstart.md` (CLAUDE.md section, one bullet).

**Do.**
1. Migration 4: `ALTER TABLE tasks ADD COLUMN progress TEXT;` Add `progress:
   Option<String>` to `Task`, `SELECT_TASKS`, and `task_from_row`.
2. Semantics (write them into Spec 01 §2.1 as a short subsection "Progress"):
   - `progress` is **session state** like `claimed_by`: the lease holder's running
     note of what is done, what remains, and the next step. It replaces the previous
     value (no append); the agent writes the whole current state each time.
   - Written only by `task heartbeat <id> --notes <text>` / MCP `task_heartbeat`
     with `notes`. A heartbeat without notes keeps the existing progress. Only the
     lease holder can write it (the existing `ensure_holder` check).
   - **Kept** on release and on lease expiry, so the next session that claims the
     task sees how far the previous one got. **Cleared** when the task leaves
     `in_progress` to `done` or `in_review`; the final state belongs in `notes`
     via `task done --notes`. `task edit` never touches it.
   - Not exported by `sync` (Spec 05 §3.2 table: add `progress` to the "not
     exported" row), so heartbeats with notes never change the snapshot.
3. Surfaces:
   - CLI `task heartbeat --notes`; `task show` prints a `Progress:` line when set.
   - MCP `task_heartbeat` gains optional `notes` (description: "Running note for
     your successor: done so far, remaining, next step. Replaces the previous
     note."); `task_get` includes `progress`.
   - Briefing: under "Your claimed work" each package shows `- **Progress:** …`
     when set. Under "Next work package", a task with leftover progress from an
     earlier holder shows `- **Earlier progress:** …`. The Workflow section gains
     one bullet: "Before your context fills, record where you are with
     `task_heartbeat` notes; your successor reads it in this briefing."
   - Dashboard: task details show progress; the Leases view shows it next to the
     session. No REST write endpoint (session operation, like claim).
   - Quickstart "Coordination (Airlock)" section: extend the "Keep your lease
     alive" bullet with the progress note.
4. No new MCP tool. `task_heartbeat` with notes is enough and keeps the clean-room
   tool count unchanged.

**Verify.**
- `tests/store.rs`: `heartbeat_records_progress_and_complete_clears_it` (heartbeat
  with notes sets it; a second heartbeat without notes keeps it; a third replaces
  it; `complete` clears it; `notes` untouched by heartbeats),
  `progress_survives_release_and_expiry` (release keeps it; expiry via `claim` from
  another session after `now` passes the lease keeps it and the new holder reads
  it), `only_the_holder_writes_progress`.
- `tests/snapshot.rs` / `tests/e2e.rs` `sync_is_byte_stable_and_ignores_session_state`:
  a heartbeat with notes does not change the snapshot bytes.
- `tests/onboarding.rs` `start_briefs_claims_next_work_and_answers`: the claimed
  package shows the progress text; a released task with progress shows "Earlier
  progress" to the next session.
- `tests/mcp.rs` `unbound_session_runs_the_full_workflow`: `task_heartbeat` with
  `notes`, then `task_get` returns `progress`; `task_complete` clears it.
- `cargo test`, clippy, fmt green.

---

## WO-3: Session instance presence

**Why.** Two Claude Code tabs in the same worktree are both
`engine@openshr-engine`. For a restart that is wanted: the new tab inherits the
lease. But when the old tab is still open and working, Airlock cannot tell, and
both act under one identity. This order makes the situation **visible**; it does
not fence anyone out.

**Read.**
- `docs/spec/06_onboarding.md` §2.3, §3.1, §5
- `docs/spec/03_mcp_protocol.md` §2 (Server Binding)
- `src/app.rs` around line 71 (where `mcp-<8 hex>` is generated), `src/mcp.rs`
  `Server::new`, `tools_call`, `briefing`
- `src/store.rs` `MIGRATIONS`, `write`, `bindings`/`bind` (a small table with its
  own accessors, as a pattern)
- `src/briefing.rs` `Briefing`, `OtherClaim`, `markdown`
- `src/doctor.rs` `check_bindings`

**Touch.** `src/store.rs`, `src/mcp.rs`, `src/app.rs`, `src/briefing.rs`,
`src/doctor.rs`, `tests/store.rs`, `tests/mcp.rs`, `tests/onboarding.rs`,
`docs/spec/06_onboarding.md` (new §2.5), `docs/spec/03_mcp_protocol.md` §2.

**Do.**
1. Migration 5:
   ```sql
   CREATE TABLE instances (
       session    TEXT NOT NULL,
       instance   TEXT NOT NULL,
       started_at INTEGER NOT NULL,
       last_seen  INTEGER NOT NULL,
       PRIMARY KEY (session, instance)
   ) STRICT;
   ```
   Store API: `register_instance(session, instance, now)`,
   `touch_instance(session, instance, now)`, `instances(session) -> Vec<Instance>`.
   Every `write` transaction also deletes rows with `last_seen` older than 24 h, so
   the table never grows.
2. Only the **MCP server** registers: it is the long-lived process. It generates an
   instance id `mcp-<8 hex>` at startup (independent of the session id, which may
   come from the binding), registers it, and touches it on every `tools/call` and
   `prompts/get`. CLI invocations are one-shot and do not register.
3. Briefing: new field `other_instances: Vec<Instance>` — instances of the **same
   session**, other than the caller's (the CLI passes `None` and sees all), with
   `last_seen` within the project's `lease_minutes`. Markdown, right under the
   header line, only when non-empty:
   > **Another instance of this session was active 3 min ago** (`mcp-ab12cd34`).
   > If that is a tab you left open, close it: Airlock cannot tell your actions
   > apart. If it was the session you are replacing, continue.
4. `airlock doctor`: under Bindings, warn for every session with two or more
   instances seen within `lease_minutes`.
5. Spec 06 §2.5 "Session instances": the table, who registers, what the briefing
   and doctor show, and this sentence: "Airlock does not refuse a stale instance;
   fencing (refusing writes from an instance that was superseded) is a possible
   later opt-in and would still not replace OS-level isolation (Spec 05 §4.2)."
   Spec 03 §2: the server registers an instance at startup.

**Verify.**
- `tests/store.rs`: `instances_are_registered_touched_and_pruned`.
- `tests/mcp.rs`: `a_second_instance_of_the_same_session_is_reported`: two servers
  started with the same `--session` (sequentially in the test; the first one's
  process is kept alive or its `last_seen` is recent): the second's `session_start`
  Markdown contains "Another instance of this session"; a server with a different
  session does not see it; after `last_seen` is older than the lease window (use a
  project with `lease_minutes` small, or set `last_seen` directly through the
  store in the test), the warning disappears.
- `tests/onboarding.rs`: doctor warns for a session with two live instances.
- Clean-room tool set unchanged (count still 9 in the existing test).
- `cargo test`, clippy, fmt green.

---

## WO-4: Restart validation (Scenario D) — automated and real

**Why.** `docs/walkthroughs/agent-validation.md` has Scenario D empty for every
harness. It is the scenario Airlock was built for. The only related test today is
the reclaim after expiry by *another* session (`tests/store.rs:79`); nothing tests
that the *same* session, in a new process, finds its own claim.

**Read.**
- `docs/walkthroughs/agent-validation.md` (Scenarios, Status, Runs; follow the run
  format exactly)
- `tests/e2e.rs` `concurrent_claims_from_separate_processes_have_one_winner`,
  `tests/common/mod.rs` fixtures
- `tests/mcp.rs` `unbound_session_runs_the_full_workflow`
- `tests/onboarding.rs` `mcp_takes_role_and_session_from_the_binding`
- WO-1, WO-2, WO-3 as merged

**Touch.** `tests/e2e.rs`, `tests/mcp.rs`, `docs/walkthroughs/agent-validation.md`,
`docs/plans/PHASE_PLAN.md` (tick the validation items that pass).

**Do.**
1. `tests/e2e.rs` `a_restarted_session_resumes_its_claim`: in a bound worktree,
   process 1 runs `task claim E-1` and `task heartbeat E-1 --notes "half done"`;
   process 2 (fresh invocation, same worktree, no `AIRLOCK_SESSION`) runs
   `start --json`: `claimed[0].id == "E-1"`, its `progress == "half done"`, `next`
   is `null`; `task claim E-1` from process 2 fails with the existing "already
   claimed by this session; use `task heartbeat`" message; `task done E-1` from
   process 2 succeeds.
2. `tests/mcp.rs` `a_restarted_mcp_server_resumes_the_session`: server A (bound
   worktree) claims and heartbeats with notes, stdin closes; server B in the same
   worktree calls `session_start`: the Markdown names the task under "Your claimed
   work" with the progress line; `task_complete` from B succeeds.
3. Real run on OpenSHR, Claude Code, engine worktree, with WO-1's hook installed.
   Record it as "Run 3" in the walkthrough with the existing headings
   (**Observed**, **Problems**, **Agent behavior**, **Setup fix**, **Changes in
   Airlock**). Steps:
   1. Engine session claims a task via MCP and works for a while; it records
      progress with `task_heartbeat` notes.
   2. Kill the Claude Code process (not `/exit`: simulate a crash).
   3. Open a new Claude Code in the same worktree. Expected: the hook fires, the
      first turn shows the briefing with the held task, lease time left, and the
      progress note; the agent continues without a new claim.
   4. Run `/compact` mid-task. Expected: the briefing is present again after
      compaction and the agent still knows its task.
   5. Open a **second** tab while the first is still running. Expected: the
      second tab's briefing carries the "Another instance" warning (WO-3) and the
      agent asks the user instead of working.
   Fill the Scenario D cell for Claude Code with ✅ or ⚠ plus the run reference.
   While at it, fill Scenario B (work cycle) from the same run; it is exercised
   on the way.
4. Every problem found goes into a `fix/validation-run-3` branch as in Runs 1–2,
   with the fix listed under "Changes in Airlock".

**Verify.**
- Both new tests pass on Linux, macOS, and Windows in CI.
- The Status table has D (and B) filled for Claude Code with a run reference.
- `docs/plans/PHASE_PLAN.md` Phase 3b: the validation item is updated to name
  which scenarios are done and which remain.

---

## WO-5: Directory entries in `touch` lists

**Why.** Spec 04 §3.5 counts only identical paths as overlap: `src/render/` does
not overlap `src/render/mesh.rs`. Tasks that touch a whole module are common
("Touch: `src/render/`"), and today they show as free of overlap. Semantic conflict
detection stays out of scope; this is the cheap part of the gap.

**Read.**
- `docs/spec/04_web_dashboard_and_api.md` §3.5–3.7
- `docs/spec/01_architecture_and_data_model.md` §2.1 "Selection Order" and
  "Overlap at Claim Time" (they defer to Spec 04 for path rules)
- `src/overlap.rs` (whole file, including the inline tests)
- `src/web/app.js` File matrix rendering

**Touch.** `src/overlap.rs`, `src/web/app.js` (only if the matrix needs a hint for
directory rows), `docs/spec/04_web_dashboard_and_api.md` §3.5,
`docs/walkthroughs/quickstart.md` (one sentence at the "no touch data" hint).

**Do.**
1. Rule, written into Spec 04 §3.5: after normalization, entry `a` **covers** path
   `b` when `a == b` or `b` starts with `a` followed by `/` (or `a` already ends
   with `/` and `b` starts with `a`). A trailing `/` is kept by normalization and
   only marks intent; `src/render` and `src/render/` cover the same paths. Two
   tasks overlap when any entry of one covers any entry of the other. A file path
   never covers anything but itself, since a file has no children.
2. Reported shared paths: for each covering pair, report the **more specific**
   path (the covered one), deduplicated and sorted. The File matrix lists it as a
   row; a directory entry itself appears as a row only when two tasks name the
   same directory.
3. `normalize_path` must keep a trailing `/` (today `"src/"` stays `"src/"`; keep
   the existing test `"src/"` ≠ `"src/a.rs"` for *equality* but add coverage).
4. No change to the claim-time behavior (warn, never refuse) or to the selection
   order; they call `analyze` and inherit the rule.

**Verify.**
- `src/overlap.rs` tests: `directory_entries_cover_files_beneath` (`src/render/`
  vs `src/render/mesh.rs` overlap; shared path reported is `src/render/mesh.rs`),
  `trailing_slash_is_optional` (`src/render` behaves the same),
  `sibling_files_do_not_overlap_by_directory` (`src/render/a.rs` vs
  `src/render/b.rs` do not overlap), `prefix_without_separator_is_not_coverage`
  (`src/rend` does not cover `src/render/a.rs`), `nested_directories_overlap`
  (`src/` vs `src/render/`).
- `tests/e2e.rs` `overlapping_work_is_handed_out_last_and_claims_ask_the_lead`:
  add a case where the occupying task touches a directory and the ready task a
  file beneath it; the file task is handed out last and the claim records the
  overlap question.
- `tests/web.rs`: `/api/overlaps` lists the covered file under `files`.
- `cargo test`, clippy, fmt green.

---

## WO-6: Documentation housekeeping

**Why.** Several documents describe a state older than the code. An agent reading
them first (as the briefing tells it to) inherits the drift.

**Read.** `PROJECT_OVERVIEW.md`, `README.md`, `docs/plans/PHASE_PLAN.md`,
`docs/spec/03_mcp_protocol.md` §1, `docs/walkthroughs/agent-validation.md`
Scenario A, `Cargo.toml`, `src/mcp.rs` `tool_definitions`.

**Touch.** Those files only. No code.

**Do.**
1. `PROJECT_OVERVIEW.md`:
   - Delete "Planned: `axum` and `rust-embed` (Phase 4)." Add rows for `axum`
     (0.8, dashboard REST/SSE), `tokio` (runtime for `serve` only), `getrandom`
     (dashboard token), `futures-util` (SSE stream) with the same column style.
     `rust-embed` is not used; assets are `include_str!`.
   - Project Stats "Status": Phases 1–4 implemented; open: agent validation
     (Phase 3b) and Phase 3c Session Continuity (link this document).
   - Check the Source Files list against `src/` and `tests/` (add anything missing,
     e.g. confirm `src/web/` assets are mentioned).
2. `README.md` status line: "Phase 4 done; Phase 3c (session continuity) and the
   agent validation in progress", with the links.
3. `docs/plans/PHASE_PLAN.md`: Phase 3b is no longer "(Current)"; mark it
   "(validation open)". Phase 3c lists WO-1 to WO-5 as checkboxes (already added
   with this document; tick what has merged).
4. `docs/spec/03_mcp_protocol.md` §1 example configuration: add one sentence that
   with a bound worktree `args: ["mcp"]` suffices (Spec 06 §2.3) and the example
   shows the explicit form.
5. `docs/walkthroughs/agent-validation.md` Scenario A: the tool counts are
   "9 clean-room, 14 otherwise" (`task_update` was added after the table was
   written; `src/mcp.rs` defines 14 tools).
6. Do not touch `docs/superpowers/`: those are historical design records.

**Verify.**
- `grep -n "rust-embed\|Planned:" PROJECT_OVERVIEW.md` finds nothing.
- Every dependency in `Cargo.toml` `[dependencies]` has a row in the overview table.
- `grep -c '"name": "' src/mcp.rs` (or counting `tool(` calls in
  `tool_definitions`) matches the number written in the walkthrough.
- Links in the changed files resolve (`docs/plans/SESSION_CONTINUITY.md` exists).
- `cargo test` still green (doc-only change; CI runs anyway).

---

## Explicitly not ordered

These were discussed and are deliberately left out until WO-1 to WO-4 have proven
themselves on OpenSHR:

- Fencing of superseded instances (refusing their writes). WO-3 makes duplicates
  visible first.
- Event notifications to agents (MCP notifications, a resident daemon). SSE exists
  for the dashboard; agents poll through the briefing.
- Agent lifecycle management (starting and stopping harnesses from Airlock),
  automatic scheduling, replanning on failure.
- A separate decision or artifact store. Questions with `spec_file` are the
  decision record; task `notes` with commit references are the artifact record.
- Multi-machine synchronization (ROADMAP milestone 4).
- Semantic conflict detection beyond `touch` lists.
