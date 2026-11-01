# Dashboard Shell Implementation Plan (concept step 1)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the three flat chrome rows of the dashboard with header, hero band and tab nav from the visual concept, and ship the SPECTRUM fonts inside the binary.

**Architecture:** `src/web.rs` gains three `/fonts/*.woff2` routes served from `include_bytes!`. `src/web/index.html` gets the new skeleton (header, hero, nav); `renderChrome()` in `src/web/app.js` fills it; `src/web/style.css` gets the `@font-face` rules and the shell styles. Views below the nav are untouched in this step.

**Tech Stack:** Rust (axum 0.8), vanilla JS modules, CSS custom properties. No new crates.

**Spec:** `docs/superpowers/specs/2026-11-01-dashboard-visual-concept.md` (§2.1, §2.2, §2.5, §6) and `docs/spec/04_web_dashboard_and_api.md` §1 (nothing loaded from external hosts, CSP `default-src 'self'`).

## Global Constraints

- The dashboard loads nothing from external hosts (Spec 04 §1). Fonts are served by the binary under `/fonts/`.
- Assets carry `Content-Security-Policy: default-src 'self'; img-src 'self' data:; frame-ancestors 'none'` (existing `asset()`); fonts fall under `default-src 'self'`.
- `STUDIO // v{major}.{minor}` stays verbatim (`#version`).
- No 1px solid borders for sectioning; tonal shifts only. Tabs get a 2px primary underline (positional anchor), filters stay pills.
- Hero band is always visible, at most three lease rows, then `+N more`. Seam turns tertiary under 15 min (concept §2.1).
- The under-1100px "Filter ▾" popover (concept §6) is step 2's, with the board filters; this step only wraps the filter cluster.
- Space Grotesk, Manrope, JetBrains Mono: OFL 1.1; licence texts ship in `src/web/fonts/`.
- `node --check src/web/app.js` before every commit (memory rule); `cargo test`, `cargo clippy`, `cargo fmt --check` before the PR.

## Review Focus

1. Dashboard opened with no running task: hero "now" column shows `nothing running` muted, no empty box (Task 3 test `hero_handles_no_running_tasks` is a JS-string check; verify manually in browser).
2. More than three running tasks: three rows plus a `+N more` label, never a fourth row (Task 3).
3. Lease expired or under 15 min: seam turns tertiary, label still readable (Task 3 styles).
4. Window under 1100px at 125% zoom: nav wraps, no horizontal document scroll (Task 3 CSS; verify in browser).
5. Font route under a disallowed `Host`: still rejected by `guard` with 403 (Task 1 test).

---

### Task 1: Serve fonts from the binary

**Files:**
- Create: `src/web/fonts/space-grotesk.woff2`, `src/web/fonts/manrope.woff2`, `src/web/fonts/jetbrains-mono.woff2` (latin subsets, variable, from the scratchpad download)
- Create: `src/web/fonts/OFL-space-grotesk.txt`, `src/web/fonts/OFL-manrope.txt`, `src/web/fonts/OFL-jetbrains-mono.txt`
- Modify: `src/web.rs:50-52` (constants), `:190-203` (routes), `:234-247` (`asset`)
- Test: `tests/web.rs`

**Interfaces:**
- Produces: `GET /fonts/space-grotesk.woff2`, `/fonts/manrope.woff2`, `/fonts/jetbrains-mono.woff2` → 200, `Content-Type: font/woff2`, `Cache-Control: max-age=86400`, same CSP as other assets.

- [ ] **Step 1: Write the failing tests** in `tests/web.rs`

```rust
#[test]
fn serves_embedded_fonts() {
    let s = Server::start();
    for name in ["space-grotesk", "manrope", "jetbrains-mono"] {
        let r = s.raw("GET", &format!("/fonts/{name}.woff2"), &[("Host", &s.host())], "");
        assert_eq!(r.status, 200, "{name}");
        assert!(r.body.starts_with("wOF2"), "{name}: not a woff2 body");
        assert_eq!(r.header("content-type").as_deref(), Some("font/woff2"));
    }
    let css = s.raw("GET", "/style.css", &[("Host", &s.host())], "").body;
    assert!(css.contains("url(/fonts/space-grotesk.woff2)"), "style.css must use the embedded fonts");
    assert!(!css.contains("googleapis"), "no external font host");
}

#[test]
fn fonts_need_allowed_host() {
    let s = Server::start();
    let r = s.raw("GET", "/fonts/manrope.woff2", &[("Host", "evil.example")], "");
    assert_eq!(r.status, 403);
}
```

If `Server::raw` returns the body as `String`, read it with `String::from_utf8_lossy` in the helper or add a `raw_bytes` variant; `starts_with("wOF2")` must see the first four bytes. Check how `r.body` and headers are exposed in `tests/web.rs:1-145` and adapt the accessor (`r.header(...)`) to what exists.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test web fonts`
Expected: FAIL, 404 for `/fonts/...`.

- [ ] **Step 3: Implement**

In `src/web.rs`: add
```rust
const FONT_SPACE_GROTESK: &[u8] = include_bytes!("web/fonts/space-grotesk.woff2");
const FONT_MANROPE: &[u8] = include_bytes!("web/fonts/manrope.woff2");
const FONT_JETBRAINS_MONO: &[u8] = include_bytes!("web/fonts/jetbrains-mono.woff2");
```
add `fn font(body: &'static [u8]) -> Response` next to `asset()` with headers `Content-Type: font/woff2`, `Cache-Control: max-age=86400`, and the same CSP string (move the CSP into a `const CSP: &str`). Add three routes `/fonts/<name>.woff2` → `get(|| async { font(FONT_...) })` before `/api/project`.

- [ ] **Step 4: Add `@font-face` to `src/web/style.css`** (top of file, before `:root`)

```css
@font-face { font-family: "Space Grotesk"; font-style: normal; font-weight: 500 700; font-display: swap; src: url(/fonts/space-grotesk.woff2) format("woff2"); }
@font-face { font-family: "Manrope"; font-style: normal; font-weight: 400 700; font-display: swap; src: url(/fonts/manrope.woff2) format("woff2"); }
@font-face { font-family: "JetBrains Mono"; font-style: normal; font-weight: 400 600; font-display: swap; src: url(/fonts/jetbrains-mono.woff2) format("woff2"); }
```
Replace the comment on line 1 (`no external fonts`) with `fonts embedded in the binary, served under /fonts/`.

- [ ] **Step 5: Run tests**

Run: `cargo test --test web`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/web/fonts src/web.rs src/web/style.css tests/web.rs
git commit -m "feat(web): embed Space Grotesk, Manrope, JetBrains Mono"
```

### Task 2: New skeleton and chrome rendering

**Files:**
- Modify: `src/web/index.html` (whole body above `<main>`)
- Modify: `src/web/app.js:1088-1125` (`renderChrome`), `:220-226` (`setLive`), `:86-91` (`tokenRejected`)
- Modify: `src/web/style.css` (header, section-strip, tabs blocks)
- Test: `tests/web.rs`

**Interfaces:**
- Produces: element ids `hero-stats`, `hero-now`, `tabs`, `filters` (holds `board-filters` and `role-filter`), `live-label`, `live-dot`, `project-name`, `version`.
- Produces: `heroStats(state) -> HTMLElement[]` and `heroNow(state) -> HTMLElement[]` in `app.js`, called by `renderChrome()`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn dashboard_has_hero_band() {
    let s = Server::start();
    let get = |path: &str| s.raw("GET", path, &[("Host", &s.host())], "");
    let html = get("/").body;
    assert!(html.contains("id=\"hero-stats\"") && html.contains("id=\"hero-now\""));
    assert!(!html.contains("section-strip"), "section strip is replaced by the hero band");
    let js = get("/app.js").body;
    assert!(js.contains("function heroNow") && js.contains("+ more"));
    let css = get("/style.css").body;
    assert!(css.contains(".tab.on::after"), "active tab gets an underline anchor");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test web dashboard_has_hero_band`
Expected: FAIL on `hero-stats`.

- [ ] **Step 3: Rewrite `index.html` above `<main>`**

```html
<header class="hdr">
  <div class="brand"><div class="logo-tile" aria-hidden="true">A</div><span class="brand__product">AIRLOCK</span><span class="brand__tag" id="version">STUDIO // v0.1</span></div>
  <div class="hdr__right"><span class="project" id="project-name"></span><span class="live"><span class="pulse" id="live-dot" aria-hidden="true"></span><span id="live-label">CONNECTING</span></span></div>
</header>
<section class="hero"><div class="stats" id="hero-stats"></div><div class="now"><div class="label">In the airlock now</div><div id="hero-now"></div></div></section>
<nav class="nav"><div class="tabs" id="tabs" role="tablist"></div><div class="filters" id="filters"><span class="filter-group" id="board-filters" role="group" aria-label="Track and priority filters"></span><span class="filter-group" id="role-filter" role="group" aria-label="Role filter"></span></div></nav>
```
Keep `<main>`, drawer and toast as they are. Project name in the header: `project` label muted, name in `on-surface` (`<span class="project">project <b id="project-name"></b></span>` is fine; adapt `renderChrome`).

- [ ] **Step 4: Implement `heroStats(state)` and `heroNow(state)` in `app.js`** above `renderChrome`

`heroStats`: five `.stat` lockups `open`, `running` (class `hot`), `in review`, `open questions` (class `info`), `shared files`; value in `<b>`, label in `<span>`.
`heroNow`: tasks with `status === "in_progress"` sorted by `byNextOrder`, first three as `.lease` rows: `.pulse`, `.id`, `.t` title, `.who` `claimed_by`, `.left` `${minutesLeft(t)} min` (class `low` when under 15), `.seam` with `<i style="width:{pct}%">` where `pct = min(100, 100 * minutesLeft / state.project.lease_minutes)`; the row is a `button` that calls `openDetail(t.id)`. More than three → append `<div class="more">+N more</div>` linking to `#leases`. None → `<div class="idle">nothing running</div>`.
`renderChrome`: replace the `#counts` line with `$("hero-stats").replaceChildren(...heroStats(state))` and `$("hero-now").replaceChildren(...heroNow(state))`; tabs become `button.tab` with class `on` when active and the count in `<span class="n">`; role pills unchanged.
`setLive` / `tokenRejected`: keep ids; class names `pulse live|offline`.

- [ ] **Step 5: CSS** in `style.css`: delete the `.studio-header*`, `.section-strip*`, `.tabs` blocks; add `.hdr`, `.brand*`, `.hdr__right`, `.live`, `.pulse` (dot plus two rings, 1.6s, `prefers-reduced-motion` kills the rings), `.hero`, `.stats`, `.stat`, `.now`, `.lease*`, `.more`, `.idle`, `.nav`, `.tabs`, `.tab`, `.tab.on::after`, `.filters`. Add token `--on-surface-dim: #555555` to `:root` (dimmed counters, zero values, placeholders). Values from the mock (`scratchpad/mock.html`): header 64px on `surface-container-lowest`, hero `grid-template-columns: minmax(0,1fr) minmax(0,1.1fr)` with 48px gap, stat value 2.25rem mono 500, lease grid `16px 4.2em minmax(0,1fr) 5em 4.5em`, tab 0.72rem mono +0.12em, underline 2px primary. `.nav { flex-wrap: wrap }`, `.tabs { overflow-x: auto }`, `.filters { flex-wrap: wrap }`, `html, body { overflow-x: hidden }`. Below 1100px the hero stacks to one column.

- [ ] **Step 6: Run checks**

Run: `node --check src/web/app.js && cargo test --test web`
Expected: both pass; `dashboard_assets_know_tracks` still finds `.p-critical`.

- [ ] **Step 7: Browser check** — `airlock serve -C ~/Projects/openshr --port 0`, open the printed URL, confirm: hero shows five numbers and three lease rows, tab underline moves on click, filters wrap at 125% zoom with no horizontal scroll, LIVE pulses in the header. Also check a project with nothing running (`airlock serve` in `~/Projects/airlock` itself).

- [ ] **Step 8: Commit**

```bash
git add src/web/index.html src/web/app.js src/web/style.css tests/web.rs
git commit -m "feat(web): hero band, header live state, tab underline nav"
```

### Task 3: Finish

- [ ] **Step 1:** `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test` — all green.
- [ ] **Step 2:** `cargo install --path .` so the local binary carries the new shell (memory rule).
- [ ] **Step 3:** Push branch `design/dashboard-shell`, open PR titled `feat(web): dashboard shell (concept step 1)` with the concept link, wait for CI, merge with `--merge` after the DSH review comment (memory rule).
