# Airlock User Guide

Airlock coordinates several AI agent and human sessions working on one project — often in several git worktrees, sometimes across a clean-room boundary — through one shared store of tasks, questions, and leases.

## Where to start

| You want to … | Read |
|---|---|
| try Airlock in ten minutes | [Quickstart](../walkthroughs/quickstart.md) |
| understand roles, sessions, leases, tracks, the inbox | [Concepts](concepts.md) |
| run a lead and a clean-room engine with Claude Code, unattended | [Walkthrough: a clean-room project with Claude Code](../walkthroughs/clean-room-with-claude-code.md) |
| run several developer agents on one codebase without collisions | [Walkthrough: several agents on one codebase](../walkthroughs/parallel-agents.md) |
| fix an error message or an agent that misbehaves; upgrade | [Troubleshooting](troubleshooting.md) |
| look up every command, tool, and rule exactly | [Specifications](../spec/README.md) |

## Commands at a glance

```bash
airlock init [--template clean-room|parallel|single]   # register the project, worktrees, bindings
airlock use <role> [--lock]                             # bind a worktree
airlock setup claude|claude-hooks|antigravity|dsh       # agent configuration for this worktree
airlock doctor                                          # check everything

airlock start                                           # the session briefing
airlock task next | claim | heartbeat | done | release  # work on a package
airlock inbox [--wait]                                  # what changed for my role
airlock question ask | answer | resolve | list          # across the airlock

airlock task add | edit | hold | resume | approve | reject
airlock track add | edit | list
airlock role set | list

airlock sync | import                                   # snapshot and views for git
airlock serve                                           # dashboard on 127.0.0.1
airlock activity [--problems]                           # what happened, what went wrong
```

Every command takes `-C <dir>` (act as if started there) and `--json`. `airlock <command> --help` lists the options.

## Agent harnesses

Claude Code is the harness Airlock has been validated with most, including the SessionStart and Stop hooks. Antigravity and DSH connect through `airlock setup antigravity` and `airlock setup dsh`; see [Spec 06 §5](../spec/06_onboarding.md) for their configuration.
