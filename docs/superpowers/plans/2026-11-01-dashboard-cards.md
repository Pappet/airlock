# Dashboard Cards and Colour Budget Implementation Plan (concept step 2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the task card and its containers (board columns, swimlanes, stages) follow the concept's colour budget and no-line rule, and collapse the board filters into a popover on narrow screens.

**Architecture:** `card()` and `overlapBadges()` in `src/web/app.js` decide which badge carries which colour; CSS in `src/web/style.css` drops the priority stripe, the `border-top` lines and the card-on-card contrast. `renderFilters()` wraps the track/priority/group controls in a `<details>` that is open on wide screens and a `Filter ▾` pill below 1100px.

**Tech Stack:** vanilla JS, CSS custom properties. No Rust changes.

**Spec:** `docs/superpowers/specs/2026-11-01-dashboard-visual-concept.md` §2.3 (colour budget), §2.4 (surfaces), §3 Board and Stages, §6 (filter popover, step 2).

## Global Constraints

- Red (`error`) on a card only for an overlap with a *running* task (`occupied`); plain overlaps and missing touch lists are neutral.
- No `border-left`/`border-top` for sectioning; tonal alternation or spacing only. Ghost rings (`inset 0 0 0 1px`) mark running/selected/overlap state.
- Card: `surface-container`, hover `surface-container-high`, radius `md`; columns and wells `surface-container-low`.
- `.p-critical` stays in the CSS (pips keep their priority colour; `dashboard_assets_know_tracks` probes it).
- `node --check src/web/app.js` before every commit; `cargo test`, `cargo clippy`, `cargo fmt --check` before the PR.

## Review Focus

1. Task with overlaps where none is running: badge `⧉ N`, neutral, title lists the paths (Task 1 test probes the class; verify in browser against openshr `L-51`).
2. Task without touch list: `? no touch list` neutral with ghost ring, not orange (Task 1).
3. Stages selection: selected card cyan ring, overlapping cards red ring, others dimmed; arrows grey by default, cyan when touching the selection (Task 2, browser).
4. Narrow window (≤1100px): board filters behind a `Filter ▾` pill; opening it must not push the tab row off screen (Task 3, browser).
5. Drawer file list: a path shared with a running task gets the red badge, others neutral (Task 1).

---

### Task 1: Card and badges

**Files:**
- Modify: `src/web/app.js:302-340` (`overlapBadges`, `card`), `:1012-1017` (`pathList`), `:748-756` (`flatBoard`)
- Modify: `src/web/style.css` (badges, priority, cards, columns blocks)
- Test: `tests/web.rs`

**Interfaces:**
- Produces: badge classes `badge` (neutral), `badge--conflict` (red), `badge--unknown` (neutral ghost); card classes `card`, `is-running`, `is-selected`, `is-overlap`, `is-dim`; column class `column--run`.

- [ ] **Step 1: Write the failing test** in `tests/web.rs`

```rust
#[test]
fn dashboard_cards_follow_the_colour_budget() {
    let s = Server::start();
    let get = |path: &str| s.raw("GET", path, &[("Host", &s.host())], "");
    let js = get("/app.js").body;
    assert!(js.contains("badge--conflict"), "red only for overlap with a running task");
    assert!(!js.contains("with running"), "plain overlaps are no longer red");
    let css = get("/style.css").body;
    assert!(css.contains(".badge--conflict"));
    assert!(!css.contains("border-left: 3px"), "no priority stripe on cards");
    assert!(!css.contains("border-top: 1px solid"), "no lines between lanes or rows");
}
```

- [ ] **Step 2: Run** `cargo test --test web colour_budget` — Expected: FAIL on `badge--conflict`.

- [ ] **Step 3: Implement**

`overlapBadges(id)`: unknown → `badge badge--unknown` text `? no touch list`; overlaps with `running.length` → one `badge badge--conflict` text `⚠ running ${running.join(", ")}`; else overlaps → `badge` text `⧉ ${n}`; `title` lists `id: paths` as before.
`card(task, extraClass, onclick, showTrack)`: class `card ${running ? "is-running" : ""} ${extraClass}` (drop `p-${priority}`); id cell `h("span", { class: "card__id" }, running ? h("span", { class: "pulse live", "aria-hidden": "true" }) : null, task.id)`; meta: `claimed_by` wrapped in `h("span", { class: "card__who" }, …)`.
`pathList`: badge class `badge--conflict` when any id starts with `▶`, else `badge`.
`flatBoard`: column class `column${status === "in_progress" ? " column--run" : ""}`; count in `h("b", {}, items.length)`.
CSS: `.badge` neutral (`surface-container-high`, `on-surface-variant`); `.badge--conflict` red 14% / `error`; `.badge--unknown` transparent with `inset 0 0 0 1px var(--surface-container-highest)`; delete `.badge--overlap`, `.badge--running`; `.card` without `border-left`, background `surface-container`, hover `high`; `.card__id` inline-flex gap 8px; `.card__who` secondary; `.card.is-running` ring `rgba(166,255,0,.45)`; `.is-selected` 2px secondary; `.is-overlap` ring `error` 60%, background unchanged; `.column` padding 12px, `.column > .label` with `b` mono 1rem `on-surface-variant`; `.column--run > .label span:first-child` primary. The `.p-*` block keeps `--prio` for pips and chips; `.chip` keeps its ghost border (tag badge, allowed).

- [ ] **Step 4: Run** `node --check src/web/app.js && cargo test --test web` — Expected: PASS, 40 tests.

- [ ] **Step 5: Commit** `feat(web): card colour budget; red only for overlap with a running task`

### Task 2: Swimlanes and stages without lines

**Files:**
- Modify: `src/web/app.js:759-784` (`swimlanes`), `:786-829` (`stagesView`)
- Modify: `src/web/style.css` (swim, stages, selection blocks)

- [ ] **Step 1:** CSS: `.swim__lane`, `.swim__cell` lose `border-top`; each lane row gets `background: var(--surface-container-low)` on alternate lanes via a `swim__row--alt` class that `swimlanes()` adds to every second lane's cells (`laneIndex % 2`), radius `md` on the first/last cell of the row is not needed (cells are gapped; use `padding: 8px` and no radius). `.stages svg path` stroke `rgba(65,74,52,.6)`; `.hot` secondary 2px. `.selection` becomes a `label`-line: `.selection` padding `0 0 12px`, no background, mono `.75rem`; the warn/ok colours stay.
- [ ] **Step 2:** `stagesView`: the hint when nothing is selected renders as `h("div", { class: "selection muted" }, …)` already — keep text, the CSS change does the rest. Stage column headers: `h("b", {}, items.length)` like the board.
- [ ] **Step 3:** `node --check src/web/app.js && cargo test --test web` — Expected: PASS.
- [ ] **Step 4:** Browser: `target/debug/airlock serve -C ~/Projects/openshr --port 0`; Board with group-by-track on: lanes alternate, no lines; Stages: click `L-51`, cards and arrows recolour as in Review Focus 3.
- [ ] **Step 5:** Commit `feat(web): swimlanes and stages without lines`

### Task 3: Filter popover under 1100px

**Files:**
- Modify: `src/web/app.js:1124-1180` (`renderFilters`), `start()` (media query listener)
- Modify: `src/web/style.css` (filters block)

- [ ] **Step 1:** `renderFilters`: wrap `track, min, group` in `h("details", { class: "filter-pop", open: wide ? "" : null }, h("summary", { class: "pill" }, "filter ▾"), h("div", { class: "filter-pop__body" }, track, min, group))` where `wide = matchMedia("(min-width: 1101px)").matches`; include `wide` in the memo `key`. `h()` must skip attributes whose value is `null` — check `h()` at `app.js:188-201` and add `if (value == null) continue;` if missing.
- [ ] **Step 2:** `start()`: `matchMedia("(min-width: 1101px)").addEventListener("change", () => render())`.
- [ ] **Step 3:** CSS: `.filter-pop > summary { list-style: none; cursor: pointer; }`, `.filter-pop > summary::-webkit-details-marker { display: none; }`; `@media (min-width: 1101px) { .filter-pop > summary { display: none; } .filter-pop__body { display: flex; gap: 8px; } }`; `@media (max-width: 1100px) { .filter-pop { position: relative; } .filter-pop__body { position: absolute; z-index: 5; top: calc(100% + 4px); left: 0; display: grid; gap: 8px; padding: 12px; border-radius: var(--radius-md); background: var(--surface-container-highest); box-shadow: 0 4px 24px rgba(226,226,226,.06); } }`.
- [ ] **Step 4:** `node --check src/web/app.js && cargo test --test web`; browser at a 1000px window: pill shows, opens a floating panel, tabs stay put.
- [ ] **Step 5:** Commit `feat(web): board filters collapse into a popover below 1100px`

### Task 4: Finish

- [ ] `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` — green.
- [ ] `cargo install --path .`
- [ ] Push `design/dashboard-cards`, PR `feat(web): cards and colour budget (concept step 2)`, wait for CI and the DSH review comment, merge with `--merge`.
