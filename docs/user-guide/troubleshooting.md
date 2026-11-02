# Troubleshooting

Start with the two commands that know the most:

```bash
airlock doctor                 # installation, marker, worktrees, bindings, MCP configs, hooks
airlock activity --problems    # what went wrong in the last 30 days, collapsed per cause
```

`doctor` prints one line per finding (`ok`, `warn`, `error`) and exits with 1 on any error. `activity --problems` lists refusals, clean-room sessions writing through the CLI, bursts of errors, corrected answers, and `airlock mcp` processes older than the installed binary. `airlock activity --failures --since 2h` shows every refused call with its exact error.

---

## Setup

**`not inside an Airlock project (no .airlock found); run `airlock init` or pass --project`**
The command ran outside the repository, or in a worktree created before `.airlock` was committed. Commit the marker on the main branch and create worktrees after that (they inherit it), or run the command with `-C <dir>`.

**`project … is not registered in this hub; run `airlock init` there to adopt it`**
A fresh clone, or another machine. `airlock init` in the main checkout adopts the marker's project id and recreates worktrees and bindings from `airlock.toml`; `airlock import --yes` then loads the committed snapshot.

**`airlock.toml already exists; edit it instead of applying a template, then run `airlock init``**
Templates only set up new projects. To add a worktree later, add a `[[worktree]]` entry to `airlock.toml`, run `airlock init`, and commit the file.

**`this worktree is locked to role `engine`; `--role lead` is refused`**
The worktree's binding is locked, usually because it is a clean-room worktree. Work as another role in another checkout. Changing a locked binding needs `airlock use <role> --force`; for a clean-room worktree that is almost never what you want. Also check that `AIRLOCK_ROLE` is not set in the shell that started the agent: the MCP server, the CLI, and the hooks all read it.

**Two agents in one checkout complete each other's tasks**
They share a session id: both were started with the same configuration and no `AIRLOCK_SESSION`. Give every running harness its own id, for example `AIRLOCK_SESSION=spec-1 AIRLOCK_ROLE=spec claude` next to the bound role's session.

---

## Agents and MCP

**The agent has no Airlock tools**
Check `airlock doctor`: it reports each `.mcp.json` and whether the command exists. Use the absolute path to `airlock` (what `airlock setup` prints): a harness may start servers with a different `PATH` than your shell. Keep one `.mcp.json` per worktree; `claude mcp add --scope local` applies to every worktree of the repository.

**`… has schema version 8; this airlock supports up to 7`**
A newer `airlock` migrated the store, and an older binary — typically an `airlock mcp` started before the upgrade — opened it. Restart every session after installing a new version. `airlock activity --problems` names old processes still running.

**`this server is bound to role `engine`, not `lead``** / a tool is missing for a clean-room session
Expected. A server bound to a clean-room role exposes only the tools that role needs and only its own tasks and questions. Planning tools (`task_create`, `task_update`, `task_hold`, …) belong to coordinating roles.

**A clean-room session writes through the CLI**
`activity --problems` flags it. Agents that are refused a tool sometimes fall back to `airlock task add` in a shell. The instruction files should say that clean-room sessions write through their MCP tools only; a needed follow-up goes into the completion notes, and the lead creates it.

---

## Tasks

**`task `E-4` is blocked by unfinished prerequisites`**
One of its *Blocked by* tasks is not done. `airlock task show E-4` lists them; `task next` never hands out a blocked task.

**`task `E-3` is on hold (waits for …); a coordinating role resumes it with `task resume``**
The task waits for something outside Airlock. Resume it when the reason is gone: `airlock task resume E-3`.

**`task `E-2` is claimed by engine-1 (lease expires in 42 min)`**
Another session holds it. Wait until it completes or the lease expires (an expired lease frees the task by itself), or, if that session is gone for good, release it: `airlock task release E-2 --force`, or the Leases tab of the dashboard.

**`task `L-61` is in_progress; …` when editing**
Started tasks are only edited with `--force` (`task_update` with `force: true`), so a running agent's package does not change under it. To park a running task, its holder puts it on hold instead.

**A task waits but keeps being handed out**
Put it on hold with the reason rather than lowering its priority; a `low` task is still handed out once nothing else is ready.

**`warning: D-2 touches files of unfinished work …`**
Not an error. The claim succeeded and a question was recorded for the lead (`overlap:D-2`), who decides whether both tasks go on. `task next` hands out such tasks last.

---

## Questions and answers

**The engine guessed instead of asking, or asked through the user**
Make sure its instructions say to ask with `question_ask`, and that the answer goes into the spec (`--spec <file>`). Answers reach the asking session in the result of its next `task_complete`, its inbox, and its next briefing.

**A wrong answer**
Answer the question again, also when it is resolved (`airlock question answer <id> "…" --spec <file>`). The earlier answer is kept, and the asking session sees "Answer changed" in its next briefing.

---

## Claude Code hooks

**The briefing is not in the agent's context after a start or `/compact`**
Check `/hooks` in Claude Code and `airlock doctor`. The SessionStart hook must be in `.claude/settings.local.json` of that worktree; `airlock setup claude-hooks` prints it. If Claude Code shows "Output too large … saved to a file", the briefing exceeded about 10 KB: upgrade to 0.2 or later, which keeps it under 8 KB.

**`/hooks` in a linked worktree lists the hooks twice, or the engine got another role's briefing**
Claude Code also runs the hooks in the main checkout's `.claude/settings.local.json` in sessions of linked worktrees. Hooks written by Airlock 0.2 or later name the worktree as `"$CLAUDE_PROJECT_DIR"`, so both copies serve the worktree they run in. Older hooks with a fixed `-C <path>` brief and nudge the main checkout's session there; `airlock doctor` reports them (an error in a clean-room worktree). Re-run `airlock setup claude-hooks` in each worktree and merge its `hooks` key. If a clean-room session already received another role's briefing, clear it (`/clear`).

**The Stop hook never sends the session back to work**
It only acts when the session ends its turn while news it was not yet told about is waiting, and at most `auto_continue_limit` times an hour (default 10; `0` turns it off). Turns without news are not logged; nudges appear in `airlock activity` as `hook stop` with `nudge` (or `limit`). A session that has already stopped is not woken: a session that must react to others should wait with `inbox_wait` instead of stopping (see the [clean-room walkthrough](../walkthroughs/clean-room-with-claude-code.md#7-let-the-sessions-run-on-their-own)).

**The lead's hook acts as spec (or as nobody)**
In a shared checkout the hooks follow the binding unless the harness was started with `AIRLOCK_SESSION` and `AIRLOCK_ROLE`. Start the session that is not bound as `AIRLOCK_SESSION=lead-1 AIRLOCK_ROLE=lead claude`.

---

## Inbox

**`Nothing new` although something happened**
A session's inbox starts with its first briefing or inbox call; earlier events are not replayed (use `airlock task list` and `airlock question list`). A session also never sees its own changes, and `--peek` shows without marking as read.

---

## Git export

**`docs/.airlock-state.json was not written by this hub (another machine or a manual edit). …`**
The snapshot in the worktree differs from anything this hub exported. Look at it: if it holds work from another machine, `airlock import` adopts it; if it is stale, `airlock sync --force` replaces it.

**A merge conflict in `docs/PLAN.md`, `docs/spec/QUESTIONS.md`, or the snapshot**
Never resolve generated files by hand. Take either side and run `airlock sync` again.

---

## Upgrading

1. Install the new binary and read the release's CHANGELOG entry, especially **Upgrading**.
2. Restart every running session; a store migration makes older binaries refuse the store.
3. Re-run `airlock setup …` where the CHANGELOG says configurations or hooks changed.
4. Run `airlock doctor` and `airlock activity --problems`.
