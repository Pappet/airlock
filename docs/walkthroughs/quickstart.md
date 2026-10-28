# Quickstart Guide: Airlock

> **Status:** Airlock is a scaffold. The commands below describe the specified behavior ([`docs/spec/`](../spec/)); until Phase 1 of the [phase plan](../plans/PHASE_PLAN.md) is done, they parse but exit with "not implemented yet".

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
cd ~/Projects/my-project
airlock init
git add .airlock && git commit -m "chore: register project with airlock"
```

This creates the project in the Airlock hub (`~/.local/state/airlock/` on Linux) and writes the marker file `.airlock` containing only the project id. The database never lives inside the repository, so every worktree of the project shares the same state.

On another machine, run `airlock init` in the fresh clone (it adopts the marker's id) and then `airlock import` to load the committed snapshot.

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
export AIRLOCK_SESSION=engine-agent-1

airlock task next --role engine     # show the next ready work package
airlock task claim E-1              # take it (60-minute lease by default)
airlock task heartbeat E-1          # extend the lease during long work
airlock task done E-1 --notes "Window and event loop implemented; cargo test setup passes."
```

Use `airlock task done E-1 --review` to hand the task to a human for approval (`airlock task approve` / `airlock task reject --notes ...`). If a session crashes, its lease expires and the task becomes claimable again.

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
