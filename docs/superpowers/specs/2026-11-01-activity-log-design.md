# Activity Log and Answer Corrections — Design

**Date:** 1 Nov 2026 · **Status:** agreed in conversation, pending written review

## Intent

The user runs several agent sessions on OpenSHR at once (leads, spec, a clean-room engine) and cannot see how they use Airlock. The only way to find out was to read the Claude Code transcripts by hand, as for validation run 3 (`docs/walkthroughs/agent-validation.md`). That run showed what stays invisible today:

- An agent hits a refusal and works around it without saying so. A lead corrected an answer after resolving the question, got `question #15 is already resolved`, fixed only the spec file and moved on; Airlock and `QUESTIONS.md` still hold the wrong answer.
- A clean-room engine session creates and edits tasks through the CLI, where its MCP tool set deliberately stops.
- An `airlock mcp` from before an upgrade keeps running; it is only visible with `pgrep`.

The user's own words for what they want to see: who did what where, problems, blockers, and the current state, with the chronology as the most important part. They would not notice a misbehaving session today until it caused a conflict, or until the agent happened to mention it.

This change adds two things:

1. **An activity log:** Airlock records every call from every channel (MCP, CLI, REST) and shows it in a new dashboard tab, with problems, blockers and current state on top.
2. **Answer corrections:** an answer can be corrected after the question was resolved; the correction is visible on the question and reaches the asking session in its next briefing as "answer changed".

The log also produces the evidence for future validation runs without anyone reading transcripts.

## Goals

- Every Airlock operation, read or write, from MCP, CLI and REST, leaves one log entry: when, which session, which role, which channel, which command, which task / question / track, and the result or the exact error.
- The dashboard has an **Activity** tab (layout below) that answers at a glance: what needs attention, who is working on what, what is blocked on whom, and what happened in order.
- Agent-driven CLI calls are told apart from the user's own CLI calls.
- Running `airlock` processes older than the installed version are detected and reported.
- A resolved question's answer can be corrected; the history stays; the asker learns of it.
- `airlock activity` prints the log in the terminal (and as JSON), so validation evidence can be gathered from it.

## Non-goals

- **No acknowledging or dismissing problems** in the dashboard (decided by the user: "bleiben stehen erstmal"). Problems disappear only when their entries leave the retention window. The dashboard stays read-only.
- No blocking of the CLI for clean-room sessions. Airlock makes the CLI route visible; whether a clean-room session may create tasks is a separate decision.
- No finer session identity for several harness instances sharing one session id (validation run 3, problem 1). The user does not consider it critical: the shared `lead-1` sessions are leads watching different aspects.
- No reading of harness transcripts; Airlock logs only what passes through Airlock.
- No alerts outside the dashboard (notifications, mail, chat).

## Anti-goals (would make this a failure even if it works)

- **Analysis material leaking through the log.** Answers, notes, `do` texts and question texts can carry material from the analysis side. The log must not copy free-text bodies; it records the command, ids, the names of the fields changed, and the result or error.
- The Problems panel staying calm while an agent was refused or wrote through the CLI from a clean-room binding.
- Logging that slows or breaks the operation it records. A failure to write the log entry must never fail the operation itself.
- A correction silently replacing the old answer: the earlier text must stay retrievable.

## What gets recorded

One entry per operation, written by the code path every channel already shares, so MCP, CLI and REST cannot drift apart.

| Field | Notes |
|---|---|
| `at` | unix seconds (ms precision is fine if cheap) |
| `session` | the acting session id (`engine-1`, `lead-1`, `human:peter`, `dashboard`) |
| `role` | bound or given role, if any |
| `clean_room` | whether that role is clean-room |
| `channel` | `mcp`, `cli`, `rest` |
| `agent_shell` | CLI only: true when the process runs inside an agent's shell (see below) |
| `command` | the operation name: MCP tool name (`task_claim`), CLI subcommand path (`task edit`), REST method and route |
| `target` | task id, question id, track name, or role name, if the operation has one |
| `changed` | for writes: the names of the fields changed (`track`, `priority`, `blocked_by`), never their text values; short enumerated values may be included (`priority: high`, `track: core`, `lease_minutes: 120`) |
| `ok` | success or failure |
| `error` | the exact error message on failure |
| `pid`, `version` | process id and Airlock version of the caller |

Reads (`task_get`, `question_list`, `task list`, dashboard polling) are recorded too: the user wants to see *how* sessions work. Dashboard reads (its own polling, SSE) are not recorded, otherwise they would flood the log; REST writes and REST calls from other clients are.

**Which failures are logged** (my call): every failure once the project is resolved, including an MCP call to a tool hidden from a clean-room role (the most important refusal), unknown tools, invalid arguments, and CLI errors after argument parsing. Failures before a project is known (clap usage errors, no marker or binding found, malformed JSON-RPC, REST rejected by the Host/Origin/auth checks) have nowhere to go and are not logged. `error` stores the message verbatim, cut to 300 characters: Airlock's error messages name ids, roles, tracks, fields and allowed values, not free-text bodies.

**REST identity** (my call): the dashboard's own JS sends `X-Airlock-Client: dashboard`; its reads are not logged, its writes (answer, release) are, as session `dashboard`. Other REST callers are logged as session `rest` with no role. REST has no role binding, so rule 2 below does not apply to REST.

**Role filter for entries without a role** (my call): with the dashboard's role filter set, entries and problems without a role (human CLI, unbound MCP, REST) are hidden; with no filter they show.

### Telling agents from the user on the CLI

A CLI call inherits its session from the worktree binding, so `engine-1` running `airlock task add` could be the engine agent or the user in that directory. Agent harnesses set environment variables in their shells (Claude Code sets `CLAUDECODE=1` and `AI_AGENT=claude-code_…`). When one of these is present, the entry gets `agent_shell: true` and the agent name. The builder verifies which variables Claude Code, DSH and Antigravity set and lists the recognised ones in the spec; unknown harnesses simply show as plain CLI.

### Running processes and versions

Every long-running process (`airlock mcp`, `airlock serve`) registers itself on start in the hub store with pid, version, session, role and start time, and refreshes a `last_seen` stamp while alive. On the local machine liveness is checked by pid. A live process whose version is older than the newest version known — the newest registered one or the version of the process doing the check (CLI, `serve`), whichever is newer — is a problem ("old airlock process: PID 3855358, lead-1, started Sat 22:04, version …"). Registrations of dead processes are cleaned up. The registry is per project; an `airlock mcp` that resolves no project does not register. Processes started before this change never register, so the first upgrade cannot see them; that is accepted (my call). The new store migration makes older binaries refuse the store, as with every migration: restart running sessions after installing.

### Retention

Entries older than 30 days are deleted (my call; a config value is fine). Pruning happens on write, not on a timer.

## What counts as a problem

Problems are log entries (or process registrations) matching one of these rules; they are computed, not stored separately:

1. **Refused or failed operation**: any entry with `ok = false`, with its exact error.
2. **Clean-room write outside the clean-room tool set**: a CLI write while the session's role is clean-room that the clean-room MCP set does not offer. Flagged: `task add`, `task edit`, `task remove`, `task approve` / `reject`, `track add|edit|remove`, `question answer`, `question resolve`, `sync`, `import`, `role set|remove`, `use`. Not flagged: `task claim`, `heartbeat`, `release`, `done`, `question ask`, and all reads. The problem fires whether or not `agent_shell` is set (my call): an unknown harness must not keep the panel calm; the badge only says who it probably was.
3. **Error burst**: five or more failures by one session within ten minutes, shown as one problem (thresholds my call).
4. **Answer corrected**: a correction of an answered or resolved question (below). Not a fault, but the user wants to see it. The log entry of the answer call carries `changed: correction`, which is what the rule matches.
5. **Old process**: a live registered process with an older version.

## Answer corrections

- `question_answer` (MCP), `airlock question answer` (CLI) and the REST answer endpoint accept a question that is already answered or resolved. On such a question the call is a **correction**: the new text becomes the answer, the status stays as it was (a resolved question stays resolved), and `spec_file` may be updated.
- The previous answer is kept in an answer history with its text, author session and time. Every earlier version stays retrievable (`question_get` / `airlock question show` list them).
- The question carries `corrected_at` and `corrected_by`.
- **Briefing:** the asking session's next briefing lists corrected questions under a heading "Answer changed" with the new answer. "Next" is taken from the log, so the briefing stays a read (my call): a correction appears in every briefing of the asking session (`session_start`, the `start` prompt, `airlock start`) until that session has had one briefing after the correction — that is, while the correction is newer than the session's latest logged briefing before the current one. Several instances sharing one session id share that one notice; accepted, as with the shared session itself. A question still `answered` (not resolved) that gets corrected appears among the answered questions as today, with the new answer and a "corrected" marker, not twice. Clean-room sessions see corrections of their own questions only, as today with answers.
- **Generated `QUESTIONS.md`:** a corrected question shows the current answer, marked "corrected <date>", with the earlier answer(s) below it.
- **Dashboard:** the Questions inbox marks corrected questions; the correction also appears in the Activity tab as a problem (rule 4).
- **Snapshot (Spec 05):** the answer history travels with the question; the format version goes up by one, and older snapshots import without history.
- Who may correct: whoever may answer (not clean-room sessions).

## Dashboard: the Activity tab

Agreed with the user from mockups (combination of three variants; the logbook in the centre). Read-only, follows the dashboard's role filter like the other tabs.

**Top strip, two panels side by side:**

- **Problems** (left): every problem in the retention window, newest first. Repeats of the same rule by the same session on the same command are collapsed into one line with a count and the latest time ("engine-1 (clean-room) writes through CLI: task add, task edit · 2×"), so the panel stays short although nothing is dismissed (my call). Clicking a problem filters the logbook to its entries.
- **State** (right): who is working on what now (claimed tasks with session, track and lease minutes left) and **blockers**: blocked tasks with the open prerequisites they wait on and who holds those ("E-76, E-77 ← L-42, L-43 · lead, nobody on it").

**Session bar:** one chip per session seen in the window: session, role, number of calls per channel (MCP / CLI / REST), time since its last call; clicking a chip filters the logbook to that session, "all" clears it.

**Logbook:** chronological, newest first, grouped by day. Each row: time, session, channel badge, command with target and the short `changed` summary; failures red with the error, problem rows highlighted. Filters: all · writes only · failures and problems only · MCP · CLI · REST. Live via SSE (a new `activity` event).

## CLI, MCP, REST

- `airlock activity [--session <s>] [--role <r>] [--since <duration|time>] [--problems] [--writes] [--json]`: the logbook in the terminal; `--problems` prints the problem list.
- REST: `GET /api/activity` (same filters as query parameters) and `GET /api/activity/problems`; SSE event `activity`.
- MCP: no activity tool in this change. The log is for the user; agents do not need it, and clean-room sessions must not get it.

## Documentation

- Specs 01 (log, process registry, answer history), 02 (`airlock activity`, correction on `question answer`), 03 (correction semantics of `question_answer`, briefing section), 04 (Activity tab, endpoints, SSE), 05 (snapshot version, `QUESTIONS.md` format), 06 (briefing "Answer changed").
- `docs/walkthroughs/agent-validation.md`: note that later runs take their evidence from `airlock activity`.
- After merge: reinstall the binary and update the OpenSHR agent docs (how to correct an answer; that every call is logged).

## Left to the builder

- Table layout, indices, how pruning and the process registry are implemented, and whether the log lives in the hub store or a separate database file.
- The exact `changed` summaries per command, within the no-free-text rule.
- Thresholds and grouping keys for error bursts and collapsed problems.
- Styling within the SPECTRUM tokens, empty states, narrow layout.
- Help texts and error wording.
