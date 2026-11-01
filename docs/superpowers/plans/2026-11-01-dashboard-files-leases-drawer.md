# Dashboard Files, Leases and Drawer Implementation Plan (concept step 4)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring the file matrix, the Leases tab and the detail drawer onto the concept's colour budget and surfaces, and fix the Leases overflow.

**Architecture:** `filesView()`, `leasesView()`, `renderDrawer()` in `src/web/app.js` change their markup only where a cell needs its own class; `src/web/style.css` carries the rest (sticky matrix headers, neutral hits, lease rows reusing the hero's `.lease` grid, glass drawer with a sticky head). No data or API change.

**Tech Stack:** vanilla JS, CSS. No Rust changes.

**Spec:** `docs/superpowers/specs/2026-11-01-dashboard-visual-concept.md` §2.3, §2.4, §3 Files / Leases / Drawer; step-2 deferral (long Stages selection list).

## Global Constraints

- Matrix: a hit is neutral; red only where a *running* task holds the file (`occupied`); the per-file count is red only when the file has a running holder. The ghost outline on hover stays (hover state, not sectioning).
- Leases: no horizontal overflow at any width; progress notes wrap (`pre-wrap`).
- Drawer: `surface-container-high` at 80% with 16px backdrop blur, sticky head holding tags and `Close`, title in Space Grotesk, Read and Touch side by side from 480px drawer width, `pre` blocks on `surface-container-lowest`.
- Stages selection list: capped at 180px with its own scroll.
- No new lines; `node --check src/web/app.js` before every commit; `cargo test`, `cargo clippy`, `cargo fmt --check` before the PR; browser check against openshr at 1376px, headless at 600px for Leases and the drawer.

## Review Focus

1. File held by a running task and touched by five idle tasks: one red `▶` cell, four neutral `●` cells, count red (Task 1, browser).
2. Leases with a 600-character progress note at 600px: note wraps inside its block, page does not scroll sideways (Task 2, headless).
3. Lease under 15 min: minutes in tertiary, seam tertiary; expired: `EXPIRED` in error, seam empty (Task 2).
4. Drawer on a done task: no overlap badges, no claimed line, Read/Touch still two columns (Task 3, browser).
5. Drawer scrolled to Notes: head with tags and `Close` stays visible (Task 3, browser).

---

### Task 1: File matrix

**Files:**
- Modify: `src/web/app.js` `filesView()` (`:878-920`)
- Modify: `src/web/style.css` File matrix block (`:438-460`)
- Test: `tests/web.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn dashboard_matrix_is_sticky_and_neutral() {
    let s = Server::start();
    let get = |path: &str| s.raw("GET", path, &[("Host", &s.host())], "");
    let css = get("/style.css").body;
    assert!(css.contains(".matrix thead th { position: sticky"), "sticky column heads");
    assert!(css.contains(".matrix tbody th { position: sticky"), "sticky row heads");
    assert!(css.contains(".matrix td.hit.occupied { background: rgba(255, 77, 77"), "red only for a running holder");
    let js = get("/app.js").body;
    assert!(js.contains("count is-hot"), "file count red only with a running holder");
}
```

- [ ] **Step 2: Run** `cargo test --test web matrix_is_sticky` — Expected: FAIL on sticky column heads.
- [ ] **Step 3: Implement.** `filesView()`: row head count `h("span", { class: f.occupied_by.length ? "count is-hot" : "count" }, f.tasks.length)` (no `⚠`); legend texts: `● open task touches the file`, `▶ running task holds the file`, `n open tasks share the file`. CSS: `.matrix-wrap` padding 0 with `max-height: 70vh; overflow: auto`; `.matrix thead th { position: sticky; top: 0; z-index: 2; background: var(--surface-container-low); }`; `.matrix tbody th { position: sticky; left: 0; z-index: 1; background: var(--surface-container-low); }`; `.matrix thead th:first-child { left: 0; z-index: 3; }`; `.matrix td.hit { background: var(--surface-container-high); color: var(--on-surface-variant); }`; `.matrix td.hit.occupied { background: rgba(255, 77, 77, 0.22); color: var(--error); }`; `.matrix tbody th .count { color: var(--on-surface-variant); }`, `.count.is-hot { color: var(--error); }`; table padding moves to `th`/`td` so sticky cells have no gap.
- [ ] **Step 4:** `node --check src/web/app.js && cargo test --test web` — PASS.
- [ ] **Step 5: Commit** `feat(web): file matrix with sticky heads; red only for a running holder`

### Task 2: Leases as lease rows

**Files:**
- Modify: `src/web/app.js` `leasesView()` (`:990-1027`)
- Modify: `src/web/style.css` (new `.lease-item` block after Lists; `.minutes` dropped)
- Test: `tests/web.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn dashboard_leases_wrap_their_progress() {
    let s = Server::start();
    let get = |path: &str| s.raw("GET", path, &[("Host", &s.host())], "");
    let js = get("/app.js").body;
    assert!(js.contains("lease-item__progress"));
    let css = get("/style.css").body;
    assert!(css.contains(".lease-item__progress { ") && css.contains("white-space: pre-wrap"), "progress notes wrap");
}
```

- [ ] **Step 2: Run** `cargo test --test web leases_wrap` — Expected: FAIL.
- [ ] **Step 3: Implement.** `leasesView()` returns `div.leases` of `article.lease-item`: head `div.lease-item__head` = `span.pulse.live`, `button.link.lease-item__id` (opens the drawer), `span.lease-item__title`, `span.lease__who`, `span.lease__left` (`${minutes} min` / `EXPIRED` / `—`; class `is-low` under 15, `is-expired` at 0), `span.lease__seam > i` with `pct = min(100, 100 * minutes / lease_minutes)` as in `heroNow` (expired → 0); then `pre.lease-item__progress` when `task.progress`; then `div.lease-item__actions` with the armed `Force release` button (behaviour unchanged). CSS: `.leases { display: grid; gap: var(--space-2); max-width: 1100px; }`; `.lease-item { padding: var(--space-3) var(--space-4); border-radius: var(--radius-md); background: var(--surface-container-low); }`; `.lease-item__head { display: grid; grid-template-columns: 16px 4.2em minmax(0,1fr) 6em 5em; gap: var(--space-3); align-items: center; font-family: var(--font-mono); font-size: 0.8125rem; }` with the seam spanning `2 / -1`; `.lease-item__title { font-family: var(--font-body); overflow-wrap: anywhere; }`; `.lease-item__progress { margin: var(--space-3) 0 0; padding: var(--space-3); border-radius: var(--radius-sm); background: var(--surface-container-lowest); font-family: var(--font-mono); font-size: 0.78rem; white-space: pre-wrap; overflow-wrap: anywhere; }`; `.lease-item__actions { display: flex; justify-content: flex-end; margin-top: var(--space-3); }`; `.lease__left.is-expired { color: var(--error); }`; delete `.minutes*`. Below 600px the head drops the `who` column like the hero.
- [ ] **Step 4:** `node --check src/web/app.js && cargo test --test web` — PASS.
- [ ] **Step 5: Commit** `feat(web): leases as lease rows with wrapping progress`

### Task 3: Drawer as glass panel; capped stages selection

**Files:**
- Modify: `src/web/app.js` `renderDrawer()` (`:1052-1100`)
- Modify: `src/web/style.css` Drawer block (`:509-540`, `:572`), `.selection`
- Test: `tests/web.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn dashboard_drawer_is_glass_with_sticky_head() {
    let s = Server::start();
    let get = |path: &str| s.raw("GET", path, &[("Host", &s.host())], "");
    let css = get("/style.css").body;
    assert!(css.contains("backdrop-filter: blur(16px)"), "glass drawer");
    assert!(css.contains(".drawer__head { position: sticky"), "sticky head");
    assert!(css.contains(".selection__list { max-height"), "capped stages selection list");
    let js = get("/app.js").body;
    assert!(js.contains("drawer__paths"));
}
```

- [ ] **Step 2: Run** `cargo test --test web drawer_is_glass` — Expected: FAIL.
- [ ] **Step 3: Implement.** `renderDrawer()`: first child `div.drawer__head` = `div.drawer__tags` (role · status label, priority chip, track label) + `button.btn.close` `Close`; then `h2`, claimed line as `div.drawer__claim` (`span.pulse.live`, session in `act-session`, `· N min left`), badges, then `div.drawer__paths` holding the Read and Touch sections, then the rest unchanged. `stagesView()`: wrap the per-overlap `div`s in `div.selection__list`. CSS: `.drawer { background: rgba(42, 42, 42, 0.8); backdrop-filter: blur(16px); -webkit-backdrop-filter: blur(16px); }`; `.drawer__head { position: sticky; top: 0; z-index: 1; display: flex; justify-content: space-between; align-items: center; gap: var(--space-3); margin: calc(-1 * var(--space-6)) calc(-1 * var(--space-6)) 0; padding: var(--space-4) var(--space-6); background: rgba(42, 42, 42, 0.92); }`; `.close` loses `float`; `.drawer h2` 1.4rem Space Grotesk stays; `.drawer__claim { display: inline-flex; gap: var(--space-2); align-items: center; font-family: var(--font-mono); font-size: 0.8125rem; }`; `.drawer__paths { display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-4); }` with `.drawer__paths section { margin-top: var(--space-5); min-width: 0; }` and one column under 480px drawer width (`@media (max-width: 560px)`); `.drawer li` `overflow-wrap: anywhere`; `.selection__list { max-height: 180px; overflow: auto; padding-right: var(--space-2); }`.
- [ ] **Step 4:** `node --check src/web/app.js && cargo test --test web` — PASS (45 tests).
- [ ] **Step 5: Browser** openshr: Files, Leases, drawer on a running and on a done task, Stages selection on `E-17`; headless 600px Leases.
- [ ] **Step 6: Commit** `feat(web): glass drawer with sticky head; capped stages selection`

### Task 4: Finish

- [ ] `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` — green; `cargo install --path .`.
- [ ] Push `design/dashboard-files-leases-drawer`, PR `feat(web): files, leases, drawer (concept step 4)`, CI, DSH review, merge `--merge`.
