# Walkthrough: Several Agents on One Codebase

This walkthrough runs a lead and two developer agents on the same codebase, each in its own git worktree. There is no clean-room boundary here; the problem Airlock solves is a different one: agents that pick the same work, edit the same files, or wait for each other without knowing it.

The example project is `shop`. Command output is shown as Airlock prints it; paths are shortened.

---

## 1. Create the project

```bash
cd ~/Projects/shop
airlock init --template parallel --agents 2 --yes
```

```
registered ~/Projects/shop  project shop (ffa16f4a-…)
committed  ~/Projects/shop  .airlock, airlock.toml
bound      ~/Projects/shop  role lead, session lead@shop
worktree   ~/Projects/shop-dev-1  created on branch dev-1
bound      ~/Projects/shop-dev-1  role dev, session dev@shop-dev-1
worktree   ~/Projects/shop-dev-2  created on branch dev-2
bound      ~/Projects/shop-dev-2  role dev, session dev@shop-dev-2
```

Two roles: `lead` plans and reviews in the main checkout; `dev` implements. Both dev worktrees share the role but each has its own session, so their claims never mix. Need a third agent later? Add a worktree entry to `airlock.toml` and run `airlock init`; it only creates what is missing (applying a template again is refused once `airlock.toml` exists):

```toml
[[worktree]]
path = "../shop-dev-3"
branch = "dev-3"
roles = ["dev"]
```

Commit the changed `airlock.toml`, so other machines get the worktree too.

Connect each worktree to its agent (one `.mcp.json` per worktree; role and session come from the binding):

```bash
for d in ~/Projects/shop ~/Projects/shop-dev-1 ~/Projects/shop-dev-2; do
  airlock -C "$d" setup claude > "$d/.mcp.json"
done
```

Optionally add the Claude Code hooks to each worktree, as in the [clean-room walkthrough](clean-room-with-claude-code.md#5-connect-claude-code).

---

## 2. Describe the work with *Touch* lists

The lead (or you) writes the packages. *Touch* names the files or directories a task will change; it is how Airlock keeps parallel agents out of each other's way.

```bash
airlock task add "Cart totals"    --role dev --touch src/cart/             --do "Sum line items with tax" --verify "cargo test cart"
airlock task add "Discount codes" --role dev --touch src/cart/discount.rs  --do "Apply codes"             --verify "cargo test discount"
airlock task add "Login page"     --role dev --touch src/auth/login.rs     --do "Form and validation"     --verify "cargo test login"
```

```
Added D-1 (ready)
Added D-2 (ready)
Added D-3 (ready)
```

A directory such as `src/cart/` overlaps every file beneath it, so a rough *Touch* list is still useful. Tasks without one show "no touch data" in the dashboard: Airlock cannot see their overlaps.

---

## 3. Two agents pick their work

The first agent asks for work and claims it:

```bash
airlock -C ~/Projects/shop-dev-1 task next      # → D-1 Cart totals
airlock -C ~/Projects/shop-dev-1 task claim D-1
```

The second agent asks next. D-2 is older than D-3, but it touches `src/cart/discount.rs`, which lies under D-1's `src/cart/`. `task next` hands out tasks that overlap running work last:

```bash
airlock -C ~/Projects/shop-dev-2 task next      # → D-3 Login page
```

An overlap never blocks a claim. If the second agent takes D-2 anyway, it gets a warning and the lead gets a question:

```
warning: D-2 touches files of unfinished work: src/cart/discount.rs with D-1; recorded as question #1 for the lead
Claimed D-2 for dev@shop-dev-2 (lease: 60 min)
```

```bash
airlock question list
#   #1 [open] overlap:D-2 — asked by dev@shop-dev-2
#     Q: dev@shop-dev-2 claimed D-2 although it touches files of unfinished work:
#        src/cart/discount.rs (D-1, in_progress by dev@shop-dev-1). Continue, or release D-2?
```

The lead answers: continue (the agents will merge carefully), or release D-2 and take it after D-1. In the rest of this walkthrough the second agent took D-3, as `task next` suggested. Each agent's briefing also lists the work other sessions of its role hold, with a reminder to leave it alone:

```
## In progress by other sessions of your role

- **D-1** Cart totals — `dev@shop-dev-1`, lease 60 min left
```

---

## 4. Order, priorities, and the next task

Each agent works on one task at a time and takes the next one from Airlock when it finishes — never from a list of its own, because the lead may have added something more important meanwhile:

```bash
airlock task done D-1 --notes "totals with tax; cargo test cart: 9 passed"
#   Completed D-1
#   Next: D-2 (normal) Discount codes
```

`task next` orders ready work by track priority, then task priority, then age, with overlapping tasks last. Use tracks when there are several lines of work:

```bash
airlock track add Checkout --priority high
airlock task edit D-2 --track Checkout
```

A claim also warns when a more important task is ready, or when the session already holds another task.

---

## 5. Prerequisites and review

Chain tasks that build on each other; a blocked task is never handed out:

```bash
airlock task add "Order confirmation e-mail" --role dev --touch src/mail/ \
  --do "Send the order summary" --verify "cargo test mail" --blocked-by D-1
```

To review before a task counts as done, the agent completes with `--review`; the lead approves or rejects with feedback, and a rejected task goes back to `open` with the feedback as its notes:

```bash
airlock task list --status in_review
airlock task approve D-3
airlock task reject D-2 --notes "Expired codes must be refused"
```

---

## 6. Keep an eye on it

```bash
airlock serve                   # dashboard: board, Stages, Files, Review, Leases, Activity
airlock activity --problems
```

- **Stages** shows what can run now and what waits; selecting a task lists every open task touching the same files.
- **Files** lists the files several open tasks touch; ▶ marks files held by a running task.
- **Leases** shows who holds what and for how long. A crashed agent's lease simply expires, and its task becomes claimable again, with the progress note it left.

Merging the dev branches back is ordinary git; Airlock keeps the agents from colliding before that point but does not merge for you. Export the plan with `airlock sync` when you want it in the repository.
