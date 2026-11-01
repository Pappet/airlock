# Tracks and a Fixed Priority Scale — Design

**Date:** 31 Oct 2026 · **Status:** approved in conversation, pending written review

## Intent

Reviewing the OpenSHR project, the user found it had drifted off course: work went into comfort features while the core (the battle, faithful to the tick) waited. Airlock could not show this. Tasks have no grouping (themes such as "Comfort track …" or "Divergence …" live only in titles), and priorities are free integers (0–40 in OpenSHR) that say little at a glance and are only shown as a small "P22".

This change makes the direction of work explicit and visible:

1. **Tracks:** named streams of work with their own priority, created on the fly by the lead (through an agent). Example: name `Comfort`, priority `low`, description "Things we COULD build into the engine".
2. **A fixed priority scale** for tracks and tasks instead of free integers.
3. **`task next` follows the tracks:** agents get work from the most important track first.
4. **Dashboard:** a Tracks tab that shows at a glance whether work runs where it should, and a board that can be grouped by track, with a visible priority on every card and filters for track and priority.

## Goals

- A lead session can create, change, rename, and remove tracks at any time (CLI and MCP).
- Every task can belong to one track; tasks without a track stay valid.
- One priority scale everywhere: `critical` > `high` > `normal` > `low`.
- `task next` hands out the ready task from the highest-priority track first.
- The dashboard answers "am I on course?" without reading task lists.

## Non-goals

- **No forms in the dashboard.** Tasks and tracks are created and changed by agents over CLI/MCP. The dashboard only displays them (as today: it has no task editing either).
- **No timeline view.** It was considered (mockup B) and dropped for now; it would need a "done at" timestamp per task.
- No automatic track assignment of existing tasks. The lead assigns them after the upgrade.
- No per-track roles, owners, deadlines, or milestones.

## Anti-goals (would make this a failure even if it works)

- Numbers creeping back in: no numeric priority accepted anywhere, no hidden numeric sub-ordering exposed to users.
- The Tracks tab looking calm while a critical track sits idle and a low one is busy.
- Two spellings of the same thing: the term is **track** everywhere (code, CLI, MCP, REST, dashboard, docs). The word **topic** stays reserved for questions (`question ask --topic`).

## Decisions made with the user

- Term: `track` (OpenSHR already says "Comfort track"; distinct from the questions' `topic`).
- Tracks have their own priority; a track's priority outranks task priority in `task next`.
- Scale: four named levels, shared by tracks and tasks.
- Tracks are created on the fly, not predefined in the project config.
- Dashboard: Tracks tab after mockup A, board after mockup C with a switch "group by track"; no forms.
- The course warning is computed **per role**: it fires only when the same role has ready work in a more important track while working in a less important one. Tracks span roles (OpenSHR has comfort and divergence tasks for both `engine` and `lead`), so a cross-role comparison would raise false alarms.

## Data model

### Priority

`Priority` enum: `critical`, `high`, `normal`, `low` (serialized lowercase in every JSON surface; ordered `critical` highest). Default for new tasks and tracks: `normal`.

Stored in SQLite as its rank (`low` 0, `normal` 1, `high` 2, `critical` 3) in the existing `INTEGER` column, so the migration converts values in place instead of rebuilding the `STRICT` tasks table; every surface outside the store sees only the names.

### Track (new entity, in the hub store)

| Field | Notes |
|---|---|
| `name` | Primary key. Letters, digits, `-`, `_` (same rule as role names); case preserved, unique exactly. |
| `priority` | `Priority`, default `normal` |
| `description` | Free text, may be empty |
| `created_at`, `updated_at` | unix seconds |

### Task

- New field `track: Option<String>`, referencing an existing track.
- `priority` changes from `i32` to `Priority`.

### Migration (store schema v4)

- Create the `tracks` table; add `tasks.track` (nullable).
- Convert task priorities: `>= 30` → `high`, `10..=29` → `normal`, `< 10` (including negatives) → `low`. Never `critical`: that stays a deliberate choice.
- No track is assigned.

## Behaviour

### Ordering (`task next`, Spec 01 §2.1)

Among ready tasks of the role:

1. track priority (a task without track counts as `normal`),
2. task priority,
3. oldest `created_at`,
4. `id` (byte order).

The existing overlap rule stays on top: tasks sharing files with running work are handed out last.

### Track operations

| Operation | CLI | MCP | Rules |
|---|---|---|---|
| list | `airlock track list` | `track_list` | sorted by priority, then name; with task counts per effective status |
| create | `airlock track add <name> [--priority <p>] [--description <d>]` | `track_create` | fails on an existing or invalid name |
| change | `airlock track edit <name> [--name <new>] [--priority <p>] [--description <d>]` | `track_update` | rename updates every task referencing it, in one transaction |
| remove | `airlock track remove <name>` | `track_remove` | refused while any task (any state) uses the track |

Track writes are open to every non-clean-room session, like `task_create` today. Clean-room sessions get none of the track tools; the track name may appear in their work packages (coordination metadata, Spec 03 §2.1).

### Task operations

- `task add … [--track <name>] [--priority <p>]`; MCP `task_create` and `POST /api/tasks` gain `track` and take `priority` as one of the four strings.
- `task edit … [--track <name>] [--clear-track] [--priority <p>]`; MCP `task_update` / `PATCH /api/tasks/{id}`: `track: null` clears it.
- `task list [--track <name>] [--priority <p>]` — `--priority` means "at least this level".
- Unknown track → error naming the defined tracks. A number given as priority → error listing the four levels.

### Briefing, exports, views

- The briefing's work package shows the track and the priority.
- Snapshot (`docs/.airlock-state.json`, Spec 05): `format_version` 2, adds `tracks` (sorted by name); tasks carry `track` and the priority string. Importing a version-1 snapshot converts priorities with the migration mapping above.
- `PLAN.md`: each task line shows `**Track:** … · **Priority:** high`. Grouping stays by role.

## Dashboard

### Priority display (everywhere)

Fixed colors: `critical` red (`--error`), `high` orange (`--tertiary`), `normal` cyan (`--secondary`), `low` grey. Cards show a colored left stripe and pips (`●●●●` critical … `●○○○` low) instead of "P22"; the detail drawer shows a priority chip and the track.

### Tracks tab (new, after mockup A)

- One row per track, sorted by track priority then name; plus a row "no track" (at `normal`) if any task has none.
- Follows the dashboard's role filter like Board, Stages, and Review: counts, bars, and warnings then cover that role's tasks only.
- Row: priority chip, name, description; a segmented bar done / in review / running / ready / blocked with counts; what is running (task, session, lease minutes left); the most recently finished task (by `updated_at`).
- **Course warning (per role):** for a role R, a track T is off course when T has ≥1 ready task of R, no task of R in progress, and some track with strictly lower priority has a task of R in progress. A task without track counts as a `normal` track for this. "In progress" means `in_progress` only: a task in review is finished work waiting for a human and neither makes a track busy nor keeps it from being idle (my call). The row gets an orange marker and one line per affected role, e.g. "engine: nothing running, 3 ready". With the role filter set, only that role is checked.
- Clicking a row shows its tasks (opens the board filtered to that track).
- Read only.

### Board (after mockup C)

- Switch "group by track" (default on; remembered per browser). On: one swimlane per track in track order, columns as today. Off: today's flat board.
- Within each cell, cards are sorted by task priority, then oldest first.
- New filters next to the role filter: track (all / one track / no track) and minimum priority (all / ≥ low … ≥ critical). They apply to board and review queue.

### REST

- `GET /api/tracks` — tracks with counts (as `track_list`). No track writes over REST.
- Tasks in all responses carry `track` and the priority string; `/api/tasks` gains `?track=` and `?priority=` (at least).
- SSE: a `track_changed` event with `{ "id": <name> }` (a rename emits it for old and new name).

## Documentation and rollout

- Specs 01–06, README, quickstart, and the phase plan describe tracks and the scale.
- After merge: reinstall the binary and update the OpenSHR agent docs (`CLAUDE.md`, `docs/PROCESS.md` on `main`) with how to create tracks and assign tasks.

## Calls made after the written review (mine, correctable)

- `in_review` is its own segment in the bar and in the counts of `track list` / `GET /api/tracks`; it does not count as running for the course warning.
- Importing a version-1 snapshot creates no tracks; tasks come in without track.
- Flat board (switch off): cells sorted by track priority, then task priority, then oldest first — the `task next` order.
- "No track" filter: CLI `task list --no-track`; REST `/api/tasks?track=` with an empty value.
- Renaming a track does not change the tasks' `updated_at` (their content is unchanged; the snapshot ignores `updated_at` anyway).

## Left to the builder

- Module layout, SQL details, and test structure.
- Exact dashboard styling within the SPECTRUM tokens, spacing, and the empty states.
- Help texts and error wording, as long as they name the valid values.
