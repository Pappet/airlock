# Quickstart Guide: Airlock

This walkthrough guides you through setting up Airlock in any existing or new project.

---

## 1. Installation

Build and install Airlock from source:

```bash
git clone https://github.com/Pappet/airlock.git # or local path
cd airlock
cargo install --path .
```

---

## 2. Initializing in a Project

Navigate to your target project repository:

```bash
cd ~/Projects/my-project
airlock init
```

This registers the project in the central Airlock hub store (`~/.local/state/airlock/`) and writes a merge-trivial `.airlock` marker file (project id only) into the project directory. The database never lives inside the repository — every worktree and clone of the project resolves the same state.

---

## 3. Creating Work Packages

Create tasks assigned to specific roles:

```bash
airlock task add E-1 "Initial setup" \
  --role engine \
  --read "docs/spec/setup.md" \
  --touch "src/main.rs" \
  --do "Initialize game window and event loop" \
  --verify "cargo test setup"
```

---

## 4. Working as an Agent

When starting an agent session (or in terminal):

```bash
# Fetch the next ready work package:
airlock task next --role engine

# Claim the package:
airlock task claim E-1 --session "engine-agent-1"

# Complete the package when tests pass:
airlock task done E-1 --notes "Window and event loop implemented and verified."
```

---

## 5. Asking Questions Across the Airlock

When an implementation agent hits an ambiguous specification:

```bash
airlock question ask "Is the front arc angle rounded up or truncated?" --topic "melee.md"
```

The lead session or human user can view and answer:

```bash
airlock question list
airlock question answer 1 "Truncated towards zero as per formula in section 3.2."
```

---

## 6. Exporting to Documentation

Before a merge (or at any milestone), export the project state for git commits:

```bash
airlock sync
```

This writes the canonical JSON snapshot (`docs/.airlock-state.json`, the lossless backup and the only `airlock import` source) plus the human-readable Markdown views `PLAN.md` and `QUESTIONS.md`. If a merge conflicts in a generated file, simply re-run `airlock sync` afterwards instead of resolving by hand.
