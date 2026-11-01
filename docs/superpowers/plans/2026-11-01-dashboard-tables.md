# Dashboard Tracks and Activity Tables Implementation Plan (concept step 3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the Tracks rows and the Activity panels from coloured prose into aligned, line-free tables that follow the concept's colour budget.

**Architecture:** `tracksView()`, `problemRow()`, `statePanel()`, `waitingOn()`, `logRow()` in `src/web/app.js` emit grid rows with one cell per fact; `src/web/style.css` drops every `border-top`/`border-left` in the Tracks and Activity blocks and uses tonal zebra rows instead. Data, filters and the logbook behaviour stay as they are.

**Tech Stack:** vanilla JS, CSS grid. No Rust changes.

**Spec:** `docs/superpowers/specs/2026-11-01-dashboard-visual-concept.md` §2.3 (colour budget), §2.4 (surfaces), §3 Tracks, §3 Activity.

## Global Constraints

- After this step the stylesheet contains no `border-top: 1px solid` and no `border-left: 3px` (the broad probe deferred from step 2).
- `nobody on it` is `on-surface-dim`; tertiary only when a blocked task sits in a `high` or `critical` track.
- Session names secondary; `✕` error for severe problems (`failure`, `error_burst`), `▲` tertiary for the rest; the Problems panel's label turns `error` when it has entries, no side stripe.
- Keep the probes the suite already has: `courseWarnings` in `app.js`, `.ch--rest` and `.p-critical` in `style.css`.
- `node --check src/web/app.js` before every commit; `cargo test`, `cargo clippy`, `cargo fmt --check` before the PR; browser check against openshr at 1376px and headless at 960px.

## Review Focus

1. Track with a course warning and nothing running: warning line in tertiary above `idle`, no second colour (Task 1, browser).
2. Track without tasks: `no tasks` dimmed, bar empty but present (Task 1).
3. Blocker group with two holder groups (`E-30 ← E-17, E-21, E-33 · engine, nobody; L-33 · lead, nobody`): one row, holder cell lists both groups on separate lines (Task 2, browser).
4. Logbook failure row: red wash plus error text, warn row (clean-room CLI write) orange text only (Task 2).
5. Narrow (≤600px) logbook: `what` cell wraps to its own line as before (Task 2, headless 600px).

---

### Task 1: Tracks as rows with lockups

**Files:**
- Modify: `src/web/app.js` `tracksView()` (`:378-437`)
- Modify: `src/web/style.css` Tracks block (`:574-606`)
- Test: `tests/web.rs`

**Interfaces:**
- Produces: classes `track-row` (grid `220px minmax(0,1fr) 340px`, zebra via `:nth-child(even)`), `track-row__name` with `h3.track-row__title`, `track-row__mini` (`span` per status, `b` count, class `is-zero` when 0), `track-row__now` with `runrow` entries, `course-warning`, `track-last`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn dashboard_tracks_are_line_free_rows() {
    let s = Server::start();
    let get = |path: &str| s.raw("GET", path, &[("Host", &s.host())], "");
    let js = get("/app.js").body;
    assert!(js.contains("track-row__mini") && js.contains("class: \"runrow\""));
    let css = get("/style.css").body;
    assert!(css.contains(".track-row:nth-child(even)"), "tonal zebra, not lines");
    assert!(!css.contains("border-left: 3px"), "no stripe on track rows");
    assert!(!css.contains(".track-row:first-child { border-top"), "no border-top on track rows");
}
```

- [ ] **Step 2: Run** `cargo test --test web tracks_are_line_free` — Expected: FAIL on `track-row__mini`.
- [ ] **Step 3: Implement.** `tracksView()`: name cell = `priorityChip`, `h("h3", { class: "track-row__title" + (muted) }, name || "no track")`, description `div.track-row__desc`. Bar cell = `bar` + `div.track-row__mini` of `BAR_SEGMENTS.map(([status, name]) => h("span", { class: n ? "" : "is-zero" }, h("b", {}, n), ` ${name}`))`. Now cell = `flagged` warnings (`div.course-warning`), running tasks as `h("div", { class: "runrow" }, h("span", { class: "pulse live", "aria-hidden": "true" }), h("span", { class: "runrow__id" }, t.id), h("span", { class: "runrow__title" }, t.title), h("span", { class: "runrow__meta" }, [claimed_by, `${minutes} min`].filter(Boolean).join(" · ")))`, else `div.track-row__idle` (`idle` / `no tasks`), then `div.track-last` with `last done `, `b` id, ` ` title. Keep `data-track`, role button, `p-${priority}` for the chip colour, `is-offcourse` class.
  CSS: `.track-row` grid `220px minmax(0,1fr) 340px`, gap 32px, padding `20px 16px`, radius `md`, no borders; `:nth-child(even)` background `surface-container-low`; hover `surface-container`; `.track-row__title` Space Grotesk 600 1.25rem line-height 1.1 margin 0; `.track-row__desc` 0.78rem variant; `.bar` height 10px; `.bar__seg--run` adds glow `0 0 8px rgba(166,255,0,.5)`; `.track-row__mini` flex gap 20px 0.78rem variant, `b` mono 500 on-surface, `.is-zero b` on-surface-dim; `.runrow` grid `14px 4em minmax(0,1fr) auto` gap 10px mono 0.75rem, title in body font 0.8125rem on-surface ellipsis, meta variant; `.track-row__idle` mono 0.75rem on-surface-dim; `.track-last` 0.75rem variant, `b` mono 500. Keep the 900px stack rule.
- [ ] **Step 4:** `node --check src/web/app.js && cargo test --test web` — Expected: PASS.
- [ ] **Step 5: Commit** `feat(web): tracks as line-free rows with stat lockups`

### Task 2: Activity panels as tables

**Files:**
- Modify: `src/web/app.js` `problemRow`, `problemsPanel`, `waitingOn`, `statePanel`, `logRow` (`:587-690`)
- Modify: `src/web/style.css` Activity block (`:608-720`)
- Test: `tests/web.rs`

**Interfaces:**
- Produces: classes `act-problem` (grid `16px minmax(0,1fr) 3.5em`), `act-glyph` (`bad`/`warn`), `act-when`, `act-claim` (grid `6em 5em minmax(0,1fr)`), `blk` (grid `minmax(5em,auto) 16px minmax(0,1fr) 9em`), `blk__nobody` (+ `is-hot`), `act-row` zebra.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn dashboard_activity_is_line_free() {
    let s = Server::start();
    let get = |path: &str| s.raw("GET", path, &[("Host", &s.host())], "");
    let js = get("/app.js").body;
    assert!(js.contains("class: \"blk\"") && js.contains("act-glyph"));
    let css = get("/style.css").body;
    assert!(!css.contains("border-top: 1px solid"), "no lines anywhere in the stylesheet");
    assert!(!css.contains("inset 3px 0 0"), "no side stripe on the problems panel");
    assert!(css.contains(".ch--rest"), "channel tags keep their classes");
}
```

- [ ] **Step 2: Run** `cargo test --test web activity_is_line_free` — Expected: FAIL on `class: "blk"`.
- [ ] **Step 3: Implement.** `problemRow`: cells `span.act-glyph.(bad|warn)` (`✕`/`▲`), `div` with `span.act-session` session (when not already in the message) + message (+ `· N×`), `span.act-when` `when(p.last_at, t)`; the button/div split stays. `problemsPanel`: label `Needs attention · N` with class `is-bad` when `N > 0`, else `Needs attention` and `No problems in the last 30 days.` dimmed. `statePanel`: label `Claimed`, each claim `div.act-claim` → session, id link, `track · lease`; label `Blocked · waiting on`; each group `div.blk` → `span.blk__a` blocked id links joined `, `, `span.blk__arr` `←`, `span.blk__b` = `waitingOn(g.waitingOn)` rendered as one `div` per holder group (`ids · role`), `span.blk__who` = per holder group a `div` with session (secondary) or `span.blk__nobody` `nobody on it` (class `is-hot` when any blocked task's track priority is `high`/`critical`: `g.blocked.some((id) => priorityRank(trackOf(taskById(id))?.priority ?? "normal") >= 2)`). `logRow`: unchanged cells; CSS gives zebra.
  CSS: `.act-panel.is-bad` box-shadow removed; `.act-panel > .label.is-bad` color `error`; `.act-problem` grid `16px minmax(0,1fr) 3.5em` gap 12px padding `6px 8px` margin `0 -8px` radius xs, hover high; `.act-glyph` mono centred, `.bad` error, `.warn` tertiary; `.act-when` mono 0.72rem variant right; `.act-claim` grid `6em 5em minmax(0,1fr)` mono 0.75rem; `.blk` grid `minmax(5em,auto) 16px minmax(0,1fr) 9em` gap 10px mono 0.75rem padding `4px 8px` margin `0 -8px` radius xs, `:nth-child(even of .blk)` → use `.blk.is-alt` set by JS index parity instead (Firefox `of` support is recent) with background `surface-container-lowest`; `.blk__arr` on-surface-dim; `.blk__b` variant; `.blk__who` secondary; `.blk__nobody` on-surface-dim, `.is-hot` tertiary; `.act-row` no border-top, `:nth-child(even)` background `surface-container-low`, radius xs, padding `5px 10px`; `.ch` border → tinted background (`--ch` at 10%) no border; `.act-chip[aria-pressed=true]` keeps its ring.
- [ ] **Step 4:** `node --check src/web/app.js && cargo test --test web` — Expected: PASS (43 tests).
- [ ] **Step 5: Browser** openshr at 1376px: Tracks and Activity; headless 960px and 600px Activity.
- [ ] **Step 6: Commit** `feat(web): activity panels as line-free tables`

### Task 3: Finish

- [ ] `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` — green; `cargo install --path .`.
- [ ] Push `design/dashboard-tables`, PR `feat(web): tracks and activity tables (concept step 3)`, CI, DSH review, merge `--merge`.
