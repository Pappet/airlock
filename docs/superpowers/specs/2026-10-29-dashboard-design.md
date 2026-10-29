# Design: Web Dashboard (Phase 4)

Date: 2026-10-29 · Status: draft for approval

## 1. Intent

Airlock coordinates several agent sessions on one project. Two things are hard to see today:

1. **Which task waits for which** (`blocked_by`).
2. **Which tasks touch the same files** (`touch`). Two agents that pick, say, OpenSHR's E-6d and E-6e in parallel both edit `sim.rs` and `behaviour.rs` and get in each other's way. Airlock already knows this from the `touch` lists but shows it nowhere.

Real data (OpenSHR, 2026-10-29): 12 open tasks, only 2 with `blocked_by` (longest chain: 2), but heavy file overlap — `sim.rs` and `drive.rs` are touched by 5 open tasks each, `behaviour.rs` by 4, `frontend.rs` and `regiment.rs` by 3. The overlap is the bigger blind spot; the dependency graph is small.

The dashboard implements Spec 04 in full and adds two views that make dependencies and file overlap visible, so the user can steer which tasks run in parallel.

## 2. Goals

- `airlock serve [--port <port>]` per Spec 02 / Spec 04: loopback-only server with the full security model of Spec 04 §2.
- Everything in Spec 04 §3: Kanban board with role filter, review queue (approve / reject with required feedback), questions inbox (answer, revise, resolve with optional spec file), live leases (with force-release).
- The REST and SSE API of Spec 04 §4.
- **New view A — Stages:** dependency stages as columns, `blocked_by` as arrows, overlap highlighting on selection (§4.2).
- **New view B — File matrix:** files × tasks, showing overlap hot spots (§4.3).
- Live updates in all views when agents claim, complete, or release tasks (SSE).

## 3. Non-goals and anti-goals

**Non-goals (this round):**
- **Overlap warnings at claim time.** `task next` skipping ready tasks that overlap a running task, and `claim` warning when one is taken anyway, are wanted but deferred. Record them as an open item in `docs/plans/PHASE_PLAN.md` (Phase 4 or 5) with a pointer to this design. The overlap computation (§4.1) is built so that this later feature can reuse it unchanged.
- Claiming or completing tasks from the dashboard (Spec 04 §4: session operations stay with CLI and MCP).
- Directory- or module-level overlap. Only identical paths count.
- Multiple projects in one server, remote access, accounts.

**Anti-goals (would be a failure even if it works):**
- Executing a `verify` command anywhere (Spec 04 §2).
- Any listener on a non-loopback interface, any request to an external host (fonts, CDNs, analytics).
- Showing a task without `touch` as "conflict-free". Missing data must look like missing data.
- A frontend build toolchain (Node, bundlers) as a requirement for building Airlock.

## 4. Behaviour decided with the user

### 4.1 Overlap rules

- **Open tasks** are tasks whose effective status is not `done`. Done tasks never take part in overlap.
- Two open tasks **overlap** when their `touch` lists share at least one identical path. Paths are compared after light normalisation (trim, strip a leading `./`, `\` → `/`, collapse repeated `/`) — exact rule is the builder's call, but `src/a.rs` and `./src/a.rs` must match and `src/a.rs` vs `src/` must not.
- A task with an empty `touch` list has **unknown** overlap. It is marked as such (e.g. a "no touch data" badge), never shown as clean.
- An overlap with a task that is `in_progress` or `in_review` is the important case: those files are **occupied** (in_review counts because the work is not yet merged — builder's call, see §6). Such overlaps are emphasised over overlaps between two ready tasks.
- The computation is a pure function in the library (e.g. in `src/core.rs` or a new `src/overlap.rs`) over `&[Task]`, returning per task the overlapping tasks and the shared paths, plus per file the open tasks touching it. The API serves its result; the later claim warning will call the same function.

### 4.2 View A — Stages

- Columns are dependency stages. Stage 1 holds every open task without open prerequisites (status ready, in progress, in review). A blocked task sits in stage `1 + max(stage of its open prerequisites)`. Done prerequisites are ignored.
- `blocked_by` edges are drawn as arrows between cards.
- Cards show id, short title, role, status (running tasks visibly distinct), and an overlap count badge.
- Selecting a card highlights every task it overlaps with and lists the shared paths ("E-6d overlaps 4 tasks via sim.rs, behaviour.rs"). Selecting again or pressing Escape clears it.
- The role filter applies here too.

### 4.3 View B — File matrix

- Rows: files touched by two or more open tasks, sorted by number of tasks (descending), then path.
- Columns: open tasks that touch at least one of these files.
- A cell marks "task touches file". Cells of running tasks (`in_progress`, `in_review`) are distinct so occupied files stand out.
- Hovering or selecting a row or column highlights it. Clicking a task opens the same detail panel as the Kanban card.
- Tasks with no `touch` data are listed separately below the matrix ("no touch data: …").

### 4.4 Rest of Spec 04

As specified in Spec 04 §3 and §4. Kanban cards additionally carry the overlap badge from §4.1 (cheap once the function exists; the user chose views A and B over a Kanban-only warning, so the badge is a secondary cue, not the main view).

## 5. How it gets made

Decided by the existing specs:
- `axum` on `tokio`, loopback-only, token in the URL fragment kept in `sessionStorage`, Host and Origin checks, JSON-only mutations, no CORS (Spec 04 §2).
- Assets embedded in the binary (Spec 04 §1; Phase plan names `rust-embed`, `include_str!`/`include_bytes!` is equally acceptable).
- SSE fed by polling `PRAGMA data_version` once per second (Spec 04 §4).
- The server works on the hub store like any CLI call (Spec 01 §3.2). Same quality bar as every phase: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, MSRV 1.85, `--locked`, CI on Linux/macOS/Windows.

Claude's calls (user may override):
- **Frontend:** plain HTML, CSS, and JavaScript modules, no framework, no build step. Graph arrows drawn as inline SVG.
- **Look:** the user's SPECTRUM house style (dark, monospace data, tonal surfaces instead of borders, accents only for meaning, every colour paired with a text or icon signal). Overlap = error red with a ⚠ marker, running = primary green with a ▶ marker.
- **UI language:** English, like the CLI and docs.
- **New endpoint:** `GET /api/overlaps` returning the result of §4.1 (per task and per file). Spec 04 §4 gets this row, and §3 gets subsections for views A and B in the same change, so spec and code stay in step.
- **Session id for dashboard answers:** `Store::answer` records a session. The dashboard uses a fixed id such as `dashboard`, documented in Spec 04.
- **Project resolution:** `airlock serve` resolves its project like every other command (`-C`, `--project`, marker discovery; Spec 02). It prints the tokenised URL and does not open a browser itself.
- **Error status codes:** store errors are currently `anyhow` strings. The server needs a typed distinction for Spec 04's 400 / 404 / 409; introduce a small error kind in the store rather than matching message text.
- **Force-release** maps to `Store::release(.., force: true)` with the dashboard session id.
- **Empty states:** no open tasks, no overlaps, and no `touch` data at all each get a plain explanatory message instead of an empty grid.

## 6. Left to the builder

- Module layout (`src/serve.rs` or `src/web/`), how `rusqlite` (not `Sync`) is used from async handlers — e.g. open a `Store` per request inside `spawn_blocking`, or one dedicated thread.
- Page layout and navigation between Kanban, Stages, Matrix, Review, Questions, Leases (tabs are fine).
- Whether `in_review` counts as occupied (§4.1 default: yes) — keep it a single, named rule so it is easy to flip.
- Behaviour when the port is taken (clear error is enough).
- Test split: unit tests for overlap and stage computation (including OpenSHR-like fixtures: unknown touch, done prerequisites, occupied files); integration tests against a temporary `AIRLOCK_HOME` for token, Host, Origin, Content-Type checks, each endpoint's status codes (400/404/409), and that no endpoint executes `verify`.
- Docs to update in the same change: Spec 04 (new views, endpoint, dashboard session id), Spec 02 if `serve` flags change, `docs/plans/PHASE_PLAN.md` (Phase 4 items, deferred claim warning), quickstart (how to open the dashboard).
