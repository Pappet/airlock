# Walkthrough: A Clean-Room Project with Claude Code

This walkthrough sets up a clean-room project the way Airlock is used on a real reimplementation project: a **lead** and a **spec** session share the main checkout, and an **engine** session implements in a separate, locked worktree that never sees the analysis material. By the end, the lead and the engine hand work and answers back and forth through Airlock while you watch, without copying text between terminals.

The example project is `tidewater`, a sailing simulation. Command output is shown as Airlock prints it; paths are shortened.

**You need:** a git repository with at least one commit, Claude Code, and Airlock 0.2 or later.

```
~/Projects/tidewater          main checkout   lead + spec   may read analysis/ material
~/Projects/tidewater-engine   engine branch   engine        clean-room: specs and formats only
                     \                                  /
                      \---- one Airlock hub (tasks, questions, inbox) ----/
```

---

## 1. Install Airlock

Download the archive for your platform from the [latest release](https://github.com/Pappet/airlock/releases/latest), check it against its `.sha256` file, and put `airlock` on your `PATH`. Or build it:

```bash
cargo install --locked --git https://github.com/Pappet/airlock
airlock --version
```

---

## 2. Create the project

```bash
cd ~/Projects/tidewater
airlock init --template clean-room --yes
```

```
registered ~/Projects/tidewater  project tidewater (359e1ba4-…)
wrote      ~/Projects/tidewater/airlock.toml
committed  ~/Projects/tidewater  .airlock, airlock.toml
bound      ~/Projects/tidewater  role lead, session lead@tidewater
worktree   ~/Projects/tidewater-engine  created on branch engine
bound      ~/Projects/tidewater-engine  role engine, session engine@tidewater-engine (locked)
Next: run `airlock setup claude|antigravity|dsh` in each worktree; agents begin with `session_start` (MCP) or `airlock start`.
```

The template created three roles (`lead`, `spec`, `engine`), committed only `.airlock` and `airlock.toml`, created the engine worktree on its own branch, and bound both checkouts. The engine binding is locked: nothing started there can act as another role.

Check the result at any time:

```bash
airlock doctor
```

---

## 3. Give the roles their rules

The briefing shows each role's instructions and reading list, so the rules live in Airlock, not in every prompt. The template already gave the engine a clean-room instruction; add the project's own files:

```bash
airlock role set engine --read CLAUDE.md --read docs/PROCESS.md
airlock role set lead --instructions "Plan the work, answer the engine's questions in the specs, review." \
  --read CLAUDE.md --read docs/PROCESS.md
```

Your instruction files (`CLAUDE.md`, `docs/PROCESS.md`) describe the *project*: its process, the clean-room boundary, conventions. They need no Airlock setup knowledge — no session ids, no MCP configuration. Add the coordination section from the [quickstart](quickstart.md#instructing-agents) to `CLAUDE.md`, plus whatever is specific to your project, for example:

```markdown
- **Main checkout:** specs (`docs/spec`) and formats (`docs/formats`). Analysis material in
  `analysis/` may be read here.
- **Engine worktree:** engine code only, built from `docs/spec` and `docs/formats`. Never read
  `analysis/`, decompiled output, or disassembly there.
- Don't paste one session's output into another: answers, tasks, and review feedback reach
  the other side through Airlock. A needed follow-up goes into the completion notes.
```

---

## 4. Plan the first work

Tracks order the work; tasks are the packages. Only coordinating roles create them.

```bash
airlock track add Navigation --priority high --description "Tides, wind and sail handling"
airlock track add Harbour --priority normal

airlock task add "Wind spec" --role spec --track Navigation \
  --touch docs/spec/wind.md --do "Describe base wind and gusts" --verify "lead review"
airlock task add "Wind model" --role engine --track Navigation --priority high \
  --read docs/spec/wind.md --touch src/sim/wind.rs \
  --do "Base wind plus gusts as specified" --verify "cargo test wind" --blocked-by S-1
airlock task add "Harbour map loader" --role engine --track Harbour \
  --read docs/formats/map.md --touch src/map/ \
  --do "Load harbour maps" --verify "cargo test map"
airlock task list
```

```
ID   STATUS   ROLE    PRIO    TRACK       CLAIMED BY  TITLE
E-1  blocked  engine  high    Navigation              Wind model
E-2  ready    engine  normal  Harbour                 Harbour map loader
S-1  ready    spec    normal  Navigation              Wind spec
```

`E-1` waits for the spec; `E-2` can start now. Write *Touch* lists even when they are rough — a directory such as `src/map/` is enough — because overlaps between running tasks are found from them.

---

## 5. Connect Claude Code

### The engine worktree

```bash
cd ~/Projects/tidewater-engine
airlock setup claude > .mcp.json
mkdir -p .claude
airlock setup claude-hooks --auto-continue > .claude/settings.local.json   # merge if the file exists
```

- `.mcp.json` runs `airlock mcp` with no arguments: role and session come from the binding. Keep one `.mcp.json` per worktree; don't use `claude mcp add --scope local`, which Claude Code applies to every worktree of the repository.
- The hooks file adds two Claude Code hooks. **SessionStart** runs `airlock start` when a session starts, resumes, is cleared, or is compacted, so the briefing — role, rules, held task, progress note — is always in context. **Stop** (from `--auto-continue`) sends the session back to work when it ends its turn while news is waiting.
- Both hooks name the worktree as `"$CLAUDE_PROJECT_DIR"`, not by path. This matters: Claude Code also runs the main checkout's `.claude/settings.local.json` hooks in sessions of linked worktrees. With a fixed path there, the engine session would receive the main checkout's briefing. `airlock doctor` reports such inherited hooks as an error in a clean-room worktree.

A clean-room worktree should also keep Claude Code itself away from the analysis material. Add deny rules to the same settings file, adjusted to where your material lives:

```json
{
  "permissions": {
    "deny": [
      "Read(**/analysis/**)",
      "Read(//home/me/Projects/tidewater/analysis/**)",
      "Bash(*objdump*)"
    ]
  }
}
```

### The main checkout

Lead and spec take turns in the main checkout, which is bound to `lead`. Give it a plain `airlock mcp` configuration and the hooks as well:

```bash
cd ~/Projects/tidewater
airlock setup claude > .mcp.json
mkdir -p .claude && airlock setup claude-hooks --auto-continue > .claude/settings.local.json
```

Start the bound role normally. Start a session of the *other* role with its identity in the environment; the MCP server, the CLI, and both hooks pick it up:

```bash
claude                                            # lead, from the binding
AIRLOCK_SESSION=spec-1 AIRLOCK_ROLE=spec claude   # a spec session next to it
```

Give every running harness its own session id. Two Claude Code windows started the same way in one checkout share a session — and can complete each other's tasks.

Run `airlock doctor` once more: it checks the MCP configurations, the hooks, and the bindings of every worktree.

---

## 6. A round of work

Start Claude Code in both worktrees. Each session receives its briefing from the hook; the engine's looks like this:

```markdown
# Airlock session briefing

Project **tidewater** · session `engine@tidewater-engine` · role **engine** (clean-room)

## Rules

Implement only from the specifications and documents listed in your work packages. Never read
analysis material (decompiled output, disassembly, the main checkout). If a specification is
unclear, ask a question instead of guessing.

Read first: `CLAUDE.md`, `docs/PROCESS.md`

## Next work package

### E-2 — Harbour map loader

- **Track:** Harbour · **Priority:** normal
- **Read:** `docs/formats/map.md`
- **Touch:** `src/map/`
- **Do:** Load harbour maps
- **Verify:** cargo test map

Claim it with `task_claim` before you start.

## Workflow
…
```

From here the agents work through MCP tools. The table shows what happens, with the CLI equivalent you can run yourself to follow along:

| Who | Does | CLI equivalent |
|---|---|---|
| engine | claims E-2 and starts | `airlock task claim E-2` |
| engine | hits an open point in a spec and asks | `airlock question ask "Do gusts change direction or only strength?" --topic wind.md` |
| lead | sees the question in its inbox and answers it in the spec | `airlock question answer 1 "Strength only; …" --spec docs/spec/wind.md` |
| engine | records progress before its context fills | `airlock task heartbeat E-2 --notes "loader reads tiles; next: quays"` |
| engine | finishes and asks for review | `airlock task done E-2 --notes "maps load; cargo test map: 4 passed" --review` |

The engine's completion returns what arrived meanwhile and its next package:

```
Submitted E-2 for review
Since your last look:
  #1 answered (wind.md): Strength only; direction follows the base wind. (codified in docs/spec/wind.md)
Next: nothing ready for engine
```

The lead's inbox shows the review request; it rejects with feedback:

```bash
airlock inbox
#   E-2 waits for review — Harbour map loader (normal, Harbour)
airlock task reject E-2 --notes "Quays missing: see docs/formats/map.md section 3"
```

The engine hears `E-2 rejected in review`; the feedback is now the task's *Notes* (in its briefing and in `task_get`). It claims E-2 again and completes it. The lead's inbox then carries the completion with its notes:

```
E-2 done — Harbour map loader (normal, Harbour)
    notes: quays added; cargo test map: 6 passed
```

Every answer, package, and review result crossed between the sessions through Airlock. What crosses is what Airlock stores — task fields, questions, answers, notes — so the engine never sees the lead's working context.

### More important work arrives

The lead adds a critical task while the engine is busy:

```bash
airlock task add "Tide tables" --role engine --track Navigation --priority critical \
  --do "Tide tables from the harbour master's data" --verify "cargo test tide"
```

The engine does not drop what it holds. When it completes its current task, the result says:

```
Since your last look:
  E-3 ready — Tide tables (critical, Navigation)
Next: E-3 (critical, Navigation) Tide tables
```

and it continues with E-3.

### Work that waits

If E-3 turns out to need data you have not delivered yet, the lead parks it instead of lowering its priority:

```bash
airlock task hold E-3 --reason "waits for the harbour master's tide data"
airlock task list
#   E-3  ready (held)  engine  critical  Navigation  Tide tables
airlock task resume E-3          # once the data is there
```

A held task is passed over by `task next`, and a claim is refused with the reason. The engine can also hold the task it is working on itself, with the reason; Airlock releases it in the same step and keeps its progress note.

---

## 7. Let the sessions run on their own

Three mechanisms carry news between the sessions; together they let both run for hours while you only check in:

1. **The task boundary.** Every completion returns the inbox and the next package. An engine with a queue of packages just keeps going.
2. **The Stop hook.** When a session ends its turn while news is waiting — an answer arrived during the turn — Claude Code sends it back to work instead of stopping. At most ten nudges an hour per session (`auto_continue_limit` in the project settings).
3. **Waiting instead of stopping.** A session that has already stopped is not woken by anything. The side that must react to the other — usually the lead, which answers the engine's questions — therefore waits instead of stopping: with nothing to do, it calls `inbox_wait`, which returns as soon as something arrives for its role (at most four minutes, then it calls again).

Whether a session waits is the project's decision, not Airlock's: the briefing tells agents to wait only if their instructions say they run unattended. For a lead that should, add to `CLAUDE.md`:

```markdown
- **The lead runs unattended.** When nothing is ready for the lead and it waits for no answer,
  it calls MCP `inbox_wait` instead of ending its turn, and acts on what arrives: answer the
  engine's questions, review tasks in review, create the follow-ups that completion notes ask
  for. After an hour without news (15 empty `inbox_wait` results in a row) it ends its turn
  with a short status.
```

A wait can always be interrupted with Esc. On the real project this setup ran the lead and the engine for two hours with seven packages completed and questions answered within about a minute, without a message from the user.

---

## 8. Get the work into git

Airlock's state is in the hub, not in git. Export it at milestones and before merges, from the main checkout:

```bash
airlock sync
git add docs/.airlock-state.json docs/PLAN.md docs/spec/QUESTIONS.md
git commit -m "airlock sync"
```

`docs/PLAN.md` and `docs/spec/QUESTIONS.md` are generated views; never edit them. If a merge conflicts in one of them, take either side and run `airlock sync` again.

The engine branch gets new specs and answers by merging `main`; the main checkout gets the engine's code by merging `engine`. Keep the analysis material out of the repository (or out of the branches the engine merges), since the boundary is only as good as what the engine's checkout contains.

On another machine, clone the repository and run `airlock init` (it adopts the project id and recreates worktrees and bindings from `airlock.toml`), then `airlock import --yes` to load the committed snapshot.

---

## 9. Watch what happens

```bash
airlock serve                  # dashboard on 127.0.0.1; open the printed URL
airlock activity --since 2h    # every call of the last two hours
airlock activity --problems    # refusals, clean-room CLI writes, old processes, …
```

The dashboard shows tracks, a board, overlaps, the review queue, questions, leases, and the activity log, live. The activity log is the quickest way to see what the agents actually did: every call with session, role, and the exact error of a refusal, but never the text of tasks, notes, questions, or answers.

---

## 10. Upgrade Airlock

1. Install the new binary.
2. Read the release's CHANGELOG entry under **Upgrading**.
3. Restart every running session. A new binary may migrate the store; older `airlock mcp` processes then refuse it, and `airlock activity --problems` lists any that still run.
4. Re-run `airlock setup claude-hooks` where the CHANGELOG says hooks changed, and merge its `hooks` key.
5. Run `airlock doctor`.

When something does not behave as described here, see [Troubleshooting](../user-guide/troubleshooting.md).
