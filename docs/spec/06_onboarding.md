# Spec 06: Onboarding — Worktree Bindings, Session Briefing, Setup

## 1. Overview & Purpose

Setting up a project used to mean hand-written MCP configurations per worktree and role, a session id repeated in several places (`--session`, `AIRLOCK_SESSION`), and kickoff prompts and rules copied into each agent harness's instruction files. This spec moves that knowledge into Airlock:

- **Worktree bindings** (§2): each worktree knows its role and session. CLI and MCP pick them up, so harness configurations no longer carry role or session.
- **Session briefing** (§3): `airlock start` / MCP `session_start` tell an agent who it is, which rules apply, what it holds, and what to do next.
- **Role management** (§4): roles, their rules, and their reading list are edited with `airlock role`, not by hand.
- **Doctor and setup** (§5, §6): check an installation and print harness configurations.
- **Guided init** (§7, planned): templates that create the roles, worktrees, and bindings of a project in one step.

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
| `--role` of `airlock task next` | `--role` → binding → error |

A locked binding rejects `airlock mcp --role <other>`. CLI commands other than `mcp` are not restricted by the role.

### 2.4 `-C <dir>`
The global option `-C <dir>` makes Airlock behave as if started in `<dir>` (like `git -C`): marker discovery, project root, and binding all use that directory, and relative paths such as `import --from` resolve against it. Harnesses whose MCP configuration is global and that cannot set a working directory use `airlock -C <worktree> mcp`.

---

## 3. Session Briefing

### 3.1 Content
A briefing is computed from the live store for one session and role:

- project name, session id, role, whether the role is clean-room;
- the role's description, instructions, and reading list (§4);
- the session's claimed tasks with their work packages and remaining lease time;
- otherwise the next ready work package of the role (Spec 01 §2.1);
- the session's own questions that have been answered but not resolved;
- a fixed workflow reminder: claim before starting, heartbeat during long work, complete when *Verify* passes, ask instead of guessing.

Without a role, the briefing omits the role parts and the next package and says how to bind one.

### 3.2 Delivery
- **CLI:** `airlock start` prints the briefing as Markdown (`--json`: the structured form). Shell-only agents are told to run it first.
- **MCP tool `session_start`** (no parameters): `structuredContent` holds the structured briefing; the text content holds the Markdown. Part of the clean-room tool set.
- **MCP prompt `start`:** the server advertises the `prompts` capability with one prompt, `start`, whose single user message is the Markdown briefing followed by "Begin with the claimed or next work package." Clients that map MCP prompts to commands (Claude Code: `/mcp__airlock__start`) make the kickoff a single command.

With this, the agent-facing instruction files of a project (`CLAUDE.md`, `AGENTS.md`, …) need one line: *"Coordinate through Airlock: call `session_start` (or run `airlock start`) before anything else."*

---

## 4. Role Management

Roles gain two optional fields, carried in `project.json` and in the snapshot (both omitted when empty, so existing snapshots are unchanged):

| Field | Meaning |
|---|---|
| `instructions` | Short rules for sessions of this role, shown in the briefing |
| `read` | Files every session of this role reads first (e.g. `CLAUDE.md`, `docs/PROCESS.md`) |

```bash
airlock role list
airlock role set <name> [--description <text>] [--clean-room <true|false>] [--instructions <text>] [--read <file>]...
airlock role remove <name>
```
`role set` creates the role if missing; given `--read` options replace the list, `--clear-read` empties it, and `--instructions ""` removes the instructions. `role remove` refuses while tasks or bindings use the role.

---

## 5. `airlock doctor`

Checks an installation and prints one line per finding (`ok`, `warn`, `error`; `--json` for a list). Exits `1` if any error was found.

1. **Binary:** whether `airlock` is found in `PATH`, and whether that file differs from the running binary (stale second installation).
2. **Project:** hub location; inside a project: marker valid and registered.
3. **Marker in git:** whether `.airlock` is tracked (needs `git`; skipped otherwise).
4. **Worktrees:** for every `git worktree`: marker present and of this project, binding (role, session, lock).
5. **Bindings:** roles that no longer exist (error), directories that are gone (warn), the same session id bound to several worktrees (warn).
6. **Harness configurations** mentioning `airlock`: Claude Code (`.mcp.json` in each worktree, local-scope entries in `~/.claude.json`) and Antigravity (`~/.gemini/antigravity/mcp_config.json`). Flags are recognized in both forms (`--role lead`, `--role=lead`). Reports commands that do not exist, global entries without `-C`/`--project` (they only work if the client starts them inside a project), `--role` values contradicting a locked binding, and session ids used by several entries. DSH profiles are YAML patches and are only listed, not parsed; `dsh --profile <name> --dump-config` shows the composed result.

Airlock calls `git` here only to inspect the setup; it still never uses git to locate state (Spec 05 §2.2).

---

## 6. `airlock setup <claude|antigravity|dsh>`

Prints the configuration for the current worktree; it never edits harness files. The command path is the running binary's canonical path.

- **claude:** the `claude mcp add --scope local airlock -- <bin> mcp` command and the equivalent `.mcp.json` entry. Role and session come from the binding.
- **antigravity:** an `mcpServers` entry `airlock-<role>` with `args: ["-C", "<root>", "mcp", "--role", "<role>", "--session", "<session>"]` — the configuration is global, so it carries the worktree explicitly.
- **dsh:** a complete patch overlay file (YAML) with one `@deepseek-ai/dsh-mcp-client` row using `-C <root>`, plus the launch command as a comment: `dsh web --patch <file> --port 0`. One overlay per role keeps one role per DSH instance.

`--role` overrides the binding's role (e.g. to print the spec entry in a checkout bound to lead).

---

## 7. Guided Init (planned)

`airlock init` gains templates that define roles and their places:

- **clean-room:** `lead` and `spec` in the main checkout, `engine` in a worktree `../<name>-engine` on branch `engine`, clean-room and locked.
- **parallel:** `lead` in the main checkout, one worktree and branch per agent role.
- **single:** one role, no worktrees.

The layout is committed as `airlock.toml` so `init` on another machine recreates worktrees and bindings. Order of operations: register project, commit marker and layout, create worktrees (they inherit the marker), write bindings, print `setup` output per harness. Existing worktrees (`git worktree list`) are offered for role assignment instead of being created. Details follow in a revision of this spec.
