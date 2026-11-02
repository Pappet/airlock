# Spec 06: Onboarding — Worktree Bindings, Session Briefing, Setup

## 1. Overview & Purpose

Setting up a project used to mean hand-written MCP configurations per worktree and role, a session id repeated in several places (`--session`, `AIRLOCK_SESSION`), and kickoff prompts and rules copied into each agent harness's instruction files. This spec moves that knowledge into Airlock:

- **Worktree bindings** (§2): each worktree knows its role and session. CLI and MCP pick them up, so harness configurations no longer carry role or session.
- **Session briefing** (§3): `airlock start` / MCP `session_start` tell an agent who it is, which rules apply, what it holds, and what to do next.
- **Role management** (§4): roles, their rules, and their reading list are edited with `airlock role`, not by hand.
- **Doctor and setup** (§5, §6): check an installation and print harness configurations.
- **Guided init** (§7): templates that create the roles, worktrees, and bindings of a project in one step, recorded in a committed `airlock.toml`.

Like every Airlock restriction, bindings are guardrails for cooperating agents, not a security boundary (Spec 05 §4).

---

## 2. Worktree Bindings

### 2.1 Storage
Bindings are machine-local and live in the project's hub database (table `bindings`, schema version 3), never in the snapshot:

| root | role | session | locked |
|---|---|---|---|
| `/home/peter/Projects/openshr-engine` | `engine` | `engine@openshr-engine` | yes |
| `/home/peter/Projects/openshr` | `lead` | `lead@openshr` | no |

Roots are canonical absolute paths of project roots (the directory holding the `.airlock` marker). Binding and unbinding run in an immediate transaction that checks the lock and writes together, so concurrent `airlock use` calls in different worktrees cannot lose each other's bindings. Keeping bindings out of the repository means nothing to ignore or commit, nothing for `git clean` to delete, and nothing an agent edits by writing a file in its worktree. Moving a worktree loses its binding; `airlock doctor` reports bindings whose directory is gone.

### 2.2 `airlock use`
```bash
airlock use                      # show this worktree's binding
airlock use <role> [--lock]      # bind this worktree to a role
airlock use --clear              # remove the binding
```
- The role must exist in the project configuration.
- The session id is `--session`/`AIRLOCK_SESSION` if given, otherwise `<role>@<directory name>` (e.g. `engine@openshr-engine`). It stays fixed across restarts, so a restarted agent remains the holder of its leases.
- `--lock` requires a role. It marks the binding as locked: `use` refuses to change or clear it unless `--force` is given, and `airlock mcp --role` refuses a different role. Intended for clean-room worktrees.
- `use` works only inside a project root found through a marker (Spec 02 §2.2), not with `--project` alone.

### 2.3 Effect on Commands
When an invocation's project root has a binding:

| Setting | Precedence |
|---|---|
| Session (CLI and MCP) | `--session` / `AIRLOCK_SESSION` → binding → default (`human:$USER` for the CLI, `mcp-<id>` for MCP) |
| Role of `airlock mcp` | `--role` → binding → unbound |
| Role of `airlock start` and `airlock task next` | `--role` / `AIRLOCK_ROLE` → binding → none (`start`) or error (`task next`) |

A locked binding rejects `airlock mcp --role <other>` and `airlock start --role <other>`. In a shared checkout (lead next to spec), a session working in the role that is *not* bound passes its role and session explicitly — in its MCP configuration, or with `AIRLOCK_ROLE` / `AIRLOCK_SESSION` for the CLI. CLI commands other than `mcp` are not restricted by the role.

### 2.4 `-C <dir>`
The global option `-C <dir>` makes Airlock behave as if started in `<dir>` (like `git -C`): marker discovery, project root, and binding all use that directory, and relative paths such as `import --from` resolve against it. Harnesses whose MCP configuration is global and that cannot set a working directory use `airlock -C <worktree> mcp`.

---

## 3. Session Briefing

### 3.1 Content
A briefing is computed from the live store for one session and role:

- project name, session id, role, whether the role is clean-room;
- the role's description, instructions, and reading list (§4);
- the session's claimed tasks with their work packages and remaining lease time; a work package shows `**Track:** … · **Priority:** …` after its title (only `**Priority:**` for a task without a track), and `- **Progress:** …` when the task has a progress note (Spec 01 §2.4); with a note, the closing line says to continue from where it stopped;
- otherwise the next ready work package of the role (Spec 01 §2.1), with an overlap note when it shares files with running work, and `- **Earlier progress:** …` when an earlier holder left a note;
- tasks of the same role held by *other* sessions, with session and remaining lease time, so a session can tell live work from a lease about to expire;
- the session's own questions that have been answered but not resolved; one whose answer was corrected is marked `(corrected)` and shows the new answer. In the Markdown, an answer is shown in full only until the session has had a briefing after it: once a briefing strictly later than the answer (or its correction) is in the activity log, it shrinks to one line under "Already shown in an earlier briefing" — id, topic, `(corrected)`, and the spec file it was codified in — with the command to read it again (`question_list` / `airlock question show <id>`). Otherwise every open answer would repeat in every briefing and after every compaction, and bury the work package (a briefing of 21 KB on OpenSHR, mostly six old answers). The structured form keeps every answer in full. "Previous briefing" is the session's, whoever asked for it: an `airlock start` run by hand in the agent's bound worktree counts too, as for "Answer changed";
- **Answer changed:** the session's own *resolved* questions whose answer was corrected since its previous briefing, with the new answer. The section is headed `## Answer changed` in the Markdown and is `corrected_answers` in the structured form, and it appears only when there is such a question;
- a fixed workflow reminder: one task at a time, claimed before starting, and after each completion the next one from the completion's `next` (or `task next`) rather than from a plan of its own; read the inbox part of each completion before going on; when nothing is ready and no answer is awaited, wait with `inbox_wait` (`airlock inbox --wait`) instead of ending the turn — only where the role's instructions say the session runs unattended, which stays a project decision, not an Airlock setting; heartbeat during long work, record where you are with heartbeat notes before the context fills, complete when *Verify* passes, ask instead of guessing.

In the structured form, a package carries `progress` when it has a note.

**Which corrections are new.** The briefing stays a read of project state (it only starts the session's inbox cursor if it has none, Spec 01 §2.6). "Since the previous briefing" is taken from the activity log (Spec 01 §3.5): the time of the session's latest *successful* briefing (`session_start`, the `start` prompt, or `airlock start`) before the current one. A resolved question counts when its `corrected_at` is at or after that time; if the session has no earlier briefing in the log, every correction of its resolved questions counts. Times are whole seconds, so a correction made in the same second as the previous briefing is shown rather than lost, and may then appear once more in the next briefing. A briefing that fails does not use up a correction. Several instances sharing one session id share these notices, as they share the session. A correction of a question that is still `answered` needs no extra section: it appears among the answered questions with the `(corrected)` mark. Clean-room sessions see corrections of their own questions only, as they see their own answers. The briefing call itself is logged like any other (a read).

Without a role, the briefing omits the role parts and the next package and says how to bind one.

### 3.2 Delivery
- **CLI:** `airlock start [--role <role>] [--hints cli|mcp]` prints the briefing as Markdown (`--json`: the structured form). Shell-only agents are told to run it first. `--hints mcp` names the MCP tools (`task_claim`) instead of the CLI commands (`airlock task claim <id>`), for an agent that works through MCP but receives the briefing from the CLI, such as the SessionStart hook below.
- **MCP tool `session_start`** (no parameters): `structuredContent` holds the structured briefing; the text content holds the Markdown. Part of the clean-room tool set.
- **MCP prompt `start`:** the server advertises the `prompts` capability with one prompt, `start`, whose single user message is the Markdown briefing followed by "Begin with the claimed or next work package." Clients that map MCP prompts to commands (Claude Code: `/mcp__airlock__start`) make the kickoff a single command.
- **Claude Code SessionStart hook** (optional, from `airlock setup claude-hooks`, §6): Claude Code runs `<bin> -C <root> start --hints mcp` when a session starts, resumes, is cleared, or is compacted, and adds its standard output (the Markdown briefing) to the model's context. The agent no longer has to remember to call `session_start`, and after a compaction the role, rules, and claimed work are back in context without a new session. The hook's `airlock start` is an ordinary CLI call: it is recorded in the activity log as `start` from an agent shell and counts as a briefing for the "Answer changed" section (§3.1), which is intended — the briefing text reaches the model each time.

With this, the agent-facing instruction files of a project (`CLAUDE.md`, `AGENTS.md`, …) no longer need Airlock *setup* knowledge — MCP configuration per role, fixed session ids, `AIRLOCK_SESSION`, kickoff prompts. They keep the project's own rules (process, clean-room boundaries, conventions) and gain a short Airlock section telling agents to start with the briefing and follow the claim/heartbeat/complete workflow; the quickstart provides a ready-made one. Role-specific rules and reading lists move into the role configuration (§4), so the briefing can point each session at them.

---

## 4. Role Management

Roles gain two optional fields, carried in `project.json` and in the snapshot (both omitted when empty, so existing snapshots are unchanged):

| Field | Meaning |
|---|---|
| `instructions` | Short rules for sessions of this role, shown in the briefing |
| `read` | Files every session of this role reads first (e.g. `CLAUDE.md`, `docs/PROCESS.md`) |
| `prefix` | Task id prefix: tasks of the role are numbered `<prefix>-1`, `<prefix>-2`, … (Spec 01 §2.1). Letters and digits; default: the role name's first letter or digit, upper-cased. Unique across roles |

```bash
airlock role list
airlock role set <name> [--description <text>] [--clean-room <true|false>] [--instructions <text>] [--read <file>]... [--prefix <P>]
airlock role remove <name>
```
`role set` creates the role if missing; given `--read` options replace the list, `--clear-read` empties it, `--instructions ""` removes the instructions, and `--prefix ""` returns to the default prefix. A change that leaves two roles with the same prefix is refused (e.g. adding `simulation` next to `spec` needs `--prefix SIM`). A new prefix applies to tasks created afterwards; existing tasks keep their ids (Spec 01 §2.1). `role remove` refuses while tasks or bindings use the role.

---

## 5. `airlock doctor`

Checks an installation and prints one line per finding (`ok`, `warn`, `error`; `--json` for a list). Exits `1` if any error was found.

1. **Binary:** whether `airlock` is found in `PATH`, and whether that file differs from the running binary (stale second installation).
2. **Project:** hub location; inside a project: marker valid and registered.
3. **Marker in git:** whether `.airlock` is tracked (needs `git`; skipped otherwise).
4. **Worktrees:** for every `git worktree`: marker present and of this project, binding (role, session, lock).
5. **Bindings:** roles that no longer exist (error), directories that are gone (warn), the same session id bound to several worktrees (warn). With an `airlock.toml`: bindings whose role the layout does not list for that worktree, and worktrees the layout locks whose binding is not locked (warn; `airlock init` adds the lock).
6. **Harness configurations** mentioning `airlock`: Claude Code (`.mcp.json` in each worktree, local-scope entries in `~/.claude.json`) and Antigravity (`~/.gemini/antigravity/mcp_config.json`). Claude Code keys its local scope by repository, so a local entry applies to *every* worktree; doctor checks it against each one and recommends a `.mcp.json` per worktree instead. Flags are recognized in both forms (`--role lead`, `--role=lead`). Reports:
   - commands that do not exist, and bare command names (`airlock` instead of an absolute path) — clients started from the desktop often have a different `PATH` than the shell, so the absolute path is suggested;
   - global entries without `-C`/`--project` (they only work if the client starts them inside a project);
   - `--role` values contradicting a locked binding;
   - per-directory entries whose `--session` or `--role` differs from the worktree's binding, which makes CLI and MCP act as different sessions in the same worktree — except an entry for another role the layout places in the same worktree (e.g. `lead` in a checkout bound to `spec`), which is intended;
   - configuration files that are not valid JSON;
   - a server defined in both a worktree's `.mcp.json` and Claude Code's local scope (Claude Code reports conflicting scopes);
   - session ids used by several entries. DSH profiles are YAML patches and are only listed, not parsed; `dsh --profile <name> --dump-config` shows the composed result.
7. **Claude Code SessionStart hooks** (§3.2) in each worktree's `.claude/settings.json` and `.claude/settings.local.json` whose command mentions `airlock`. The command is split like a POSIX shell line. Reports a bare command name instead of an absolute path (warn, with the path to use), a command that does not exist (warn), a `-C` pointing at a directory that is not this worktree (warn), `--role` contradicting a locked binding (error), and a settings file that is not valid JSON (error: Claude Code then ignores the file). A worktree with an Airlock `.mcp.json` but no such hook gets an `ok` line naming `airlock setup claude-hooks`: the hook is optional.

Airlock calls `git` here only to inspect the setup; it still never uses git to locate state (Spec 05 §2.2).

---

## 6. `airlock setup <claude|claude-hooks|antigravity|dsh>`

Prints the configuration for the current worktree; it never edits harness files. Standard output carries only the file content (valid JSON or YAML), so redirecting it into the target file is always safe; where to save it goes to standard error. The command path is the running binary's canonical path.

- **claude:** a `.mcp.json` for this worktree. Role and session come from the binding (`args: ["mcp"]`) unless `--role` names another role. Claude Code's local scope (`claude mcp add --scope local`) is not used: it applies to every worktree of the repository and would carry one worktree's role into the others.
- **claude-hooks:** a complete `.claude/settings.local.json` whose only key is `hooks`: one `SessionStart` group with the matcher `startup|resume|clear|compact` and one command hook, `<bin> -C <root> start --hints mcp`, timeout 15 seconds (`--hints mcp`: Claude Code works through the MCP server, so the briefing names its tools, §3.2). Binary and root are shell-quoted, so paths with spaces work; `-C` keeps the hook working when Claude Code is started from a subdirectory. Role and session come from the binding; with `--role` naming another role, `--role <role> --session <role>@<dir>` is appended — the same identity `setup claude --role` gives the MCP server. The hint says to save the file if it does not exist and otherwise to merge the `hooks` key into it, and that the file is local to the worktree and not committed. `setup claude` does not include the hook: `.mcp.json` cannot carry hooks, and an existing settings file has to be merged, not overwritten. The hook reads the binding when it runs, not when the session started: in a shared checkout that is rebound between sessions (lead and spec taking turns), a running session's hook would brief the role bound *now*. Install it only where the role stays fixed, typically a locked worktree such as a clean-room engine worktree. With `--auto-continue` (§6.1) the file also carries a `Stop` group with the command hook `<bin> -C <root> hook stop`, timeout 15 seconds; without it nothing changes, so auto-continue is opt-in per worktree. `--auto-continue` needs the worktree bound to the role (the hook reads the binding's inbox) and is refused with any other harness. `--json` carries `role`, `session`, `root`, `command` (the binary), `args` (the hook's arguments), and `hint`, like the other harnesses.
- **antigravity:** an `mcpServers` entry `airlock-<role>` with `args: ["-C", "<root>", "mcp", "--role", "<role>", "--session", "<session>"]` — the configuration is global, so it carries the worktree explicitly.
- **dsh:** a complete patch overlay file (YAML) with one `@deepseek-ai/dsh-mcp-client` row using `-C <root>`, plus the launch command as a comment: `dsh web --patch <file> --port 0`. One overlay per role keeps one role per DSH instance.

`--role` overrides the binding's role (e.g. to print the spec entry in a checkout bound to lead).

### 6.1 Auto-continue: the Stop hook

`airlock hook stop` is what the `Stop` hook runs when a Claude Code session ends its turn. It reads the hook JSON from standard input and ignores it (Claude Code's `session_id` is a different id space; `stop_hook_active` needs no special case, see below), resolves the worktree's binding, and **peeks** at the inbox of the binding's session for its role (Spec 01 §2.6). If the inbox holds an item newer than the last nudge for that session and role, it prints

```json
{"decision":"block","reason":"Airlock: 2 new for engine: E-15 ready (critical); answer to #34. Call inbox (`airlock inbox`), then continue."}
```

and Claude Code continues the session with that reason instead of stopping. The reason names references only (ids, priorities, question numbers); the session reads the content through its own `inbox` call, which also marks it read. Otherwise the command prints nothing and the session stops.

- **Once per news.** The newest event a session was nudged about is kept with its inbox cursor; the same items never nudge twice, read or not. A continued session that ends its turn again without anything new therefore stops — this is the loop guard, so `stop_hook_active` needs no handling.
- **Hourly limit.** At most `auto_continue_limit` nudges per session per hour (project setting in the hub's `project.json`, default 10; `0` turns nudging off). The count comes from the activity log; when the limit is reached the hook logs that and lets the session stop.
- **Never a failure.** No binding, an unknown project, a broken store or log: the command prints nothing and exits 0, so a broken hub never traps a session.
- **Shared session ids.** Nudge state belongs to the binding's session id. Two Claude Code sessions in one worktree share it, as they share the session: a nudge for one suppresses the same items for the other. This follows from self-declared session ids (Spec 01 §2.3) and is intended.
- **Logged.** Each nudge is recorded as `hook stop` (a read) with the number of items as target and `nudge` as changed; a nudge refused by the limit is recorded with `limit`. Turns that end without news are not logged.
- **Lead side.** A new question nudges a coordinating session once. If it cannot answer from the specs and decisions, it leaves the question open and says so in its turn; the same question never nudges again.

`airlock doctor` reports for each worktree with Airlock hooks whether auto-continue is on (and warns when the Stop hook's `-C` names another worktree or the worktree has no binding) or off.

---

## 7. Guided Init

### 7.1 Templates
`airlock init --template <name>` defines roles and where they work:

| Template | Roles | Layout |
|---|---|---|
| `default` | lead, spec, engine | none — no worktrees, no bindings (the behavior before templates) |
| `single` | dev | this checkout, bound to `dev` |
| `clean-room` | lead, spec, engine (clean-room, with default instructions) | this checkout: lead, spec · `../<dir>-engine` on branch `engine`: engine, locked |
| `parallel` | lead, dev | this checkout: lead · `../<dir>-dev-<n>` on branch `dev-<n>` for n = 1…`--agents` (default 2): dev |

`<dir>` is the name of the main checkout's directory. On a terminal without `--template` and `--yes`, init asks for the template (and the agent count for `parallel`), re-asking on invalid input, and shows the plan for confirmation:

- **new project:** all four templates, default `default`;
- **known project without `airlock.toml`:** `none` (the default — nothing changes), `single`, `clean-room`, `parallel`. This is how an existing setup gets a layout;
- **known project with a layout, or a fresh clone:** no wizard; init applies `airlock.toml`.

### 7.2 Layout File
The layout is committed as `airlock.toml` in the main checkout. It says only which worktree hosts which roles; roles themselves are defined in the project configuration (§4):

```toml
[[worktree]]
path = "."
roles = ["lead", "spec"]

[[worktree]]
path = "../game-engine"
branch = "engine"
roles = ["engine"]
lock = true
```

Paths are relative to the main checkout; `.` is required. The first role of a worktree is bound; `branch` defaults to the directory name; `lock` locks the binding.

### 7.3 Behavior
Init is idempotent and runs in the main checkout:

1. **Register** a new project (template roles) or **adopt** an existing marker on a machine whose hub does not know it (default roles plus placeholders for roles the layout names; `airlock import` brings their definitions). In a known project, a template only **adds missing roles**; existing roles are not changed.
2. **Write and commit** a new layout. Only a new layout causes a commit: `.airlock` (if just created) and `airlock.toml`, committed by path so other staged changes stay untouched. Worktrees branch off after this commit and inherit both files. Plain `airlock init` without a layout never commits; it reminds the user to commit `.airlock`.
3. **Apply** the layout (also on every later `init`): for each entry, create the worktree if its path does not exist — checking out an existing local branch or the single remote branch of that name (fresh clone), otherwise creating the branch from `HEAD` — and bind it if it has no binding yet. Existing bindings keep their role and session; the only change init makes to them is adding the lock the layout requires, when the bound role is one the layout lists for that worktree. A binding to a role the layout does not list is reported, not changed. A path that exists but is not a worktree of this repository, or a worktree without the marker (merge the main branch into it), is skipped with a message.

Templates that create worktrees need a git repository with at least one commit. Without a terminal, every template that writes a layout requires `--yes`, whether or not it commits (it registers the project, writes `airlock.toml`, and binds worktrees). Applying a template to a project that already has `airlock.toml` is refused — edit the file and run `airlock init` instead.

### 7.4 Typical Flows
- **New project:** `git init && git commit --allow-empty -m init && airlock init` → choose template → done; then `airlock setup <harness>` per worktree.
- **Existing project with worktrees** (e.g. OpenSHR): `airlock init --template clean-room --yes` in the main checkout writes the layout, finds `../<dir>-engine`, and binds both worktrees.
- **Another machine:** clone, `airlock init` (adopts the id, recreates worktrees and bindings from `airlock.toml`), `airlock import --yes`.
