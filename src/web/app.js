// Airlock dashboard (Spec 04). Plain ES module, no dependencies.
// All data is rendered with textContent; `verify` is shown, never executed.

const TOKEN_KEY = "airlock-token";
const GROUP_KEY = "airlock-group-by-track";
const PRIORITIES = ["critical", "high", "normal", "low"];
const PIPS = { critical: "●●●●", high: "●●●○", normal: "●●○○", low: "●○○○" };
const STATUS_COLUMNS = [
  ["blocked", "Blocked"],
  ["ready", "Ready"],
  ["in_progress", "In Progress"],
  ["in_review", "In Review"],
  ["done", "Done"],
];
const TABS = [
  ["tracks", "Tracks"],
  ["board", "Board"],
  ["stages", "Stages"],
  ["files", "Files"],
  ["review", "Review"],
  ["questions", "Questions"],
  ["leases", "Leases"],
];
const RUNNING = new Set(["in_progress", "in_review"]);

function loadGroupByTrack() {
  try { return localStorage.getItem(GROUP_KEY) !== "false"; } catch (_) { return true; }
}

function saveGroupByTrack(on) {
  try { localStorage.setItem(GROUP_KEY, String(on)); } catch (_) { /* private mode */ }
}

const state = {
  project: null,
  tasks: [],
  tracks: [],
  groupByTrack: loadGroupByTrack(),
  trackFilter: null, // null = all tracks, "" = no track, otherwise a track name
  minPriority: null, // null = all, otherwise a level
  analysis: { tasks: [], files: [], no_touch: [] },
  questions: [],
  tab: "board",
  role: null,
  selected: null,
  detail: null,
  questionFilter: "open",
  live: false,
};

// ─── Token & API ─────────────────────────────────────────────────────
function token() {
  const match = location.hash.match(/token=([0-9a-f]+)/);
  if (match) {
    try { sessionStorage.setItem(TOKEN_KEY, match[1]); } catch (_) { /* private mode */ }
    history.replaceState(null, "", location.pathname + location.search);
    state.token = match[1];
  }
  if (!state.token) {
    try { state.token = sessionStorage.getItem(TOKEN_KEY); } catch (_) { state.token = null; }
  }
  return state.token;
}

async function api(method, path, body) {
  const headers = { Authorization: `Bearer ${state.token}` };
  if (method === "POST") headers["Content-Type"] = "application/json";
  const res = await fetch(path, {
    method,
    headers,
    body: method === "POST" ? JSON.stringify(body ?? {}) : undefined,
  });
  const data = await res.json().catch(() => ({}));
  if (res.status === 401) tokenRejected();
  if (!res.ok) throw new Error(data.error || `${res.status} ${res.statusText}`);
  return data;
}

// A restarted server has a new token; stop and say so instead of retrying.
function tokenRejected() {
  state.rejected = true;
  setLive(false);
  $("live-label").textContent = "TOKEN REJECTED";
  $("view").replaceChildren(empty("Access token rejected", "The server was restarted or the link is outdated. Open the URL that `airlock serve` printed last."));
}

async function load() {
  const [tasks, analysis, questions, tracks] = await Promise.all([
    api("GET", "/api/tasks"),
    api("GET", "/api/overlaps"),
    api("GET", "/api/questions"),
    api("GET", "/api/tracks"),
  ]);
  tracks.sort(byTrackOrder);
  Object.assign(state, { tasks, analysis, questions, tracks });
  // A renamed or removed track would leave an invisible active filter behind.
  if (state.trackFilter && !tracks.some((t) => t.name === state.trackFilter) && !tasks.some((t) => t.track === state.trackFilter)) {
    state.trackFilter = null;
  }
  // Do not wipe a half-written answer or feedback; render once focus leaves.
  if (document.activeElement?.matches("#view textarea, #view input")) {
    state.pending = true;
    renderChrome();
    return;
  }
  render();
}

let reloadTimer = null;
function scheduleReload() {
  clearTimeout(reloadTimer);
  reloadTimer = setTimeout(() => load().catch(showError), 150);
}

// SSE over fetch: EventSource cannot send the Authorization header.
async function listen() {
  for (;;) {
    try {
      const res = await fetch("/api/events", { headers: { Authorization: `Bearer ${state.token}` } });
      if (res.status === 401) return tokenRejected();
      if (!res.ok) throw new Error(`events: ${res.status}`);
      setLive(true);
      await load();
      const reader = res.body.getReader();
      const decoder = new TextDecoder();
      let buffer = "";
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        let end;
        while ((end = buffer.indexOf("\n\n")) >= 0) {
          const block = buffer.slice(0, end);
          buffer = buffer.slice(end + 2);
          if (/^event: /m.test(block)) scheduleReload();
        }
      }
    } catch (_) { /* reconnect below */ }
    setLive(false);
    await new Promise((r) => setTimeout(r, 3000));
  }
}

// ─── DOM helpers ─────────────────────────────────────────────────────
function h(tag, attrs, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs || {})) {
    if (value === null || value === undefined || value === false) continue;
    if (key === "class") el.className = value;
    else if (key.startsWith("on")) el.addEventListener(key.slice(2), value);
    else el.setAttribute(key, value === true ? "" : value);
  }
  for (const child of children.flat(Infinity)) {
    if (child === null || child === undefined || child === false) continue;
    el.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return el;
}

const $ = (id) => document.getElementById(id);

function showError(err) { toast(err.message || String(err), true); }

let toastTimer = null;
function toast(message, error = false) {
  const el = $("toast");
  el.textContent = message;
  el.className = `toast show${error ? " error" : ""}`;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { el.className = "toast"; }, error ? 6000 : 2500);
}

function empty(label, text) {
  return h("div", { class: "empty" }, h("span", { class: "label" }, label), text);
}

function setLive(live) {
  state.live = live;
  $("live-dot").className = `pulse-dot ${live ? "live" : "offline"}`;
  const label = $("live-label");
  label.textContent = live ? "LIVE" : "OFFLINE";
  label.className = live ? "live" : "offline";
}

// ─── Derived data ────────────────────────────────────────────────────
const taskById = (id) => state.tasks.find((t) => t.id === id);
const analysisById = (id) => state.analysis.tasks.find((t) => t.id === id);
const visible = (task) => !state.role || task.role === state.role;

// critical = 3 … low = 0; unknown levels count as normal.
function priorityRank(level) {
  const i = PRIORITIES.indexOf(level);
  return i < 0 ? priorityRank("normal") : PRIORITIES.length - 1 - i;
}

const trackOf = (task) => (task.track && state.tracks.find((t) => t.name === task.track)) || null;

// A task without a track ranks as a `normal` track.
const trackRank = (task) => priorityRank(trackOf(task)?.priority ?? "normal");

// Role, track, and minimum-priority filters together (Board and Review).
function matchesFilters(task) {
  if (!visible(task)) return false;
  if (state.trackFilter !== null && (task.track || "") !== state.trackFilter) return false;
  if (state.minPriority !== null && priorityRank(task.priority) < priorityRank(state.minPriority)) return false;
  return true;
}

function priorityChip(level) {
  return h("span", { class: `chip p-${level}` }, level);
}

function pips(level) {
  return h("span", { class: `pips p-${level}`, title: `priority ${level}`, "aria-label": `priority ${level}` }, PIPS[level] || PIPS.normal);
}

// Track order everywhere: priority desc, then name. Works on tracks and lanes.
function byTrackOrder(a, b) {
  return priorityRank(b.priority) - priorityRank(a.priority) || (a.name < b.name ? -1 : a.name > b.name ? 1 : 0);
}

// Task priority desc, then oldest first.
function byPriority(a, b) {
  return priorityRank(b.priority) - priorityRank(a.priority)
    || (a.created_at < b.created_at ? -1 : a.created_at > b.created_at ? 1 : 0)
    || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0);
}

// The `task next` order: track rank, task priority, oldest first.
const byNextOrder = (a, b) => trackRank(b) - trackRank(a) || byPriority(a, b);
const now = () => Math.floor(Date.now() / 1000);

function minutesLeft(task) {
  if (!task.lease_expires_at) return null;
  return Math.ceil(Math.max(0, task.lease_expires_at - now()) / 60);
}

function overlapBadges(id) {
  const a = analysisById(id);
  if (!a) return [];
  const badges = [];
  if (a.unknown) badges.push(h("span", { class: "badge badge--unknown", title: "No touch list: overlap unknown" }, "? no touch data"));
  if (a.overlaps.length) {
    const running = a.overlaps.filter((o) => o.occupied).map((o) => o.id);
    const title = a.overlaps.map((o) => `${o.id}: ${o.paths.join(", ")}`).join("\n");
    badges.push(h("span", { class: "badge badge--overlap", title }, `⚠ ${a.overlaps.length} overlap${a.overlaps.length > 1 ? "s" : ""}`));
    if (running.length) badges.push(h("span", { class: "badge badge--overlap", title }, `⚠ with running ${running.join(", ")}`));
  }
  return badges;
}

function card(task, extraClass = "", onclick, showTrack = true) {
  const running = RUNNING.has(task.status);
  const minutes = minutesLeft(task);
  return h(
    "button",
    {
      type: "button",
      class: `card p-${task.priority} ${running ? "is-running" : ""} ${extraClass}`,
      "data-id": task.id,
      onclick: onclick || (() => openDetail(task.id)),
    },
    h("div", { class: "card__head" },
      h("span", { class: "card__id" }, running ? `▶ ${task.id}` : task.id),
      pips(task.priority)),
    h("div", { class: "card__title" }, task.title),
    h("div", { class: "card__meta" },
      task.role,
      showTrack && task.track ? ` · ${task.track}` : "",
      task.claimed_by ? ` · ${task.claimed_by}` : "",
      minutes !== null ? ` · ${minutes} min` : ""),
    h("div", { class: "badges" },
      task.blocked_by.length ? h("span", { class: "badge", title: "blocked by" }, `← ${task.blocked_by.join(", ")}`) : null,
      task.status !== "done" ? overlapBadges(task.id) : null),
  );
}

// ─── Views ───────────────────────────────────────────────────────────
// ─── Tracks tab ──────────────────────────────────────────────────────
// Course warning, per role R: track T is off course when T has a ready task of R,
// none of R in progress, and a track of strictly lower priority has one of R in
// progress. `in_review` never counts as running. Tasks without a track form the
// "" key at `normal`. Pure: the caller passes the tasks it wants checked (e.g.
// only one role's). Returns Map<trackKey, Array<{ role, ready }>> of flagged
// tracks only.
function courseWarnings(tasks, tracks) {
  const rankOf = (key) => priorityRank(tracks.find((t) => t.name === key)?.priority ?? "normal");
  const roles = [...new Set(tasks.map((t) => t.role))].sort();
  const warnings = new Map();
  for (const role of roles) {
    const mine = tasks.filter((t) => t.role === role);
    const count = (key, status) => mine.filter((t) => (t.track || "") === key && t.status === status).length;
    const keys = [...new Set(mine.map((t) => t.track || ""))];
    const busyRanks = keys.filter((k) => count(k, "in_progress") > 0).map(rankOf);
    for (const key of keys) {
      const ready = count(key, "ready");
      if (ready > 0 && count(key, "in_progress") === 0 && busyRanks.some((r) => r < rankOf(key))) {
        warnings.set(key, [...(warnings.get(key) || []), { role, ready }]);
      }
    }
  }
  return warnings;
}

const BAR_SEGMENTS = [
  ["done", "done", "done"],
  ["in_review", "in review", "review"],
  ["in_progress", "running", "run"],
  ["ready", "ready", "ready"],
  ["blocked", "blocked", "blocked"],
];

function tracksView() {
  const tasks = state.tasks.filter(visible);
  const rows = state.tracks.map((t) => ({ name: t.name, priority: t.priority, description: t.description }));
  for (const name of new Set(tasks.map((t) => t.track || ""))) {
    if (!rows.some((r) => r.name === name)) rows.push({ name, priority: "normal", description: "" });
  }
  rows.sort(byTrackOrder);
  const shown = rows.filter((r) => r.name || tasks.some((t) => !t.track));
  if (!shown.length) return empty("No tracks", "Create tracks with `airlock track add` or over MCP.");
  const warnings = courseWarnings(tasks, state.tracks);

  return h("div", { class: "tracks" }, shown.map((row) => {
    const own = tasks.filter((t) => (t.track || "") === row.name);
    const count = (status) => own.filter((t) => t.status === status).length;
    const running = own.filter((t) => t.status === "in_progress");
    const lastDone = own.filter((t) => t.status === "done").sort((a, b) => b.updated_at - a.updated_at)[0];
    const flagged = warnings.get(row.name) || [];
    const open = () => {
      state.trackFilter = row.name;
      state.tab = "board";
      location.replace("#board");
      render();
    };
    const bar = h("div", { class: "bar", role: "img", "aria-label": own.length ? `${own.length} tasks` : "no tasks" },
      BAR_SEGMENTS.map(([status, name, cls]) => {
        const n = count(status);
        if (!n) return null;
        const seg = h("span", { class: `bar__seg bar__seg--${cls}`, title: `${n} ${name}` });
        seg.style.flexGrow = String(n);
        return seg;
      }));
    return h("div", {
      class: `track-row p-${row.priority}${flagged.length ? " is-offcourse" : ""}`,
      role: "button",
      tabindex: "0",
      title: "Show this track's tasks on the board",
      "data-track": row.name,
      onclick: open,
      onkeydown: (e) => {
        if (e.target !== e.currentTarget) return;
        if (e.key === "Enter" || e.key === " ") { e.preventDefault(); open(); }
      },
    },
      h("div", { class: "track-row__name" },
        priorityChip(row.priority),
        h("b", { class: row.name ? "" : "muted" }, row.name || "no track"),
        row.description ? h("span", { class: "muted swim__desc" }, row.description) : null),
      h("div", { class: "track-row__bar" },
        bar,
        h("div", { class: "track-row__counts" },
          BAR_SEGMENTS.map(([status, name]) => `${count(status)} ${name}`).join(" · "))),
      h("div", { class: "track-row__now" },
        flagged.map((w) => h("div", { class: "course-warning" }, `${w.role}: nothing running, ${w.ready} ready`)),
        running.map((t) => {
          const minutes = minutesLeft(t);
          return h("div", { class: "track-run" }, `▶ ${t.id} ${t.title}`, t.claimed_by ? ` · ${t.claimed_by}` : "", minutes !== null ? ` · ${minutes} min` : "");
        }),
        !running.length && !flagged.length ? h("div", { class: "muted" }, own.length ? "idle" : "no tasks") : null,
        lastDone ? h("div", { class: "muted track-last" }, `last done: ${lastDone.id} ${lastDone.title}`) : null));
  }));
}

function boardView() {
  if (!state.tasks.length) return empty("No tasks", "Add tasks with `airlock task add` or over MCP.");
  const tasks = state.tasks.filter(matchesFilters);
  if (!tasks.length) return empty("No matching tasks", "No task matches the role, track, and priority filters.");
  return state.groupByTrack ? swimlanes(tasks) : flatBoard(tasks);
}

function flatBoard(tasks) {
  return h("div", { class: "columns" },
    STATUS_COLUMNS.map(([status, name]) => {
      const items = tasks.filter((t) => t.status === status).sort(byNextOrder);
      return h("section", { class: "column" },
        h("div", { class: "label" }, h("span", {}, name), h("span", {}, items.length)),
        items.length ? items.map((t) => card(t)) : h("div", { class: "muted" }, "—"));
    }));
}

// One lane per track in track order; "no track" sorts in as a `normal` track named "".
function swimlanes(tasks) {
  const lanes = state.tracks.map((t) => ({ name: t.name, priority: t.priority, description: t.description }));
  lanes.push({ name: "", priority: "normal", description: "" });
  for (const name of new Set(tasks.map((t) => t.track || ""))) {
    if (!lanes.some((l) => l.name === name)) lanes.push({ name, priority: "normal", description: "" });
  }
  lanes.sort(byTrackOrder);

  const cells = [h("div", {})];
  for (const [status, name] of STATUS_COLUMNS) {
    cells.push(h("div", { class: "label swim__colh" }, h("span", {}, name), h("span", {}, tasks.filter((t) => t.status === status).length)));
  }
  for (const lane of lanes) {
    const own = tasks.filter((t) => (t.track || "") === lane.name);
    if (!own.length) continue;
    cells.push(h("div", { class: "swim__lane", title: lane.description || null },
      priorityChip(lane.priority),
      h("b", { class: lane.name ? "" : "muted" }, lane.name || "no track"),
      lane.description ? h("span", { class: "muted swim__desc" }, lane.description) : null));
    for (const [status] of STATUS_COLUMNS) {
      const items = own.filter((t) => t.status === status).sort(byPriority);
      cells.push(h("div", { class: "swim__cell" }, items.map((t) => card(t, "", undefined, false))));
    }
  }
  return h("div", { class: "swim-wrap" }, h("div", { class: "swim" }, cells));
}

function stagesView() {
  const open = state.analysis.tasks.filter((a) => {
    const t = taskById(a.id);
    return t && visible(t);
  });
  if (!open.length) return empty("No open tasks", "Every task is done, or none matches the role filter.");
  const selected = state.selected && analysisById(state.selected);
  const overlapIds = new Set(selected ? selected.overlaps.map((o) => o.id) : []);
  const maxStage = Math.max(...open.map((a) => a.stage));

  const info = selected
    ? h("div", { class: "selection" },
        h("strong", {}, `${selected.id} `),
        selected.unknown
          ? h("span", { class: "warn" }, "has no touch data — overlap unknown")
          : selected.overlaps.length
            ? [
                h("span", { class: "warn" }, `⚠ overlaps ${selected.overlaps.length} task${selected.overlaps.length > 1 ? "s" : ""}:`),
                selected.overlaps.map((o) => h("div", {}, `${o.occupied ? "▶ " : "· "}${o.id} — ${o.paths.join(", ")}${o.occupied ? " (running)" : ""}`)),
              ]
            : h("span", { class: "ok" }, "✓ shares no files with another open task"),
        h("div", { class: "muted" }, "Click again or press Esc to clear."))
    : h("div", { class: "selection muted" }, "Select a task to highlight every open task that touches the same files. Arrows show blocked_by.");

  const columns = [];
  for (let stage = 1; stage <= maxStage; stage++) {
    const items = open.filter((a) => a.stage === stage);
    columns.push(h("section", { class: "column" },
      h("div", { class: "label" }, h("span", {}, stage === 1 ? "Stage 1 · now" : `Stage ${stage}`), h("span", {}, items.length)),
      items.map((a) => {
        const task = taskById(a.id);
        let cls = "";
        if (selected) cls = a.id === selected.id ? "is-selected" : overlapIds.has(a.id) ? "is-overlap" : "is-dim";
        return card(task, cls, () => {
          state.selected = state.selected === a.id ? null : a.id;
          render();
        });
      })));
  }
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  const wrap = h("div", { class: "stages" }, svg, h("div", { class: "columns" }, columns));
  requestAnimationFrame(() => drawArrows(wrap, svg, open));
  return h("div", {}, info, wrap);
}

function drawArrows(wrap, svg, open) {
  svg.replaceChildren();
  const base = wrap.getBoundingClientRect();
  const ids = new Set(open.map((a) => a.id));
  for (const a of open) {
    const to = wrap.querySelector(`[data-id="${CSS.escape(a.id)}"]`);
    if (!to) continue;
    for (const dep of taskById(a.id).blocked_by) {
      if (!ids.has(dep)) continue;
      const from = wrap.querySelector(`[data-id="${CSS.escape(dep)}"]`);
      if (!from) continue;
      const f = from.getBoundingClientRect();
      const t = to.getBoundingClientRect();
      const x1 = f.right - base.left, y1 = f.top + f.height / 2 - base.top;
      const x2 = t.left - base.left, y2 = t.top + t.height / 2 - base.top;
      const dx = Math.max(24, (x2 - x1) / 2);
      const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
      path.setAttribute("d", `M${x1},${y1} C${x1 + dx},${y1} ${x2 - dx},${y2} ${x2 - 6},${y2} l-5,-4 m5,4 l-5,4`);
      if (state.selected && (state.selected === a.id || state.selected === dep)) path.setAttribute("class", "hot");
      svg.append(path);
    }
  }
}

function filesView() {
  const { files, no_touch } = state.analysis;
  const noTouch = no_touch.length
    ? h("p", { class: "muted" }, h("span", { class: "label" }, "No touch data "), no_touch.join(", "), " — overlap unknown")
    : null;
  if (!files.length) {
    return h("div", {},
      empty("No shared files", state.analysis.tasks.length ? "No two open tasks touch the same path." : "There are no open tasks."),
      noTouch);
  }
  const columns = [...new Set(files.flatMap((f) => f.tasks))].sort();
  const running = new Set(state.analysis.tasks.filter((a) => RUNNING.has(a.status)).map((a) => a.id));
  const table = h("table", { class: "matrix" },
    h("thead", {}, h("tr", {}, h("th", {}),
      columns.map((id, col) => h("th", { scope: "col" },
        h("button", {
          type: "button",
          title: taskById(id)?.title || id,
          onclick: () => openDetail(id),
          onmouseenter: () => hoverColumn(table, col, true),
          onmouseleave: () => hoverColumn(table, col, false),
        }, running.has(id) ? `▶ ${id}` : id))))),
    h("tbody", {}, files.map((f) => h("tr", {},
      h("th", { scope: "row" }, f.path, h("span", { class: "count" }, `⚠ ${f.tasks.length}`)),
      columns.map((id) => {
        const hit = f.tasks.includes(id);
        const occupied = f.occupied_by.includes(id);
        return h("td", {
          class: hit ? `hit${occupied ? " occupied" : ""}` : "",
          title: hit ? `${id} touches ${f.path}${occupied ? " (running)" : ""}` : null,
        }, hit ? (occupied ? "▶" : "●") : "");
      })))));
  return h("div", {},
    h("div", { class: "legend muted" },
      h("span", {}, "● open task touches the file"),
      h("span", {}, "▶ running task holds the file"),
      h("span", {}, "⚠ n = open tasks sharing the file"),
      h("span", {}, "All roles")),
    h("div", { class: "matrix-wrap" }, table),
    noTouch);
}

function hoverColumn(table, col, on) {
  for (const row of table.tBodies[0].rows) row.cells[col + 1].classList.toggle("col-hover", on);
}

function reviewView() {
  const items = state.tasks.filter((t) => t.status === "in_review" && matchesFilters(t));
  if (!items.length) return empty("Review queue empty", "Tasks completed with `--review` appear here, unless a filter hides them.");
  return h("div", { class: "list" }, items.map((task) => {
    const feedback = h("textarea", { placeholder: "Feedback (required to reject)", "aria-label": `Feedback for ${task.id}` });
    const reject = h("button", { type: "button", class: "btn btn--danger", disabled: true, onclick: () => act(
      api("POST", `/api/tasks/${encodeURIComponent(task.id)}/reject`, { notes: feedback.value }), `Rejected ${task.id}`) }, "✕ Reject");
    feedback.addEventListener("input", () => { reject.disabled = !feedback.value.trim(); });
    return h("article", { class: "item" },
      h("div", { class: "item__head" },
        h("button", { type: "button", class: "link card__id", onclick: () => openDetail(task.id) }, task.id),
        h("span", { class: "item__title" }, task.title),
        h("span", { class: "label" }, task.role),
        pips(task.priority),
        h("span", { class: "label" }, task.track || "no track")),
      task.notes ? h("p", {}, task.notes) : h("p", { class: "muted" }, "No notes."),
      h("div", { class: "actions" },
        h("div", { class: "field" }, feedback),
        h("button", { type: "button", class: "btn btn--approve", onclick: () => act(
          api("POST", `/api/tasks/${encodeURIComponent(task.id)}/approve`, {}), `Approved ${task.id}`) }, "✓ Approve"),
        reject));
  }));
}

function questionsView() {
  const filters = ["open", "answered", "resolved", "all"];
  const items = state.questions.filter((q) => state.questionFilter === "all" || q.status === state.questionFilter);
  return h("div", {},
    h("div", { class: "filters", role: "group", "aria-label": "Question status" },
      filters.map((f) => h("button", {
        type: "button",
        class: "pill",
        "aria-pressed": String(state.questionFilter === f),
        onclick: () => { state.questionFilter = f; render(); },
      }, `${f} ${f === "all" ? state.questions.length : state.questions.filter((q) => q.status === f).length}`))),
    items.length ? h("div", { class: "list" }, items.map(questionItem)) : empty("No questions", `No ${state.questionFilter === "all" ? "" : state.questionFilter + " "}questions.`));
}

function questionItem(q) {
  const head = h("div", { class: "item__head" },
    h("span", { class: "card__id" }, `#${q.id}`),
    h("span", { class: "label" }, q.topic),
    h("span", { class: "label" }, q.status === "open" ? "● open" : q.status === "answered" ? "◐ answered" : "✓ resolved"),
    h("span", { class: "muted mono" }, `asked by ${q.asked_by}`));
  const body = [h("p", {}, q.question)];
  if (q.answer) body.push(h("p", { class: "muted" }, `→ ${q.answer}`, q.answered_by ? ` (${q.answered_by})` : ""));
  if (q.spec_file) body.push(h("p", { class: "muted mono" }, `spec: ${q.spec_file}`));
  if (q.status === "resolved") return h("article", { class: "item" }, head, body);

  const answer = h("textarea", { "aria-label": `Answer to question ${q.id}`, placeholder: "Answer in behavioral prose" });
  answer.value = q.answer || "";
  const spec = h("input", { type: "text", "aria-label": "Spec file", placeholder: "Spec file (optional), e.g. docs/spec/melee.md" });
  spec.value = q.spec_file || "";
  const send = h("button", { type: "button", class: "btn", disabled: !answer.value.trim(), onclick: () => act(
    api("POST", `/api/questions/${q.id}/answer`, { answer: answer.value, spec_file: spec.value }),
    `${q.answer ? "Revised" : "Answered"} #${q.id}`) }, q.answer ? "Revise" : "Answer");
  answer.addEventListener("input", () => { send.disabled = !answer.value.trim(); });
  const resolve = q.status === "answered"
    ? h("button", { type: "button", class: "btn btn--approve", onclick: () => act(
        api("POST", `/api/questions/${q.id}/resolve`, { spec_file: spec.value }), `Resolved #${q.id}`) }, "✓ Resolve")
    : null;
  return h("article", { class: "item" }, head, body,
    h("div", { class: "actions" }, h("div", { class: "field" }, answer, spec), send, resolve));
}

function leasesView() {
  const items = state.tasks.filter((t) => t.status === "in_progress");
  if (!items.length) return empty("No active leases", "Claimed tasks appear here with their remaining lease time.");
  return h("div", { class: "list" }, items.map((task) => {
    const minutes = minutesLeft(task);
    const expired = minutes === 0;
    const release = h("button", { type: "button", class: "btn btn--danger" }, "Force release");
    release.addEventListener("click", () => {
      if (release.dataset.armed) {
        act(api("POST", `/api/tasks/${encodeURIComponent(task.id)}/release`, {}), `Released ${task.id}`);
      } else {
        release.dataset.armed = "1";
        release.textContent = "Confirm release";
        setTimeout(() => { delete release.dataset.armed; release.textContent = "Force release"; }, 4000);
      }
    });
    return h("article", { class: "item" },
      h("div", { class: "item__head" },
        h("button", { type: "button", class: "link card__id", onclick: () => openDetail(task.id) }, `▶ ${task.id}`),
        h("span", { class: "item__title" }, task.title)),
      h("div", { class: "actions" },
        h("div", { class: "field" },
          h("span", { class: "label" }, "Session"),
          h("span", { class: "mono" }, task.claimed_by || "?")),
        h("div", { class: "field" },
          h("span", { class: "label" }, "Lease"),
          h("span", { class: `minutes${expired ? " expired" : minutes !== null && minutes <= 5 ? " low" : ""}` },
            minutes === null ? "—" : expired ? "EXPIRED" : `${minutes} min`)),
        release));
  }));
}

// ─── Detail drawer ───────────────────────────────────────────────────
function openDetail(id) {
  state.detail = id;
  renderDrawer();
}

function closeDetail() {
  state.detail = null;
  renderDrawer();
}

function pathList(paths, overlapping) {
  if (!paths.length) return h("div", { class: "muted" }, "—");
  return h("ul", {}, paths.map((p) => {
    const others = overlapping.get(p);
    return h("li", {}, p, others ? h("span", { class: "badge badge--overlap" }, `⚠ ${others.join(", ")}`) : null);
  }));
}

function idLinks(ids) {
  if (!ids.length) return h("div", { class: "muted" }, "—");
  return h("ul", {}, ids.map((id) => h("li", {},
    h("button", { type: "button", class: "link", onclick: () => openDetail(id) }, id),
    taskById(id) ? ` ${taskById(id).title} · ${taskById(id).status}` : "")));
}

function renderDrawer() {
  const drawer = $("drawer");
  const body = $("drawer-body");
  const task = state.detail && taskById(state.detail);
  if (!task) {
    drawer.classList.remove("open");
    drawer.setAttribute("aria-hidden", "true");
    body.replaceChildren();
    return;
  }
  const a = analysisById(task.id);
  const overlapping = new Map();
  for (const o of a?.overlaps || []) {
    for (const p of o.paths) overlapping.set(p, [...(overlapping.get(p) || []), `${o.occupied ? "▶ " : ""}${o.id}`]);
  }
  const touchNorm = (p) => p.trim().replace(/\\/g, "/").replace(/\/{2,}/g, "/").replace(/^(\.\/)+/, "");
  const touchMarks = new Map(task.touch.map((p) => [p, overlapping.get(touchNorm(p))]).filter(([, v]) => v));
  const dependents = state.tasks.filter((t) => t.blocked_by.includes(task.id)).map((t) => t.id);
  const minutes = minutesLeft(task);

  body.replaceChildren(
    h("button", { type: "button", class: "btn close", onclick: closeDetail }, "Close"),
    h("div", { class: "drawer__tags" },
      h("span", { class: "label" }, `${task.role} · ${task.status.replace("_", " ")}`),
      priorityChip(task.priority),
      h("span", { class: "label" }, task.track ? `track ${task.track}` : "no track")),
    h("h2", {}, h("span", { class: "mono" }, task.id), " ", task.title),
    task.claimed_by ? h("div", { class: "mono muted" }, `▶ ${task.claimed_by}${minutes !== null ? ` · ${minutes} min left` : ""}`) : "",
    h("div", { class: "badges" }, task.status !== "done" ? overlapBadges(task.id) : null),
    h("section", {}, h("span", { class: "label" }, "Read"), pathList(task.read, new Map())),
    h("section", {}, h("span", { class: "label" }, "Touch"), pathList(task.touch, touchMarks)),
    h("section", {}, h("span", { class: "label" }, "Do"), h("pre", {}, task.do)),
    h("section", {}, h("span", { class: "label" }, "Verify · never executed by Airlock"), h("pre", {}, task.verify)),
    h("section", {}, h("span", { class: "label" }, "Notes"), task.notes ? h("pre", {}, task.notes) : h("div", { class: "muted" }, "—")),
    h("section", {}, h("span", { class: "label" }, "Blocked by"), idLinks(task.blocked_by)),
    h("section", {}, h("span", { class: "label" }, "Unblocks"), idLinks(dependents)),
  );
  drawer.classList.add("open");
  drawer.setAttribute("aria-hidden", "false");
}

// ─── Actions & rendering ─────────────────────────────────────────────
async function act(promise, message) {
  try {
    await promise;
    toast(message);
    await load();
  } catch (err) {
    showError(err);
  }
}

function renderChrome() {
  const p = state.project;
  $("project-name").textContent = p ? p.name : "";
  if (p) $("version").textContent = `STUDIO // v${p.version.split(".").slice(0, 2).join(".")}`;
  document.title = p ? `Airlock · ${p.name}` : "Airlock";

  const open = state.tasks.filter((t) => t.status !== "done").length;
  const running = state.tasks.filter((t) => t.status === "in_progress").length;
  const shared = state.analysis.files.length;
  $("counts").textContent = `${open} open  ${running} running  ${shared} shared files`;

  const roles = p ? p.roles.map((r) => r.name) : [];
  $("role-filter").replaceChildren(
    ...[null, ...roles].map((role) => h("button", {
      type: "button",
      class: "pill",
      "aria-pressed": String(state.role === role),
      onclick: () => { state.role = role; render(); },
    }, role || "all roles")));

  renderFilters();

  const counts = {
    review: state.tasks.filter((t) => t.status === "in_review").length,
    questions: state.questions.filter((q) => q.status === "open").length,
    leases: running,
  };
  $("tabs").replaceChildren(...TABS.map(([id, name]) => h("button", {
    type: "button",
    class: "pill",
    role: "tab",
    "aria-selected": String(state.tab === id),
    onclick: () => { state.tab = id; location.replace(`#${id}`); render(); },
  }, counts[id] ? `${name} ${counts[id]}` : name)));
}

// Track, minimum-priority, and grouping controls; Board and Review only.
function renderFilters() {
  const slot = $("board-filters");
  const active = state.tab === "board" || state.tab === "review";
  const key = JSON.stringify([active, state.tab === "board", state.tracks.map((t) => t.name), state.trackFilter, state.minPriority, state.groupByTrack]);
  if (slot.dataset.key === key) return;
  slot.dataset.key = key;
  const focused = slot.contains(document.activeElement) ? document.activeElement.id : null;
  if (!active) { slot.replaceChildren(); return; }

  const track = h("select", {
    id: "filter-track",
    class: "select",
    "aria-label": "Filter by track",
    onchange: (e) => {
      const v = e.target.value;
      state.trackFilter = v === "all" ? null : v.slice(2);
      render();
    },
  },
    h("option", { value: "all", selected: state.trackFilter === null }, "all tracks"),
    state.tracks.map((t) => h("option", { value: `t:${t.name}`, selected: state.trackFilter === t.name }, t.name)),
    h("option", { value: "t:", selected: state.trackFilter === "" }, "no track"));

  const min = h("select", {
    id: "filter-priority",
    class: "select",
    "aria-label": "Minimum priority",
    onchange: (e) => {
      state.minPriority = e.target.value === "all" ? null : e.target.value;
      render();
    },
  },
    h("option", { value: "all", selected: state.minPriority === null }, "all priorities"),
    [...PRIORITIES].reverse().map((p) => h("option", { value: p, selected: state.minPriority === p }, `≥ ${p}`)));

  const group = state.tab === "board"
    ? h("button", {
        id: "filter-group",
        type: "button",
        class: "pill switch",
        "aria-pressed": String(state.groupByTrack),
        title: "One swimlane per track",
        onclick: () => {
          state.groupByTrack = !state.groupByTrack;
          saveGroupByTrack(state.groupByTrack);
          render();
        },
      }, "group by track")
    : null;

  slot.replaceChildren(track, min, group);
  if (focused) $(focused)?.focus();
}

function render() {
  if (state.rejected) return;
  renderChrome();
  const views = { tracks: tracksView, board: boardView, stages: stagesView, files: filesView, review: reviewView, questions: questionsView, leases: leasesView };
  $("view").replaceChildren(views[state.tab]());
  renderDrawer();
}

function start() {
  if (!token()) {
    $("view").replaceChildren(empty("No access token", "Open the URL that `airlock serve` printed, including its #token=… part."));
    setLive(false);
    return;
  }
  const tab = location.hash.slice(1);
  if (TABS.some(([id]) => id === tab)) state.tab = tab;
  // Pasting a new URL into this tab only changes the fragment: reload to pick up its token.
  window.addEventListener("hashchange", () => {
    if (/token=/.test(location.hash)) location.reload();
  });
  document.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    if (state.detail) closeDetail();
    else if (state.selected) { state.selected = null; render(); }
  });
  $("view").addEventListener("focusout", () => {
    setTimeout(() => {
      if (state.pending && !document.activeElement?.matches("#view textarea, #view input")) {
        state.pending = false;
        render();
      }
    }, 0);
  });
  let resizeTimer = null;
  window.addEventListener("resize", () => {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => { if (state.tab === "stages") render(); }, 100);
  });
  // Lease minutes tick without server events.
  setInterval(() => { if (state.tab === "leases" || state.tab === "board" || state.tab === "tracks") render(); }, 30000);
  api("GET", "/api/project")
    .then((project) => { state.project = project; render(); listen(); })
    .catch((err) => {
      showError(err);
      $("view").replaceChildren(empty("Cannot reach the API", err.message));
      setLive(false);
    });
}

start();
