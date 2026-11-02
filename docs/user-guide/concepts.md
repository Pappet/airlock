# Concepts

What Airlock keeps track of and the words the rest of the documentation uses. Each section links to the spec that defines the details.

---

## Project and hub

A **project** is one git repository with all its worktrees. Its state lives in the **hub**, outside the repository: one SQLite database per project under the platform state directory (`~/.local/state/airlock/projects/<uuid>/` on Linux; [Spec 01 §3.1](../spec/01_architecture_and_data_model.md) lists macOS and Windows; override with `AIRLOCK_HOME`). The repository only carries a one-line marker, `.airlock`, with the project id. Every worktree finds the same hub entry through the marker, so a claim made in one worktree is seen in all others at once — there is nothing to merge.

`airlock.toml` (committed) records which worktree hosts which roles, so `airlock init` can recreate worktrees and bindings on another machine.

→ [Spec 01 §3](../spec/01_architecture_and_data_model.md), [Spec 05](../spec/05_git_sync_and_airlock.md)

## Roles

A **role** is a kind of work: `lead`, `spec`, `engine`, `dev`, … Every task belongs to one role. A role carries what a session of that role should know before it starts:

- a short description and **instructions** (rules shown in every briefing),
- a **reading list** (`--read CLAUDE.md --read docs/PROCESS.md`),
- a task id **prefix** (`E` for `E-1`, `E-2`, …),
- the **clean-room** flag.

A **coordinating** role (every role that is not clean-room) plans and answers: it creates tasks, answers questions, reviews, and hears about everything that needs a reply. A **clean-room** role only implements: through MCP it sees a restricted tool set, only its own role's tasks, and only its own questions.

```bash
airlock role list
airlock role set engine --instructions "Never read decompiled output." --read CLAUDE.md
```

→ [Spec 06 §4](../spec/06_onboarding.md)

## Sessions and bindings

A **session** is one agent (or human) working under one id, for example `engine@tidewater-engine` or `lead-1`. Claims, questions, answers, and the inbox belong to the session. A restarted agent that comes back with the same session id still holds its claims.

A **binding** ties a worktree to a role and a session id (`airlock use engine --lock`). Everything started in that worktree — CLI, MCP server, hooks — then acts as that session without further configuration. A **locked** binding refuses other roles; use it for clean-room worktrees.

A checkout can be shared by two roles (lead and spec in the main checkout). The one that is not bound starts its harness with its identity in the environment:

```bash
AIRLOCK_SESSION=lead-1 AIRLOCK_ROLE=lead claude
```

The MCP server, the CLI, and the Claude Code hooks all read these variables before the binding.

→ [Spec 06 §2](../spec/06_onboarding.md)

## Tasks (work packages)

A **task** is one package of work, written so that an agent can do it without asking:

| Field | Meaning |
|---|---|
| *Read* | files to read first (specs, formats) |
| *Touch* | files or directories the task changes — used to find overlaps |
| *Do* | what to do |
| *Verify* | how to prove it is done (a test, a review) |
| *Blocked by* | tasks that must be done first |

Airlock assigns the id (`E-7`); never pick one yourself.

**Lifecycle:** `open → in_progress → (in_review →) done`. A task's shown status is derived: an `open` task is `ready` when all its prerequisites are done and `blocked` otherwise.

- **Claim** takes a ready task and starts a **lease** (60 minutes by default). Only the lease holder may heartbeat, complete, or release it.
- **Heartbeat** renews the lease. With `--notes` it also stores a **progress note** ("done …; next …") that the next session to take the task reads in its briefing.
- **Done** finishes the task with notes; `--review` sends it to `in_review`, where a coordinating session **approves** or **rejects** it with feedback.
- **Release** gives the task back. A lease that is not renewed expires, and the task becomes claimable again.
- **Hold** parks a task that waits for something outside Airlock (a capture, a decision) with a reason. It keeps its priority, but `task next` passes it over and claims are refused until it is **resumed**. The lease holder can hold its own task; it is released in the same step.

→ [Spec 01 §2.1](../spec/01_architecture_and_data_model.md)

## Tracks and priority

A **track** is a named line of work with its own priority (`Navigation`, `UI`, …). Priorities use one scale everywhere: `critical` > `high` > `normal` (default) > `low`.

`task next` hands out the ready task of the most important track first, then by the task's own priority, then the oldest. A task that touches files of unfinished work comes last; if it is claimed anyway, Airlock records an `overlap:` question for the lead.

→ [Spec 01 §2.5](../spec/01_architecture_and_data_model.md)

## Questions and the airlock

A **question** is how an implementing session asks about a spec instead of guessing: `open → answered → resolved`. The answer names the spec file it was written into (`--spec docs/spec/wind.md`), so the knowledge ends up in the spec, not only in the database. A wrong answer is corrected by answering again; the earlier text is kept.

The **airlock** is the boundary in a clean-room project: analysis material stays on one side, and only behavioral specs, task descriptions, and prose answers cross to the implementation side. Airlock gives that boundary a structure — restricted tools, own-role views, a recorded channel — but it does not enforce it against a session that ignores it. Hard isolation (separate OS user, container) is the environment's job.

→ [Spec 01 §2.2](../spec/01_architecture_and_data_model.md), [Spec 05 §1](../spec/05_git_sync_and_airlock.md)

## Briefing, inbox, and handoff

The **briefing** is what a session reads first: role, rules, reading list, the task it holds (with its progress note), or the next package, answers to its questions, and the workflow. Agents get it from the MCP tool `session_start`, from `airlock start`, or automatically from the Claude Code SessionStart hook. It stays below about 8 KB; long new answers are then listed by reference.

The **inbox** is what changed for a session's role since it last looked:

- for every role: tasks that became ready or were changed, answers to its questions, rejected reviews;
- for coordinating roles also: new questions, tasks waiting for review, and tasks completed by other roles together with their completion notes.

A session never sees its own changes. Its inbox starts with its first briefing or inbox call; anything older is in the task and question lists.

How news reaches a session without anyone copying text between terminals:

1. **At the task boundary.** `task_complete` (`airlock task done`) returns the inbox and the `next` package. An agent finishes one task, reads what arrived, and continues with the next.
2. **At the end of a turn.** With the opt-in Claude Code Stop hook, a session that ends its turn while news is waiting is sent back to work (at most ten times an hour).
3. **While idle.** A session that has nothing to do and is meant to run unattended calls `inbox_wait` (`airlock inbox --wait`), which blocks for up to four minutes until something arrives. A stopped session is not woken by anything else.

→ [Spec 01 §2.6](../spec/01_architecture_and_data_model.md), [Spec 06 §3, §6](../spec/06_onboarding.md)

## Snapshot and generated files

`airlock sync` writes the project state into the worktree for git: `docs/.airlock-state.json` (the snapshot, the only thing `import` reads) and two read-only views, `docs/PLAN.md` and `docs/spec/QUESTIONS.md`. Session state — claims, leases, holds, inboxes — is never exported. On a merge conflict in a generated file, take either side and run `airlock sync` again.

→ [Spec 05 §3](../spec/05_git_sync_and_airlock.md)

## Activity log

Every MCP, CLI, and REST call is logged next to the store: time, session, role, channel, command, target, and the exact error of a refusal — never task text, notes, questions, or answers. It answers "what did the agents actually do?" and flags problems: refusals, clean-room sessions writing through the CLI, bursts of errors, `airlock mcp` processes older than the installed binary. Entries are kept for 30 days.

```bash
airlock activity --since 2h
airlock activity --problems
```

→ [Spec 01 §3.5](../spec/01_architecture_and_data_model.md)
