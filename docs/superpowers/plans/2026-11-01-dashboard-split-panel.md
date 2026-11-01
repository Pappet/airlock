# Dashboard Review and Questions Split Panel Implementation Plan (concept step 5)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show Review and Questions as a split panel: a compact list on the left, the selected item with its form on the right, instead of one long column with an empty right half.

**Architecture:** A shared `splitView({ rows, selected, onSelect, detail, emptyText })` helper in `src/web/app.js` renders the two panes; `reviewView()` and `questionsView()` feed it rows and a detail builder and keep their existing API calls. `state.reviewSelected` / `state.questionSelected` hold the chosen id and fall back to the first visible item. CSS adds the `.split*` block; the old `.list`/`.item` block goes (Leases no longer uses it).

**Tech Stack:** vanilla JS, CSS grid. No Rust changes.

**Spec:** `docs/superpowers/specs/2026-11-01-dashboard-visual-concept.md` §3 Review und Questions, §2.4, §6 decision 2.

## Global Constraints

- Split per SPECTRUM §5.5: both panes on `surface`, the seam is a 2px tonal gap, no divider line; the list well is `surface-container-low`, the selected row lifts to `surface-container` with a 2px secondary inset ring (selection state).
- Forms keep their behaviour: Reject enabled only with feedback text, Answer/Revise enabled only with answer text, Resolve only on answered questions; `state.pending` still protects a half-typed answer across live reloads.
- Below 900px the list sits above the detail with its own 220px scroll.
- `node --check src/web/app.js` before every commit; `cargo test`, `cargo clippy`, `cargo fmt --check` before the PR; browser check against openshr at 1376px and headless at 800px.

## Review Focus

1. Selected question gets resolved by someone else while open: the list refreshes, the selection falls back to the first visible item, no stale form (Task 2, browser via `airlock question resolve`).
2. Filter switch (open → resolved) with a selection that is no longer visible: first item of the new filter is shown (Task 2).
3. Review queue with one task: list row selected by default, detail shows notes and both buttons (Task 1, browser against the demo project with a `--review` task if available, else openshr).
4. Typing an answer while an SSE reload arrives: textarea keeps its text (existing `pending` path; verify once in the browser) (Task 2).
5. Narrow 800px: list above detail, both reachable, no horizontal scroll (Task 3, headless).

---

### Task 1: `splitView` helper and the Review tab

**Files:**
- Modify: `src/web/app.js` `state` object (`:35-57`), `reviewView()`
- Modify: `src/web/style.css` (replace the Lists block with `.split*`, keep `.actions`, `.field`, `textarea`, `.btn*`)
- Test: `tests/web.rs`

**Interfaces:**
- Produces: `splitView({ rows, selected, onSelect, detail, emptyLabel, emptyText })` where `rows = [{ id, cells: Node[] , status?: string }]`, `detail(id) -> Node`; classes `split`, `split__list`, `split__row` (`aria-pressed`), `split__detail`, `split__title`, `split__meta`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn dashboard_review_and_questions_are_split_panels() {
    let s = Server::start();
    let get = |path: &str| s.raw("GET", path, &[("Host", &s.host())], "");
    let js = get("/app.js").body;
    assert!(js.contains("function splitView") && js.contains("reviewSelected") && js.contains("questionSelected"));
    let css = get("/style.css").body;
    assert!(css.contains(".split {") && css.contains(".split__row[aria-pressed=\"true\"]"));
    assert!(!css.contains(".list {"), "the single-column list block is gone");
}
```

- [ ] **Step 2: Run** `cargo test --test web split_panels` — Expected: FAIL on `function splitView`.
- [ ] **Step 3: Implement.** `state`: add `reviewSelected: null, questionSelected: null`. `splitView`: if `!rows.length` return `empty(emptyLabel, emptyText)`; `const id = rows.some((r) => r.id === selected) ? selected : rows[0].id`; left `aside.split__list` of `button.split__row` (`aria-pressed`, `onclick: () => onSelect(r.id)`, children `r.cells`); right `section.split__detail` = `detail(id)`. `reviewView()`: rows `{ id: task.id, cells: [span.split__id task.id, span.split__text task.title, pips(task.priority)] }`; detail: `div.split__meta` (role label, pips, track label, id link to drawer), `h2.split__title` title, notes `p` or muted, then the existing feedback/approve/reject form. Only the detail builds the form, so one textarea exists at a time. CSS: `.split { display: grid; grid-template-columns: minmax(260px, 34%) minmax(0, 1fr); gap: 2px; }`, `.split__list { display: grid; align-content: start; gap: 2px; padding: var(--space-2); border-radius: var(--radius-md); background: var(--surface-container-low); max-height: 70vh; overflow: auto; }`, `.split__row { display: grid; grid-template-columns: 3.4em minmax(0, 1fr) auto; gap: var(--space-2); align-items: baseline; width: 100%; padding: var(--space-2) 10px; border: none; border-radius: var(--radius-sm); background: none; color: var(--on-surface); font-size: 0.8125rem; text-align: left; cursor: pointer; }`, hover `surface-container`, `[aria-pressed="true"] { background: var(--surface-container); box-shadow: inset 0 0 0 2px var(--secondary); }`, `.split__id` mono 600 0.78rem, `.split__text` ellipsis, `.split__detail { padding: var(--space-4) var(--space-6); min-width: 0; }`, `.split__title { margin: var(--space-2) 0 var(--space-4); font-family: var(--font-display); font-weight: 600; font-size: 1.4rem; letter-spacing: -0.01em; }`, `.split__meta { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-3); }`, `.split__body { white-space: pre-wrap; overflow-wrap: anywhere; }`; `@media (max-width: 900px) { .split { grid-template-columns: 1fr; } .split__list { max-height: 220px; } }`. Delete `.list`, `.item`, `.item__head`, `.item__title`, `.item p`.
- [ ] **Step 4:** `node --check src/web/app.js && cargo test --test web` — PASS.
- [ ] **Step 5: Commit** `feat(web): review as a split panel`

### Task 2: Questions tab

**Files:**
- Modify: `src/web/app.js` `questionsView()`, `questionItem()` → `questionDetail(q)`

- [ ] **Step 1:** Rows `{ id: q.id, cells: [span.split__id "#id", span.split__text q.question (first line, the CSS truncates), span.split__status glyph+status] }` with the status glyphs from today (`●`/`◐`/`✓`); keep the status filter pills above the split. `questionDetail(q)`: meta (`#id`, topic label, status label, corrected chip, `asked by` mono muted, `ago(created_at)`), `div.split__body` question, answer paragraph with `answered_by`, spec line, then the existing answer form (resolved questions show no form). `state.questionSelected` set by `onSelect`.
- [ ] **Step 2:** `node --check src/web/app.js && cargo test --test web` — PASS.
- [ ] **Step 3:** Browser against openshr: switch filters, pick questions, type into the answer box and wait for a live reload (`airlock task list` in another shell triggers none; use `airlock question list` → no write; instead start a second `airlock serve`? Simplest: heartbeat a running task from the openshr worktree, which emits an event) — text stays.
- [ ] **Step 4: Commit** `feat(web): questions as a split panel`

### Task 3: Finish

- [ ] Headless 800px Questions: list above detail, document narrower than the viewport.
- [ ] `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` — green; `cargo install --path .`.
- [ ] Push `design/dashboard-split`, PR `feat(web): review and questions split panel (concept step 5)`, CI, DSH review, merge `--merge`.
