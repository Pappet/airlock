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
  ["activity", "Activity"],
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
  reviewSelected: null,
  questionSelected: null,
  live: false,
  // Activity tab: `key` is the entries path last requested; `entries` stays
  // null until the first answer. `session`, `filter` and `problem` are the
  // logbook filters (a problem key overrides the others).
  // `extra`: the unfiltered newest calls, loaded for the "failures & problems" view.
  activity: { key: null, entries: null, extra: [], problems: [], sessions: [], session: null, filter: "all", problem: null },
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
  const headers = { Authorization: `Bearer ${state.token}`, "X-Airlock-Client": "dashboard" };
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

let activityTimer = null;
// Off the Activity tab, a new call only marks its data stale; returning to
// the tab refetches.
function scheduleActivityReload() {
  clearTimeout(activityTimer);
  activityTimer = setTimeout(() => {
    if (state.tab === "activity") loadActivity().catch(showError);
    else state.activity.key = null;
  }, 150);
}

// Entries for the current filters plus all problems and sessions; a response
// to a superseded request is dropped.
async function loadActivity() {
  const path = activityPath();
  state.activity.key = path;
  // The failures view also shows the successful calls behind problems.
  const extraPath = state.activity.filter === "problems" && !selectedProblem() ? activityPath(false) : null;
  const [entries, problems, sessions, extra] = await Promise.all([
    api("GET", path),
    api("GET", "/api/activity/problems"),
    api("GET", "/api/activity/sessions"),
    extraPath ? api("GET", extraPath) : [],
  ]);
  if (state.activity.key !== path) return;
  Object.assign(state.activity, { entries, problems, sessions, extra });
  if (state.tab === "activity") render();
}

// SSE over fetch: EventSource cannot send the Authorization header.
async function listen() {
  for (;;) {
    try {
      const res = await fetch("/api/events", { headers: { Authorization: `Bearer ${state.token}`, "X-Airlock-Client": "dashboard" } });
      if (res.status === 401) return tokenRejected();
      if (!res.ok) throw new Error(`events: ${res.status}`);
      setLive(true);
      state.activity.key = null; // refetch the Activity tab on (re)connect
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
          const name = block.match(/^event: (.*)$/m)?.[1];
          // A logged call changes only the Activity tab; its own task or
          // question events follow separately.
          if (name === "activity") scheduleActivityReload();
          else if (name) {
            scheduleReload();
            if (name === "resync") scheduleActivityReload();
          }
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
  $("live-dot").className = `pulse ${live ? "live" : "offline"}`;
  $("live").className = `live ${live ? "is-live" : "is-offline"}`;
  $("live-label").textContent = live ? "LIVE" : "OFFLINE";
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

// Task ids as people read them: digit runs compare as numbers (E-2 before E-10).
// Mirrors `core::cmp_task_ids`; ties such as E-02 / E-2 fall back to the text.
function cmpTaskIds(a, b) {
  const isDigit = (c) => c >= "0" && c <= "9";
  let i = 0, j = 0;
  while (i < a.length && j < b.length) {
    if (isDigit(a[i]) && isDigit(b[j])) {
      let ei = i, ej = j;
      while (ei < a.length && isDigit(a[ei])) ei++;
      while (ej < b.length && isDigit(b[ej])) ej++;
      const x = a.slice(i, ei).replace(/^0+/, ""), y = b.slice(j, ej).replace(/^0+/, "");
      const order = x.length - y.length || (x < y ? -1 : x > y ? 1 : 0);
      if (order) return order;
      i = ei; j = ej;
    } else {
      if (a[i] !== b[j]) return a[i] < b[j] ? -1 : 1;
      i++; j++;
    }
  }
  return (a.length - i) - (b.length - j) || (a < b ? -1 : a > b ? 1 : 0);
}

// Task priority desc, then oldest first.
function byPriority(a, b) {
  return priorityRank(b.priority) - priorityRank(a.priority)
    || (a.created_at < b.created_at ? -1 : a.created_at > b.created_at ? 1 : 0)
    || cmpTaskIds(a.id, b.id);
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
  if (a.unknown) badges.push(h("span", { class: "badge badge--unknown", title: "No touch list: overlap unknown" }, "? no touch list"));
  if (a.overlaps.length) {
    // Colour budget: red only when the other task is running right now.
    const running = a.overlaps.filter((o) => o.occupied).map((o) => o.id);
    const title = a.overlaps.map((o) => `${o.id}: ${o.paths.join(", ")}`).join("\n");
    badges.push(running.length
      ? h("span", { class: "badge badge--conflict", title }, `⚠ running ${running.join(", ")}`)
      : h("span", { class: "badge", title }, `⧉ ${a.overlaps.length}`));
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
      class: `card ${running ? "is-running" : ""} ${extraClass}`,
      "data-id": task.id,
      onclick: onclick || (() => openDetail(task.id)),
    },
    h("div", { class: "card__head" },
      h("span", { class: "card__id" }, running ? h("span", { class: "pulse live", "aria-hidden": "true" }) : null, task.id),
      pips(task.priority)),
    h("div", { class: "card__title" }, task.title),
    h("div", { class: "card__meta" },
      task.role,
      showTrack && task.track ? ` · ${task.track}` : "",
      task.claimed_by ? [" · ", h("span", { class: "card__who" }, task.claimed_by)] : "",
      minutes !== null ? ` · ${minutes} min` : ""),
    h("div", { class: "badges" },
      task.hold ? h("span", { class: "badge", title: `on hold: ${task.hold}` }, "on hold") : null,
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
  if (!rows.length) return empty("No tracks", "Create tracks with `airlock track add` or over MCP.");
  const warnings = courseWarnings(tasks, state.tracks);

  return h("div", { class: "tracks" }, rows.map((row) => {
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
        h("h3", { class: `track-row__title${row.name ? "" : " muted"}` }, row.name || "no track"),
        row.description ? h("div", { class: "track-row__desc" }, row.description) : null),
      h("div", { class: "track-row__bar" },
        bar,
        h("div", { class: "track-row__mini" },
          BAR_SEGMENTS.map(([status, name]) => {
            const n = count(status);
            return h("span", { class: n ? "" : "is-zero" }, h("b", {}, n), ` ${name}`);
          }))),
      h("div", { class: "track-row__now" },
        flagged.map((w) => h("div", { class: "course-warning" }, `${w.role}: nothing running, ${w.ready} ready`)),
        running.map((t) => {
          const minutes = minutesLeft(t);
          return h("div", { class: "runrow" },
            h("span", { class: "pulse live", "aria-hidden": "true" }),
            h("span", { class: "runrow__id" }, t.id),
            h("span", { class: "runrow__title" }, t.title),
            h("span", { class: "runrow__meta" }, [t.claimed_by, minutes !== null ? `${minutes} min` : null].filter(Boolean).join(" · ")));
        }),
        !running.length && !flagged.length ? h("div", { class: "track-row__idle" }, own.length ? "idle" : "no tasks") : null,
        lastDone ? h("div", { class: "track-last" }, "last done ", h("b", {}, lastDone.id), ` ${lastDone.title}`) : null));
  }));
}

// ─── Activity helpers (pure; times are unix seconds, shown in local time) ───
const CHANNELS = ["mcp", "cli", "rest"];
const pad2 = (n) => String(n).padStart(2, "0");

function localDay(at) {
  const d = new Date(at * 1000);
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
}

function clock(at) {
  const d = new Date(at * 1000);
  return `${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
}

// "Today", "Yesterday", otherwise the local date.
function dayLabel(at, now) {
  const day = localDay(at);
  if (day === localDay(now)) return "Today";
  const d = new Date(now * 1000);
  const yesterday = new Date(d.getFullYear(), d.getMonth(), d.getDate() - 1);
  return day === localDay(yesterday.getTime() / 1000) ? "Yesterday" : day;
}

// Day and clock time of a moment, short for today.
function when(at, now) {
  const label = dayLabel(at, now);
  return label === "Today" ? clock(at) : `${label === "Yesterday" ? "yesterday" : label} ${clock(at)}`;
}

function ago(at, now) {
  const minutes = Math.max(0, Math.floor((now - at) / 60));
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} min ago`;
  if (minutes < 1440) return `${Math.floor(minutes / 60)} h ago`;
  return `${Math.floor(minutes / 1440)} d ago`;
}

// Entries, in the order given, as runs of one local day: [{ label, entries }].
function groupByDay(entries, now) {
  const groups = [];
  for (const entry of entries) {
    const label = dayLabel(entry.at, now);
    const last = groups[groups.length - 1];
    if (last && last.label === label) last.entries.push(entry);
    else groups.push({ label, entries: [entry] });
  }
  return groups;
}

// Two entry lists as one, without duplicates, newest first.
function mergeEntries(a, b) {
  const byId = new Map([...a, ...b].map((e) => [e.id, e]));
  return [...byId.values()].sort((x, y) => y.at - x.at || y.id - x.id);
}

// Session rows: [{ session, role, calls: [[channel, n]] (every channel, n may be 0), ago }].
function sessionChips(sessions, now) {
  return sessions.map((s) => ({
    session: s.session,
    role: s.role,
    calls: CHANNELS.map((c) => [c, s.calls[c] || 0]),
    ago: ago(s.last_at, now),
  }));
}

// Blocked tasks grouped by their unfinished prerequisites, in task order:
// [{ blocked: [id], waitingOn: [{ id, role, holder }] }]. `role` limits the
// blocked tasks; prerequisites of every role count. An unknown prerequisite
// counts as unfinished with role null.
function blockers(tasks, role = null) {
  const byId = new Map(tasks.map((t) => [t.id, t]));
  const groups = new Map();
  for (const task of tasks) {
    if (task.status !== "blocked" || (role && task.role !== role)) continue;
    const open = task.blocked_by.filter((id) => byId.get(id)?.status !== "done").sort(cmpTaskIds);
    if (!open.length) continue;
    const key = open.join(" ");
    if (!groups.has(key)) {
      groups.set(key, {
        blocked: [],
        waitingOn: open.map((id) => ({ id, role: byId.get(id)?.role ?? null, holder: byId.get(id)?.claimed_by ?? null })),
      });
    }
    groups.get(key).blocked.push(task.id);
  }
  return [...groups.values()];
}

// ─── Activity view ───────────────────────────────────────────────────
const SEVERE = new Set(["failure", "error_burst"]);
// The server's default length of `GET /api/activity`.
const ACTIVITY_LIMIT = 500;
const LOG_FILTERS = [
  ["all", "all"],
  ["writes", "writes only"],
  ["problems", "failures & problems"],
  ["mcp", "MCP"],
  ["cli", "CLI"],
  ["rest", "REST"],
];
// With a role selected, rows without a role or with another one are hidden.
const roleVisible = (item) => !state.role || item.role === state.role;
// Problems of one kind never share entries, so the oldest entry names one.
const problemKey = (p) => `${p.kind}:${p.entry_ids[0]}`;

// A session filter whose chip the role filter hides is dropped.
function dropHiddenSession() {
  const a = state.activity;
  if (!a.session || !state.role) return;
  const session = a.sessions.find((s) => s.session === a.session);
  if (!session || !roleVisible(session)) a.session = null;
}

function selectedProblem() {
  const key = state.activity.problem;
  return key ? state.activity.problems.find((p) => p.entry_ids.length && problemKey(p) === key && roleVisible(p)) || null : null;
}

// A selected problem loads its own entries; otherwise session, role, channel,
// writes and failures filter on the server.
function activityPath(failuresOnly = true) {
  const a = state.activity;
  const q = new URLSearchParams();
  const problem = selectedProblem();
  if (problem) {
    q.set("since", String(problem.first_at));
    if (problem.session) q.set("session", problem.session);
    q.set("limit", "5000");
  } else {
    if (a.session) q.set("session", a.session);
    if (state.role) q.set("role", state.role);
    if (CHANNELS.includes(a.filter)) q.set("channel", a.filter);
    if (a.filter === "writes") q.set("writes", "true");
    if (a.filter === "problems" && failuresOnly) q.set("failures", "true");
  }
  return `/api/activity?${q}`;
}

function channelBadge(channel) {
  return h("span", { class: `ch ch--${channel}` }, channel.toUpperCase());
}

function problemText(p) {
  const who = p.session && !p.message.includes(p.session) ? `${p.session} · ` : "";
  return who + problemBody(p);
}

// The message without the session, which the row shows in its own colour.
function problemBody(p) {
  if (p.kind !== "failure") return p.message;
  return `${p.command || "call"}${p.target ? ` ${p.target}` : ""} → ${p.message}`;
}

function problemRow(p, selected, t) {
  const severe = SEVERE.has(p.kind);
  const content = [
    h("span", { class: `act-glyph ${severe ? "bad" : "warn"}`, "aria-hidden": "true" }, severe ? "✕" : "▲"),
    h("span", { class: "act-problem__text" },
      p.session && !p.message.includes(p.session) ? [h("span", { class: "act-session" }, p.session), " · "] : null,
      problemBody(p),
      p.count > 1 ? h("span", { class: "muted" }, ` · ${p.count}×`) : null),
    h("span", { class: "act-when" }, when(p.last_at, t)),
  ];
  if (!p.entry_ids.length) return h("div", { class: "act-problem" }, content);
  const key = problemKey(p);
  return h("button", {
    type: "button",
    class: "act-problem",
    "aria-pressed": String(selected === p),
    title: "Show this problem's calls in the logbook",
    onclick: () => {
      state.activity.problem = state.activity.problem === key ? null : key;
      render();
    },
  }, content);
}

function problemsPanel(problems, selected, t) {
  return h("section", { class: "act-panel" },
    h("div", { class: `label${problems.length ? " is-bad" : ""}` }, problems.length ? `Needs attention · ${problems.length}` : "Needs attention"),
    problems.length
      ? problems.map((p) => problemRow(p, selected, t))
      : h("div", { class: "act-dim" }, "No problems in the last 30 days."));
}

// Prerequisites grouped by (role, holder): [{ ids, role, holder }], for the
// "waiting on" and "who" cells of a blocker row.
function waitingOn(list) {
  const byHolder = new Map();
  for (const w of list) {
    const key = JSON.stringify([w.role, w.holder]);
    byHolder.set(key, [...(byHolder.get(key) || []), w.id]);
  }
  return [...byHolder].map(([key, ids]) => {
    const [role, holder] = JSON.parse(key);
    return { ids, role, holder };
  });
}

function statePanel() {
  const claimed = state.tasks
    .filter((t) => t.status === "in_progress" && t.claimed_by && visible(t))
    .sort((a, b) => (a.claimed_by < b.claimed_by ? -1 : a.claimed_by > b.claimed_by ? 1 : 0));
  const blocked = blockers(state.tasks, state.role);
  const idLink = (id) => h("button", { type: "button", class: "link", onclick: () => openDetail(id) }, id);
  // `nobody on it` is only a warning when an important track waits.
  const hot = (g) => g.blocked.some((id) => priorityRank(trackOf(taskById(id))?.priority ?? "normal") >= priorityRank("high"));
  return h("section", { class: "act-panel" },
    h("div", { class: "label" }, "Claimed"),
    claimed.length
      ? claimed.map((t) => {
          const minutes = minutesLeft(t);
          const lease = minutes === null ? "no lease" : minutes === 0 ? "lease expired" : `lease ${minutes} min`;
          return h("div", { class: "act-claim" }, h("span", { class: "act-session" }, t.claimed_by), idLink(t.id),
            h("span", { class: "muted" }, `${t.track || "no track"} · ${lease}`));
        })
      : h("div", { class: "act-dim" }, "Nothing claimed."),
    h("div", { class: "label" }, "Blocked · waiting on"),
    blocked.length
      ? blocked.map((g, i) => {
          const groups = waitingOn(g.waitingOn);
          return h("div", { class: i % 2 ? "blk is-alt" : "blk" },
            h("span", { class: "blk__a" }, g.blocked.map((id, j) => [j ? ", " : "", idLink(id)])),
            h("span", { class: "blk__arr", "aria-hidden": "true" }, "←"),
            h("span", { class: "blk__b" }, groups.map((w) => h("div", {}, `${w.ids.join(", ")} · ${w.role ?? "unknown task"}`))),
            h("span", { class: "blk__who" }, groups.map((w) => h("div", {},
              w.holder ? h("span", { class: "act-session" }, w.holder) : h("span", { class: `blk__nobody${hot(g) ? " is-hot" : ""}` }, "nobody on it")))));
        })
      : h("div", { class: "act-dim" }, "Nothing blocked."));
}

function sessionBar(t) {
  const a = state.activity;
  const pick = (session) => () => { a.session = session; a.problem = null; render(); };
  // Aligned rows that double as the logbook's session filter (concept §3 Activity).
  return h("div", { class: "act-sessions", role: "group", "aria-label": "Sessions" },
    h("div", { class: "label" }, "Sessions"),
    h("button", { type: "button", class: "act-chip act-chip--all", "aria-pressed": String(!a.session), onclick: pick(null) }, h("b", {}, "all sessions")),
    sessionChips(a.sessions.filter(roleVisible), t).map((c) => h("button", {
      type: "button",
      class: "act-chip",
      "aria-pressed": String(a.session === c.session),
      onclick: pick(a.session === c.session ? null : c.session),
    },
      h("b", {}, c.session),
      h("span", { class: "muted" }, c.role || "—"),
      c.calls.map(([channel, n]) => n ? h("span", { class: `ch ch--${channel}` }, `${channel.toUpperCase()} ${n}`) : h("span", { class: "act-chip__none" }, "·")),
      h("span", { class: "muted act-chip__ago" }, c.ago))));
}

function logRow(e, mark) {
  return h("div", {
    class: `act-row${mark ? ` is-${mark}` : ""}${e.ok ? "" : " is-failed"}`,
    title: [e.agent, `pid ${e.pid}`, `build ${e.build}`, e.clean_room ? "clean room" : null].filter(Boolean).join(" · "),
  },
    h("span", { class: "act-time" }, clock(e.at)),
    h("span", { class: "act-session" }, e.session),
    channelBadge(e.channel),
    h("span", { class: "act-what" },
      e.command,
      e.target ? ` ${e.target}` : "",
      e.changed.length ? h("span", { class: "act-changed" }, ` · ${e.changed.join(", ")}`) : null,
      e.ok ? null : ` → ${e.error || "failed"}`));
}

function logbook(problems, selected, t) {
  const a = state.activity;
  // Row highlight per entry: a severe problem wins over a warning.
  const marks = new Map();
  for (const p of problems) {
    for (const id of p.entry_ids) if (marks.get(id) !== "bad") marks.set(id, SEVERE.has(p.kind) ? "bad" : "warn");
  }
  let entries;
  if (selected) {
    const ids = new Set(selected.entry_ids);
    entries = a.entries.filter((e) => ids.has(e.id));
  } else {
    entries = a.filter === "problems" ? mergeEntries(a.entries, a.extra.filter((e) => marks.has(e.id))) : a.entries;
    entries = entries.filter(roleVisible);
  }
  const pills = h("div", { class: "filters", role: "group", "aria-label": "Logbook filter" },
    LOG_FILTERS.map(([id, name]) => h("button", {
      type: "button",
      class: "pill",
      "aria-pressed": String(!selected && a.filter === id),
      onclick: () => { a.filter = id; a.problem = null; render(); },
    }, name)));
  const focus = selected
    ? h("div", { class: "act-focus" },
        h("span", {}, `${entries.length} call${entries.length === 1 ? "" : "s"} behind: ${problemText(selected)}`),
        h("button", { type: "button", class: "link", onclick: () => { a.problem = null; render(); } }, "show all"))
    : null;
  return h("section", { class: "act-log" },
    h("div", { class: "act-loghead" }, h("div", { class: "label" }, "Logbook"), pills),
    focus,
    entries.length
      ? groupByDay(entries, t).map((g) => h("div", {},
          h("div", { class: "label act-day" }, g.label),
          g.entries.map((e) => logRow(e, marks.get(e.id)))))
      : h("div", { class: "muted act-none" }, "No calls match."),
    !selected && a.entries.length >= ACTIVITY_LIMIT ? h("div", { class: "muted act-none" }, `Newest ${ACTIVITY_LIMIT} calls shown.`) : null);
}

function activityView() {
  const a = state.activity;
  if (!a.entries) return empty("Activity", "Loading…");
  const t = now();
  const problems = a.problems.filter(roleVisible);
  const selected = selectedProblem();
  return h("div", { class: "activity" },
    h("div", { class: "act-strip" }, problemsPanel(problems, selected, t), statePanel()),
    sessionBar(t),
    logbook(problems, selected, t));
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
      return h("section", { class: `column${status === "in_progress" ? " column--run" : ""}` },
        h("div", { class: "label" }, h("span", {}, name), h("b", {}, items.length)),
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
  let row = 0;
  for (const lane of lanes) {
    const own = tasks.filter((t) => (t.track || "") === lane.name);
    if (!own.length) continue;
    const alt = row++ % 2 ? " swim__row--alt" : "";
    cells.push(h("div", { class: `swim__lane${alt}`, title: lane.description || null },
      priorityChip(lane.priority),
      h("b", { class: lane.name ? "" : "muted" }, lane.name || "no track"),
      lane.description ? h("span", { class: "muted swim__desc" }, lane.description) : null));
    for (const [status] of STATUS_COLUMNS) {
      const items = own.filter((t) => t.status === status).sort(byPriority);
      cells.push(h("div", { class: `swim__cell${alt}` }, items.map((t) => card(t, "", undefined, false))));
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
                h("div", { class: "selection__list" },
                  selected.overlaps.map((o) => h("div", {}, `${o.occupied ? "▶ " : "· "}${o.id} — ${o.paths.join(", ")}${o.occupied ? " (running)" : ""}`))),
              ]
            : h("span", { class: "ok" }, "✓ shares no files with another open task"),
        h("div", { class: "muted" }, "Click again or press Esc to clear."))
    : h("div", { class: "selection muted" }, "Select a task to highlight every open task that touches the same files. Arrows show blocked_by.");

  const columns = [];
  for (let stage = 1; stage <= maxStage; stage++) {
    const items = open.filter((a) => a.stage === stage);
    columns.push(h("section", { class: "column" },
      h("div", { class: "label" }, h("span", {}, stage === 1 ? "Stage 1 · now" : `Stage ${stage}`), h("b", {}, items.length)),
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
  const columns = [...new Set(files.flatMap((f) => f.tasks))].sort(cmpTaskIds);
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
      h("th", { scope: "row" }, f.path, h("span", { class: f.occupied_by.length ? "count is-hot" : "count" }, f.tasks.length)),
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
      h("span", { class: "legend__hot" }, "▶ running task holds the file"),
      h("span", {}, "n open tasks share the file"),
      h("span", {}, "All roles")),
    h("div", { class: "matrix-wrap" }, table),
    noTouch);
}

function hoverColumn(table, col, on) {
  for (const row of table.tBodies[0].rows) row.cells[col + 1].classList.toggle("col-hover", on);
}

// Split panel: compact rows on the left, the selected item on the right.
// `rows` = [{ id, cells }]; an unknown `selected` falls back to the first row.
function splitView({ rows, selected, onSelect, detail, emptyLabel, emptyText }) {
  if (!rows.length) return empty(emptyLabel, emptyText);
  const id = rows.some((r) => r.id === selected) ? selected : rows[0].id;
  return h("div", { class: "split" },
    h("aside", { class: "split__list", role: "group", "aria-label": "Items" },
      rows.map((r) => h("button", {
        type: "button",
        class: "split__row",
        "aria-pressed": String(r.id === id),
        onclick: () => onSelect(r.id),
      }, r.cells))),
    h("section", { class: "split__detail" }, detail(id)));
}

function reviewView() {
  const items = state.tasks.filter((t) => t.status === "in_review" && matchesFilters(t));
  return splitView({
    rows: items.map((task) => ({
      id: task.id,
      cells: [h("span", { class: "split__id" }, task.id), h("span", { class: "split__text" }, task.title), pips(task.priority)],
    })),
    selected: state.reviewSelected,
    onSelect: (id) => { state.reviewSelected = id; render(); },
    detail: (id) => reviewDetail(taskById(id)),
    emptyLabel: "Review queue empty",
    emptyText: "Tasks completed with `--review` appear here, unless a filter hides them.",
  });
}

function reviewDetail(task) {
  const feedback = h("textarea", { placeholder: "Feedback (required to reject)", "aria-label": `Feedback for ${task.id}` });
  const reject = h("button", { type: "button", class: "btn btn--danger", disabled: true, onclick: () => act(
    api("POST", `/api/tasks/${encodeURIComponent(task.id)}/reject`, { notes: feedback.value }), `Rejected ${task.id}`) }, "✕ Reject");
  feedback.addEventListener("input", () => { reject.disabled = !feedback.value.trim(); });
  return [
    h("div", { class: "split__meta" },
      h("button", { type: "button", class: "link split__id", onclick: () => openDetail(task.id) }, task.id),
      h("span", { class: "label" }, task.role),
      pips(task.priority),
      h("span", { class: "label" }, task.track || "no track")),
    h("h2", { class: "split__title" }, task.title),
    task.notes ? h("p", { class: "split__body" }, task.notes) : h("p", { class: "muted" }, "No notes."),
    h("div", { class: "actions" },
      h("div", { class: "field" }, feedback),
      h("button", { type: "button", class: "btn btn--approve", onclick: () => act(
        api("POST", `/api/tasks/${encodeURIComponent(task.id)}/approve`, {}), `Approved ${task.id}`) }, "✓ Approve"),
      reject),
  ];
}

const QUESTION_STATUS = { open: "● open", answered: "◐ answered", resolved: "✓ resolved" };

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
    splitView({
      rows: items.map((q) => ({
        id: q.id,
        cells: [
          h("span", { class: "split__id" }, `#${q.id}`),
          h("span", { class: "split__text" }, q.question),
          h("span", { class: `split__status${q.status === "open" ? " is-open" : ""}` }, QUESTION_STATUS[q.status] || q.status),
        ],
      })),
      selected: state.questionSelected,
      onSelect: (id) => { state.questionSelected = id; render(); },
      detail: (id) => questionDetail(state.questions.find((q) => q.id === id)),
      emptyLabel: "No questions",
      emptyText: `No ${state.questionFilter === "all" ? "" : state.questionFilter + " "}questions.`,
    }));
}

function questionDetail(q) {
  const t = now();
  const meta = h("div", { class: "split__meta" },
    h("span", { class: "split__id" }, `#${q.id}`),
    h("span", { class: "label" }, q.topic),
    h("span", { class: `label${q.status === "open" ? " is-open" : ""}` }, QUESTION_STATUS[q.status] || q.status),
    q.corrected_at ? h("span", { class: "chip", title: `Corrected by ${q.corrected_by || "unknown"}; ${(q.history || []).length} earlier answer(s) kept` }, "corrected") : null,
    h("span", { class: "muted mono" }, `asked by ${q.asked_by} · ${ago(q.created_at, t)}`));
  const body = [h("p", { class: "split__body" }, q.question)];
  if (q.answer) body.push(h("p", { class: "split__body muted" }, `→ ${q.answer}`, q.answered_by ? ` (${q.answered_by})` : ""));
  if (q.spec_file) body.push(h("p", { class: "muted mono" }, `spec: ${q.spec_file}`));
  if (q.status === "resolved") return [meta, body];

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
  return [meta, body, h("div", { class: "actions" }, h("div", { class: "field" }, answer, spec), send, resolve)];
}

function leasesView() {
  const items = state.tasks.filter((t) => t.status === "in_progress").sort(byNextOrder);
  if (!items.length) return empty("No active leases", "Claimed tasks appear here with their remaining lease time.");
  const window = state.project?.lease_minutes || 60;
  return h("div", { class: "leases" }, items.map((task) => {
    const minutes = minutesLeft(task);
    const expired = minutes === 0;
    const low = minutes !== null && minutes < 15;
    const pct = minutes === null ? 100 : Math.min(100, Math.round((100 * minutes) / window));
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
    return h("article", { class: `lease-item${expired ? " is-expired" : low ? " is-low" : ""}` },
      h("div", { class: "lease-item__head" },
        h("span", { class: "pulse live", "aria-hidden": "true" }),
        h("button", { type: "button", class: "link lease-item__id", onclick: () => openDetail(task.id) }, task.id),
        h("span", { class: "lease-item__title" }, task.title),
        h("span", { class: "lease__who" }, task.claimed_by || "?"),
        h("span", { class: "lease__left" }, minutes === null ? "—" : expired ? "EXPIRED" : `${minutes} min`),
        h("span", { class: "lease__seam" }, h("i", { style: `width:${pct}%` }))),
      task.progress ? h("pre", { class: "lease-item__progress" }, task.progress) : null,
      h("div", { class: "lease-item__actions" }, release));
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
    const running = others?.some((id) => id.startsWith("▶"));
    return h("li", {}, p, others ? h("span", { class: `badge${running ? " badge--conflict" : ""}` }, `${running ? "⚠" : "⧉"} ${others.join(", ")}`) : null);
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
  // Same rule as the server (Spec 04 §3.5): a directory entry covers the paths beneath it.
  const stem = (p) => p.replace(/\/$/, "");
  const covers = (a, b) => stem(a) === stem(b) || stem(b).startsWith(`${stem(a)}/`);
  const touchMarks = new Map(task.touch.map((p) => {
    const entry = touchNorm(p);
    const ids = [...overlapping].filter(([path]) => covers(entry, path) || covers(path, entry)).flatMap(([, v]) => v);
    return [p, ids.length ? [...new Set(ids)] : null];
  }).filter(([, v]) => v));
  const dependents = state.tasks.filter((t) => t.blocked_by.includes(task.id)).map((t) => t.id);
  const minutes = minutesLeft(task);

  body.replaceChildren(
    h("div", { class: "drawer__head" },
      h("div", { class: "drawer__tags" },
        h("span", { class: "label" }, `${task.role} · ${task.status.replace("_", " ")}`),
        priorityChip(task.priority),
        h("span", { class: "label" }, task.track ? `track ${task.track}` : "no track")),
      h("button", { type: "button", class: "btn close", onclick: closeDetail }, "Close")),
    h("h2", {}, h("span", { class: "mono" }, task.id), " ", task.title),
    task.claimed_by
      ? h("div", { class: "drawer__claim" },
          h("span", { class: "pulse live", "aria-hidden": "true" }),
          h("span", { class: "act-session" }, task.claimed_by),
          minutes !== null ? h("span", { class: "muted" }, `· ${minutes} min left`) : null)
      : null,
    h("div", { class: "badges" }, task.status !== "done" ? overlapBadges(task.id) : null),
    h("div", { class: "drawer__paths" },
      h("section", {}, h("span", { class: "label" }, "Read"), pathList(task.read, new Map())),
      h("section", {}, h("span", { class: "label" }, "Touch"), pathList(task.touch, touchMarks))),
    h("section", {}, h("span", { class: "label" }, "Do"), h("pre", {}, task.do)),
    h("section", {}, h("span", { class: "label" }, "Verify · never executed by Airlock"), h("pre", {}, task.verify)),
    h("section", {}, h("span", { class: "label" }, "Notes"), task.notes ? h("pre", {}, task.notes) : h("div", { class: "muted" }, "—")),
    task.hold ? h("section", {}, h("span", { class: "label" }, "On hold"), h("pre", {}, task.hold)) : "",
    task.progress ? h("section", {}, h("span", { class: "label" }, task.status === "in_progress" ? "Progress" : "Earlier progress"), h("pre", {}, task.progress)) : "",
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

  const running = state.tasks.filter((t) => t.status === "in_progress").length;
  $("hero-stats").replaceChildren(...heroStats());
  $("hero-now").replaceChildren(...heroNow());

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
    class: `tab${state.tab === id ? " on" : ""}`,
    role: "tab",
    "aria-selected": String(state.tab === id),
    onclick: () => { state.tab = id; location.replace(`#${id}`); render(); },
  }, name, counts[id] ? h("span", { class: "tab__n" }, counts[id]) : null)));
}

// ─── Hero band: the state of the airlock, visible on every tab ──────
const HERO_ROWS = 3;

function heroStats() {
  const count = (status) => state.tasks.filter((t) => t.status === status).length;
  const stat = (value, label, cls = "") => h("div", { class: `stat ${cls}` }, h("b", {}, value), h("span", {}, label));
  return [
    stat(state.tasks.filter((t) => t.status !== "done").length, "open"),
    stat(count("in_progress"), "running", "stat--hot"),
    stat(count("in_review"), "in review"),
    stat(state.questions.filter((q) => q.status === "open").length, "open questions", "stat--info"),
    stat(state.analysis.files.length, "shared files"),
  ];
}

// Running tasks as lease rows; the seam shows the lease window left, so a
// heartbeat-extended lease (longer than one window) reads as full.
function heroNow() {
  const running = state.tasks.filter((t) => t.status === "in_progress").sort(byNextOrder);
  if (!running.length) return [h("div", { class: "now__idle" }, "nothing running")];
  const window = state.project?.lease_minutes || 60;
  const rows = running.slice(0, HERO_ROWS).map((t) => {
    const minutes = minutesLeft(t);
    const low = minutes !== null && minutes < 15;
    const pct = minutes === null ? 100 : Math.min(100, Math.round((100 * minutes) / window));
    return h("button", { type: "button", class: `lease${low ? " is-low" : ""}`, onclick: () => openDetail(t.id) },
      h("span", { class: "pulse live", "aria-hidden": "true" }),
      h("span", { class: "lease__id" }, t.id),
      h("span", { class: "lease__title" }, t.title),
      h("span", { class: "lease__who" }, t.claimed_by || ""),
      h("span", { class: "lease__left" }, minutes === null ? "" : `${minutes} min`),
      h("span", { class: "lease__seam" }, h("i", { style: `width:${pct}%` })));
  });
  if (running.length > HERO_ROWS) {
    rows.push(h("button", {
      type: "button",
      class: "now__more",
      onclick: () => { state.tab = "leases"; location.replace("#leases"); render(); },
    }, `+${running.length - HERO_ROWS} more`));
  }
  return rows;
}

// Track, minimum-priority, and grouping controls; Board and Review only.
const WIDE = matchMedia("(min-width: 1101px)");

function renderFilters() {
  const slot = $("board-filters");
  const active = state.tab === "board" || state.tab === "review";
  const wide = WIDE.matches;
  const key = JSON.stringify([active, wide, state.tab === "board", state.tracks.map((t) => t.name), state.trackFilter, state.minPriority, state.groupByTrack]);
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

  // Below 1100px the three controls fold into a "filter" pill (concept §6).
  slot.replaceChildren(h("details", { class: "filter-pop", open: wide },
    h("summary", { class: "pill" }, "filter ▾"),
    h("div", { class: "filter-pop__body" }, track, min, group)));
  if (focused) $(focused)?.focus();
}

function render() {
  if (state.rejected) return;
  renderChrome();
  if (state.tab === "activity") {
    dropHiddenSession();
    if (state.activity.key !== activityPath()) loadActivity().catch(showError);
  }
  const views = { tracks: tracksView, activity: activityView, board: boardView, stages: stagesView, files: filesView, review: reviewView, questions: questionsView, leases: leasesView };
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
  WIDE.addEventListener("change", () => render());
  let resizeTimer = null;
  window.addEventListener("resize", () => {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => { if (state.tab === "stages") render(); }, 100);
  });
  // Lease minutes and "N min ago" tick without server events.
  setInterval(() => { if (["leases", "board", "tracks", "activity"].includes(state.tab)) render(); }, 30000);
  api("GET", "/api/project")
    .then((project) => { state.project = project; render(); listen(); })
    .catch((err) => {
      showError(err);
      $("view").replaceChildren(empty("Cannot reach the API", err.message));
      setLive(false);
    });
}

start();
