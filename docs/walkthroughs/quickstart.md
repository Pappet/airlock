# Quickstart Guide: Airlock

This walkthrough sets up Airlock in a project and runs through the daily workflow.

---

## 1. Installation

```bash
git clone https://github.com/Pappet/airlock.git
cd airlock
cargo install --path .
```

---

## 2. Initializing a Project

```bash
cd ~/Projects/my-project        # a git repository with at least one commit
airlock init                    # on a terminal: choose a template, confirm the plan
```

Templates (`--template`, see [Spec 06 §7](../spec/06_onboarding.md#7-guided-init)):

| Template | Result |
|---|---|
| `default` | roles lead, spec, engine; no worktrees — commit `.airlock` yourself |
| `single` | one role `dev` in this checkout |
| `clean-room` | lead + spec here, engine in `../my-project-engine` (branch `engine`, locked) |
| `parallel` | lead here, `--agents N` dev worktrees `../my-project-dev-<n>` |

A template commits `.airlock` and `airlock.toml` (nothing else), creates the worktrees, and binds each to its role. `airlock init` is safe to re-run; it only adds what is missing. In scripts, pass `--template … --yes`.

This creates the project in the Airlock hub (`~/.local/state/airlock/` on Linux) and writes the marker file `.airlock` containing only the project id. The database never lives inside the repository, so every worktree of the project shares the same state.

On another machine, run `airlock init` in the fresh clone — it adopts the marker's id and recreates the worktrees and bindings from `airlock.toml` — and then `airlock import --yes` to load the committed snapshot.

### Worktrees and roles by hand

Without a template, create worktrees *after* committing the marker — they inherit it — and bind each one:

```bash
git worktree add -b engine ../my-project-engine
airlock use lead                                   # main checkout
airlock -C ../my-project-engine use engine --lock  # clean-room worktree, role fixed
airlock doctor                                     # checks marker, worktrees, bindings, configs
```

A binding gives the worktree a fixed session id (`engine@my-project-engine`), so CLI and MCP act as the same session and a restarted agent keeps its leases. Roles carry the rules agents see in their briefing:

```bash
airlock role set engine --instructions "Work only from docs/spec; never read decompiled output." \
  --read CLAUDE.md --read docs/PROCESS.md
```

---

## 3. Creating Work Packages

```bash
airlock task add L-1 "Write setup spec" --role spec \
  --touch docs/spec/setup.md \
  --do "Describe window size, title, and event loop behavior" \
  --verify "Spec reviewed by lead"

airlock task add E-1 "Initial setup" --role engine \
  --read docs/spec/setup.md \
  --touch src/main.rs \
  --do "Initialize game window and event loop" \
  --verify "cargo test setup" \
  --blocked-by L-1
```

Prerequisites must exist before a task can reference them. `E-1` stays `blocked` until `L-1` is `done`.

---

## 4. Working as an Agent

```bash
airlock start                       # briefing: role, rules, claimed work, next package
airlock task next                   # the next ready work package of the bound role
airlock task claim E-1              # take it (60-minute lease by default)
airlock task heartbeat E-1          # extend the lease during long work
airlock task done E-1 --notes "Window and event loop implemented; cargo test setup passes."
```

Use `airlock task done E-1 --review` to hand the task to a human for approval (`airlock task approve` / `airlock task reject --notes ...`). If a session crashes, its lease expires and the task becomes claimable again.

### Connecting an AI agent over MCP

Agents use Airlock's MCP tools directly. `airlock setup` prints the configuration for the current worktree:

```bash
cd ../my-project-engine
airlock setup claude        # `claude mcp add …` command and .mcp.json entry
airlock setup antigravity   # entry for the global ~/.gemini/antigravity/mcp_config.json
airlock setup dsh > ~/.dsh/airlock/my-project-engine.yml   # patch overlay; start: dsh web --patch … --port 0
```

Per-directory harnesses (Claude Code) only need `airlock mcp` — role and session come from the binding. Global configurations carry the worktree with `-C <dir>`. For a clean-room role the agent only sees the tools it needs and only its own role's tasks and questions (Spec 03 §2.1).

The agent's first action is the briefing: the MCP tool `session_start`, the MCP prompt `start` (in Claude Code: `/mcp__airlock__start`), or `airlock start`.

### Instructing agents

Your project's instruction files (`CLAUDE.md`, `AGENTS.md`, `GEMINI.md`, …) keep everything that describes *the project*: its process, clean-room boundaries, conventions, and documentation map. What they no longer need is Airlock *setup* knowledge — MCP configuration per role, fixed session ids, `AIRLOCK_SESSION`, or kickoff prompts; bindings and the briefing take care of that. Put role-specific rules and reading lists into the roles, so the briefing shows them:

```bash
airlock role set engine --instructions "Never read decompiled output; work only from docs/spec." \
  --read CLAUDE.md --read docs/PROCESS.md
```

Then add this section to the instruction files (adjust the paths in the last bullet to your exports):

```markdown
## Coordination (Airlock)

Work is coordinated through Airlock; this checkout is bound to your role and session.

- **Start every session with the briefing:** call the MCP tool `session_start`
  (or run `airlock start`). It tells you your role, the rules and files to read,
  the task you already hold, or the next work package.
- **One task at a time:** claim it before you start (`task_claim` /
  `airlock task claim <id>`). Work only on what its *Do* says and touch only
  the files it lists.
- **Keep your lease alive:** during long work, renew it with `task_heartbeat`
  (`airlock task heartbeat <id>`); an expired lease lets another session take the task.
- **Finish only when *Verify* passes:** complete with `task_complete`
  (`airlock task done <id> --notes "…"`) and say in the notes what you changed
  and how you verified it. If you cannot finish, release the task
  (`task_release` / `airlock task release <id>`) and explain why in a question.
- **Ask instead of guessing:** if a specification is unclear or incomplete, ask
  with `question_ask` (`airlock question ask "…" --topic <spec>`). Answers to your
  questions appear in the next briefing.
- **Do not change the setup:** never run `airlock use`, `airlock init`, or
  `airlock import`, and never edit `.airlock` or `airlock.toml`.
- **Generated files are read-only:** `docs/PLAN.md`, `docs/spec/QUESTIONS.md`, and
  `docs/.airlock-state.json` are written by `airlock sync`; never edit them by hand.
```

The section is the same for every role; what differs per role comes from the briefing.

---

## 5. Asking Questions Across the Airlock

When an implementation session hits an ambiguous spec:

```bash
airlock question ask "Is the front arc angle rounded up or truncated?" --topic melee.md
```

The lead session or a human answers, then resolves the question once the spec is updated:

```bash
airlock question list --status open
airlock question answer 1 "Truncated towards zero, as in section 3.2."
airlock question resolve 1 --spec docs/spec/melee.md
```

---

## 6. Exporting to Git

Before a merge or at a milestone:

```bash
airlock sync
git add docs/.airlock-state.json docs/PLAN.md docs/spec/QUESTIONS.md
```

This writes the canonical snapshot and the Markdown views into the current worktree. If a merge conflicts in a generated file, take either side and re-run `airlock sync` — never resolve generated files by hand.
