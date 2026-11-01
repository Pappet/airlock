//! SQLite hub store for one project (Spec 01 §3.2).
//!
//! Every mutation runs in a `BEGIN IMMEDIATE` transaction that first resets
//! expired leases, then checks the transition against current state, then
//! applies it with an update conditioned on the expected prior state. Reads
//! present expired leases as already released. Callers pass `now` (unix
//! seconds) so behavior is testable without a clock.

use crate::core::{
    AnswerVersion, EventKind, Inbox, InboxItem, InboxQuestion, InboxTask, InboxTrack, Priority,
    Question, QuestionStatus, Task, TaskState, TaskStatus, Track, check_dependencies,
    check_task_prefix, check_track_name, cmp_task_ids, dependents, task_number,
};
use crate::hub::{Binding, Bindings};
use crate::snapshot::Snapshot;
use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

/// Schema migrations; entry `n` upgrades `user_version` `n` to `n + 1`.
const MIGRATIONS: &[&str] = &[
    SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7,
];

const SCHEMA_V1: &str = "
CREATE TABLE tasks (
    id               TEXT PRIMARY KEY,
    title            TEXT NOT NULL,
    role             TEXT NOT NULL,
    state            TEXT NOT NULL CHECK (state IN ('open', 'in_progress', 'in_review', 'done')),
    priority         INTEGER NOT NULL DEFAULT 0,
    read             TEXT NOT NULL,
    touch            TEXT NOT NULL,
    do               TEXT NOT NULL,
    verify           TEXT NOT NULL,
    notes            TEXT,
    claimed_by       TEXT,
    lease_expires_at INTEGER,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL
) STRICT;

CREATE TABLE task_deps (
    task_id    TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    blocked_by TEXT NOT NULL REFERENCES tasks (id),
    PRIMARY KEY (task_id, blocked_by)
) STRICT;

CREATE TABLE questions (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    topic       TEXT NOT NULL,
    asked_by    TEXT NOT NULL,
    question    TEXT NOT NULL,
    status      TEXT NOT NULL CHECK (status IN ('open', 'answered', 'resolved')),
    answer      TEXT,
    answered_by TEXT,
    spec_file   TEXT,
    created_at  INTEGER NOT NULL,
    answered_at INTEGER
) STRICT;
";

/// Hashes of every snapshot this hub wrote or imported (Spec 05 §3.5).
const SCHEMA_V2: &str = "
CREATE TABLE exports (
    sha256      TEXT PRIMARY KEY,
    recorded_at INTEGER NOT NULL
) STRICT;
";

/// Machine-local worktree bindings (Spec 06 §2), keyed by canonical root path.
const SCHEMA_V3: &str = "
CREATE TABLE bindings (
    root    TEXT PRIMARY KEY,
    role    TEXT NOT NULL,
    session TEXT NOT NULL,
    locked  INTEGER NOT NULL DEFAULT 0
) STRICT;
";

/// Priority becomes a four-level scale stored as a rank 0 to 3 (the old
/// integers map by threshold, never to critical), and tasks may belong to a
/// track. Triggers refuse any other priority on tasks, so an airlock process
/// from before the migration, still running, cannot write integers the new
/// binary could not read back.
const SCHEMA_V4: &str = "
UPDATE tasks SET priority = CASE WHEN priority >= 30 THEN 2 WHEN priority >= 10 THEN 1 ELSE 0 END;
CREATE TABLE tracks (
    name        TEXT PRIMARY KEY,
    priority    INTEGER NOT NULL DEFAULT 1 CHECK (priority BETWEEN 0 AND 3),
    description TEXT NOT NULL DEFAULT '',
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
) STRICT;
ALTER TABLE tasks ADD COLUMN track TEXT REFERENCES tracks (name) ON UPDATE CASCADE;
CREATE TRIGGER tasks_priority_rank_insert BEFORE INSERT ON tasks
WHEN NEW.priority NOT BETWEEN 0 AND 3
BEGIN
    SELECT RAISE(ABORT, 'priority must be a rank 0 to 3; restart old airlock processes');
END;
CREATE TRIGGER tasks_priority_rank_update BEFORE UPDATE OF priority ON tasks
WHEN NEW.priority NOT BETWEEN 0 AND 3
BEGIN
    SELECT RAISE(ABORT, 'priority must be a rank 0 to 3; restart old airlock processes');
END;
";

/// Answer corrections: the replaced answers are kept per question, oldest
/// first, and the question records who corrected it last and when. A trigger
/// refuses to replace an answer unless the newest history row holds it, so an
/// airlock process from before the migration, still running, cannot overwrite
/// an answer and lose the earlier text.
const SCHEMA_V5: &str = "
ALTER TABLE questions ADD COLUMN corrected_at INTEGER;
ALTER TABLE questions ADD COLUMN corrected_by TEXT;
CREATE TABLE answer_history (
    question_id INTEGER NOT NULL REFERENCES questions (id),
    seq         INTEGER NOT NULL,
    answer      TEXT NOT NULL,
    answered_by TEXT,
    answered_at INTEGER,
    spec_file   TEXT,
    PRIMARY KEY (question_id, seq)
) STRICT;
CREATE TRIGGER questions_answer_history BEFORE UPDATE OF answer ON questions
WHEN OLD.answer IS NOT NULL
 AND NOT EXISTS (
     SELECT 1 FROM answer_history h
      WHERE h.question_id = OLD.id
        AND h.seq = (SELECT max(seq) FROM answer_history WHERE question_id = OLD.id)
        AND h.answer = OLD.answer
        AND h.answered_by IS OLD.answered_by
        AND h.answered_at IS OLD.answered_at)
BEGIN
    SELECT RAISE(ABORT, 'answer history missing for a replaced answer; restart old airlock processes');
END;
";

/// Progress notes on a lease: the holder's running note for its successor.
/// Session state, never exported; kept across release and lease expiry. A
/// trigger clears it when the task moves to `done` or `in_review`, so the rule
/// holds for every writer, an airlock process from before the migration included.
const SCHEMA_V6: &str = "
ALTER TABLE tasks ADD COLUMN progress TEXT;
CREATE TRIGGER tasks_progress_cleared AFTER UPDATE OF state ON tasks
WHEN NEW.state IN ('done', 'in_review') AND NEW.progress IS NOT NULL
BEGIN
    UPDATE tasks SET progress = NULL WHERE id = NEW.id;
END;
";

/// Events and inbox cursors (Spec 01 §2.6): an append-only feed of what
/// happened, written in the transaction of the change, and per session and
/// role how far it was read. Session state: never exported, pruned after
/// 30 days. `role` is the role an event is for, or `*` for every coordinating
/// (non-clean-room) role except `origin`, the role it came from. Events hold
/// ids, never text.
const SCHEMA_V7: &str = "
CREATE TABLE events (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    at          INTEGER NOT NULL,
    kind        TEXT NOT NULL,
    role        TEXT NOT NULL,
    origin      TEXT,
    task_id     TEXT,
    question_id INTEGER,
    track       TEXT
) STRICT;
CREATE INDEX events_role_seq ON events (role, seq);
CREATE INDEX events_at ON events (at);
CREATE TABLE inbox_cursors (
    session    TEXT NOT NULL,
    role       TEXT NOT NULL,
    seq        INTEGER NOT NULL,
    nudged_seq INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (session, role)
) STRICT;
";

/// Events older than this are pruned on write.
const EVENT_RETENTION_SECS: i64 = 30 * 24 * 60 * 60;

/// `events.role` of an event for every coordinating role but its origin.
const COORDINATORS: &str = "*";

const SELECT_TASKS: &str = "
SELECT id, title, role, state, priority,
       (SELECT json_group_array(blocked_by)
          FROM (SELECT blocked_by FROM task_deps WHERE task_id = t.id ORDER BY rowid)),
       read, touch, do, verify, notes, claimed_by, lease_expires_at, created_at, updated_at, track,
       progress
  FROM tasks t";

const SELECT_TRACKS: &str =
    "SELECT name, priority, description, created_at, updated_at FROM tracks";

const SELECT_QUESTIONS: &str = "
SELECT id, topic, asked_by, question, status, answer, answered_by, spec_file, created_at, answered_at,
       corrected_at, corrected_by
  FROM questions";

/// Fields of a task to be created. The store assigns the id
/// `<prefix>-<n>`, continuing after the highest number used with `prefix`.
#[derive(Debug, Clone)]
pub struct NewTask {
    /// Id prefix, normally the role's (`ProjectConfig::task_prefix`)
    pub prefix: String,
    pub title: String,
    pub role: String,
    pub priority: Priority,
    /// Must name a defined track
    pub track: Option<String>,
    pub blocked_by: Vec<String>,
    pub read: Vec<String>,
    pub touch: Vec<String>,
    pub r#do: String,
    pub verify: String,
}

/// Changes to an existing task; `None` keeps the field, a list replaces it.
/// `track` is doubly optional: `Some(None)` clears it.
/// A new content field must also go into `cli::TaskEditSpec`, the MCP
/// `task_update` tool, the REST `EditTask` body, and Specs 02–04.
#[derive(Debug, Clone, Default)]
pub struct TaskEdit {
    pub title: Option<String>,
    pub role: Option<String>,
    pub priority: Option<Priority>,
    pub track: Option<Option<String>>,
    pub blocked_by: Option<Vec<String>>,
    pub read: Option<Vec<String>>,
    pub touch: Option<Vec<String>>,
    pub r#do: Option<String>,
    pub verify: Option<String>,
}

impl TaskEdit {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.role.is_none()
            && self.priority.is_none()
            && self.track.is_none()
            && self.blocked_by.is_none()
            && self.read.is_none()
            && self.touch.is_none()
            && self.r#do.is_none()
            && self.verify.is_none()
    }
}

/// Changes to an existing track; `None` keeps the field.
#[derive(Debug, Clone, Default)]
pub struct TrackEdit {
    pub name: Option<String>,
    pub priority: Option<Priority>,
    pub description: Option<String>,
}

impl TrackEdit {
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.priority.is_none() && self.description.is_none()
    }
}

/// Result of finishing a task.
#[derive(Debug, Clone)]
pub struct Completion {
    pub task: Task,
    /// Dependents that became ready because this task is now done
    pub now_ready: Vec<String>,
}

/// Error for an unknown task or question id; callers can tell it apart from a
/// refused transition with `anyhow::Error::downcast_ref`.
#[derive(Debug)]
pub struct NotFound(pub String);

impl std::fmt::Display for NotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NotFound {}

/// An open connection to one project's store.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens (and if needed creates) the store at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        let mut conn =
            Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let mode = enable_wal(&conn)?;
        ensure!(
            mode.eq_ignore_ascii_case("wal"),
            "{} does not support WAL mode",
            path.display()
        );
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;

        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        let version = usize::try_from(version).unwrap_or(usize::MAX);
        ensure!(
            version <= MIGRATIONS.len(),
            "{} has schema version {version}; this airlock supports up to {}",
            path.display(),
            MIGRATIONS.len()
        );
        for migration in &MIGRATIONS[version..] {
            tx.execute_batch(migration)?;
        }
        tx.pragma_update(None, "user_version", MIGRATIONS.len() as i64)?;
        tx.commit()?;
        Ok(Self { conn })
    }

    /// SQLite's `PRAGMA data_version`: changes whenever another connection
    /// commits to the store.
    pub fn data_version(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("PRAGMA data_version", [], |row| row.get(0))?)
    }

    /// All tasks, sorted by id, with expired leases shown as released.
    pub fn tasks(&self, now: i64) -> Result<Vec<Task>> {
        Ok(load_tasks(&self.conn)?
            .into_iter()
            .map(|t| released_if_expired(t, now))
            .collect())
    }

    pub fn task(&self, id: &str, now: i64) -> Result<Task> {
        load_task(&self.conn, id).map(|t| released_if_expired(t, now))
    }

    /// All questions, newest first.
    pub fn questions(&self) -> Result<Vec<Question>> {
        let mut stmt = self
            .conn
            .prepare(&format!("{SELECT_QUESTIONS} ORDER BY id DESC"))?;
        let rows = stmt.query_map([], question_from_row)?;
        let mut questions: Vec<Question> = rows.collect::<rusqlite::Result<_>>()?;
        // One query for every history instead of one per question.
        let mut history = load_all_history(&self.conn)?;
        for question in &mut questions {
            question.history = history.remove(&question.id).unwrap_or_default();
        }
        Ok(questions)
    }

    pub fn question(&self, id: u64) -> Result<Question> {
        load_question(&self.conn, id)
    }

    /// Fails with the defined track names unless `name` is a track.
    pub fn ensure_track(&self, name: &str) -> Result<()> {
        ensure_track(&self.conn, name)
    }

    /// All tracks, highest priority first, then by name.
    pub fn tracks(&self) -> Result<Vec<Track>> {
        let mut stmt = self
            .conn
            .prepare(&format!("{SELECT_TRACKS} ORDER BY priority DESC, name"))?;
        let rows = stmt.query_map([], track_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn track(&self, name: &str) -> Result<Track> {
        load_track(&self.conn, name)
    }

    pub fn add_track(
        &mut self,
        name: &str,
        priority: Priority,
        description: &str,
        now: i64,
    ) -> Result<Track> {
        check_track_name(name).map_err(anyhow::Error::msg)?;
        let tx = self.write(now)?;
        ensure!(!track_exists(&tx, name)?, "track `{name}` already exists");
        tx.execute(
            "INSERT INTO tracks (name, priority, description, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            params![name, priority.rank(), description, now],
        )?;
        let track = load_track(&tx, name)?;
        tx.commit()?;
        Ok(track)
    }

    /// Renames, reprioritizes, or redescribes a track. A rename moves the
    /// track's tasks along (`ON UPDATE CASCADE`) without touching their
    /// `updated_at`.
    pub fn edit_track(&mut self, name: &str, edit: TrackEdit, now: i64) -> Result<Track> {
        ensure!(!edit.is_empty(), "nothing to change for track `{name}`");
        if let Some(new_name) = &edit.name {
            check_track_name(new_name).map_err(anyhow::Error::msg)?;
        }
        let tx = self.write(now)?;
        let mut track = load_track(&tx, name)?;
        if let Some(new_name) = edit.name.filter(|n| n != name) {
            ensure!(
                !track_exists(&tx, &new_name)?,
                "track `{new_name}` already exists"
            );
            track.name = new_name;
        }
        let reprioritized = edit.priority.is_some_and(|p| p != track.priority);
        if let Some(priority) = edit.priority {
            track.priority = priority;
        }
        if let Some(description) = edit.description {
            track.description = description;
        }
        tx.execute(
            "UPDATE tracks SET name = ?1, priority = ?2, description = ?3, updated_at = ?4 WHERE name = ?5",
            params![track.name, track.priority.rank(), track.description, now, name],
        )?;
        if reprioritized {
            let mut stmt = tx.prepare(
                "SELECT DISTINCT role FROM tasks WHERE track = ?1 AND state = 'open' ORDER BY role",
            )?;
            let roles: Vec<String> = stmt
                .query_map([&track.name], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            drop(stmt);
            for role in roles {
                emit(
                    &tx,
                    now,
                    EventKind::TrackChanged,
                    &role,
                    None,
                    None,
                    None,
                    Some(&track.name),
                )?;
            }
        }
        let track = load_track(&tx, &track.name)?;
        tx.commit()?;
        Ok(track)
    }

    /// Removes a track no task belongs to.
    pub fn remove_track(&mut self, name: &str, now: i64) -> Result<()> {
        let tx = self.write(now)?;
        load_track(&tx, name)?;
        let used: i64 = tx.query_row(
            "SELECT count(*) FROM tasks WHERE track = ?1",
            [name],
            |row| row.get(0),
        )?;
        ensure!(used == 0, "track `{name}` is used by {used} task(s)");
        tx.execute("DELETE FROM tracks WHERE name = ?1", [name])?;
        tx.commit()?;
        Ok(())
    }

    pub fn add_task(&mut self, new: NewTask, now: i64) -> Result<Task> {
        check_task_prefix(&new.prefix).map_err(anyhow::Error::msg)?;
        let tx = self.write(now)?;
        if let Some(track) = &new.track {
            ensure_track(&tx, track)?;
        }
        // Numbered inside the write transaction, so concurrent adds never collide.
        let id = format!("{}-{}", new.prefix, next_task_number(&tx, &new.prefix)?);
        let mut seen = HashSet::new();
        let blocked_by: Vec<String> = new
            .blocked_by
            .into_iter()
            .filter(|b| seen.insert(b.clone()))
            .collect();
        for dep in &blocked_by {
            ensure!(
                task_exists(&tx, dep)?,
                "prerequisite `{dep}` does not exist; add it first"
            );
        }

        tx.execute(
            "INSERT INTO tasks (id, title, role, state, priority, track, read, touch, do, verify, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'open', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
            params![
                id,
                new.title,
                new.role,
                new.priority.rank(),
                new.track,
                serde_json::to_string(&new.read)?,
                serde_json::to_string(&new.touch)?,
                new.r#do,
                new.verify,
                now
            ],
        )?;
        for dep in &blocked_by {
            tx.execute(
                "INSERT INTO task_deps (task_id, blocked_by) VALUES (?1, ?2)",
                params![id, dep],
            )?;
        }
        emit_if_ready(&tx, &id, EventKind::TaskReady, now)?;
        let task = load_task(&tx, &id)?;
        tx.commit()?;
        Ok(task)
    }

    /// Changes the content of a task. Tasks past `open` are only changed with
    /// `force`, since a session may be working from the old content; state,
    /// claim, lease, and notes are never touched.
    pub fn edit_task(&mut self, id: &str, edit: TaskEdit, force: bool, now: i64) -> Result<Task> {
        ensure!(!edit.is_empty(), "nothing to change for task `{id}`");
        let tx = self.write(now)?;
        let mut tasks = load_tasks(&tx)?;
        let pos = tasks
            .iter()
            .position(|t| t.id == id)
            .ok_or_else(|| NotFound(format!("no task `{id}`")))?;
        let state = tasks[pos].state;
        ensure!(
            force || state == TaskState::Open,
            "task `{id}` is {}; use --force to edit it anyway",
            state.as_str()
        );
        let before = tasks[pos].clone();
        let was_ready = status_of(&tasks, &before) == TaskStatus::Ready;

        let task = &mut tasks[pos];
        if let Some(blocked_by) = edit.blocked_by {
            let mut seen = HashSet::new();
            task.blocked_by = blocked_by
                .into_iter()
                .filter(|b| seen.insert(b.clone()))
                .collect();
        }
        if let Some(title) = edit.title {
            task.title = title;
        }
        if let Some(role) = edit.role {
            task.role = role;
        }
        if let Some(priority) = edit.priority {
            task.priority = priority;
        }
        if let Some(track) = edit.track {
            if let Some(name) = &track {
                ensure_track(&tx, name)?;
            }
            task.track = track;
        }
        if let Some(read) = edit.read {
            task.read = read;
        }
        if let Some(touch) = edit.touch {
            task.touch = touch;
        }
        if let Some(r#do) = edit.r#do {
            task.r#do = r#do;
        }
        if let Some(verify) = edit.verify {
            task.verify = verify;
        }
        check_dependencies(
            tasks
                .iter()
                .map(|t| (t.id.as_str(), t.blocked_by.as_slice())),
        )?;

        let task = &tasks[pos];
        let changed = tx.execute(
            "UPDATE tasks SET title = ?1, role = ?2, priority = ?3, track = ?4, read = ?5, touch = ?6, do = ?7, verify = ?8, updated_at = ?9
              WHERE id = ?10 AND state = ?11",
            params![
                task.title,
                task.role,
                task.priority.rank(),
                task.track,
                serde_json::to_string(&task.read)?,
                serde_json::to_string(&task.touch)?,
                task.r#do,
                task.verify,
                now,
                id,
                state.as_str()
            ],
        )?;
        ensure!(changed == 1, "task `{id}` changed concurrently; retry");
        tx.execute("DELETE FROM task_deps WHERE task_id = ?1", [id])?;
        for dep in &task.blocked_by {
            tx.execute(
                "INSERT INTO task_deps (task_id, blocked_by) VALUES (?1, ?2)",
                params![id, dep],
            )?;
        }
        // Only an open, ready task concerns its role: new to it (just unblocked or
        // moved to it) or changed in what decides whether to take it next.
        if state == TaskState::Open && status_of(&tasks, task) == TaskStatus::Ready {
            if !was_ready || before.role != task.role {
                emit(
                    &tx,
                    now,
                    EventKind::TaskReady,
                    &task.role,
                    None,
                    Some(id),
                    None,
                    None,
                )?;
            } else if before.priority != task.priority
                || before.track != task.track
                || before.title != task.title
                || before.read != task.read
                || before.touch != task.touch
                || before.r#do != task.r#do
                || before.verify != task.verify
            {
                emit(
                    &tx,
                    now,
                    EventKind::TaskChanged,
                    &task.role,
                    None,
                    Some(id),
                    None,
                    None,
                )?;
            }
        }
        commit_task(tx, id)
    }

    pub fn claim(&mut self, id: &str, session: &str, lease_secs: i64, now: i64) -> Result<Task> {
        let tx = self.write(now)?;
        let tasks = load_tasks(&tx)?;
        let task = find(&tasks, id)?;
        match status_of(&tasks, task) {
            TaskStatus::Ready => {}
            TaskStatus::Blocked => bail!("task `{id}` is blocked by unfinished prerequisites"),
            TaskStatus::InProgress if task.claimed_by.as_deref() == Some(session) => {
                bail!(
                    "task `{id}` is already claimed by this session; use `task heartbeat` to extend the lease"
                )
            }
            TaskStatus::InProgress => {
                bail!("task `{id}` is claimed by {}", lease_holder(task, now))
            }
            status => bail!("task `{id}` is {} and cannot be claimed", status.as_str()),
        }
        set_state(
            &tx,
            id,
            TaskState::Open,
            TaskState::InProgress,
            Some(session),
            Some(now + lease_secs),
            None,
            now,
        )?;
        commit_task(tx, id)
    }

    /// Extends the lease; `progress`, when given, replaces the running note for the
    /// successor (`None` keeps the existing one, an empty note removes it).
    pub fn heartbeat(
        &mut self,
        id: &str,
        session: &str,
        lease_secs: i64,
        progress: Option<&str>,
        now: i64,
    ) -> Result<Task> {
        let tx = self.write(now)?;
        let task = load_task(&tx, id)?;
        ensure_holder(&task, session, now)?;
        let changed = tx.execute(
            "UPDATE tasks SET lease_expires_at = ?1, updated_at = ?2, progress = CASE WHEN ?5 IS NULL THEN progress ELSE nullif(?5, '') END
              WHERE id = ?3 AND state = 'in_progress' AND claimed_by = ?4",
            params![now + lease_secs, now, id, session, progress],
        )?;
        ensure!(changed == 1, "task `{id}` changed concurrently; retry");
        commit_task(tx, id)
    }

    pub fn release(&mut self, id: &str, session: &str, force: bool, now: i64) -> Result<Task> {
        let tx = self.write(now)?;
        let task = load_task(&tx, id)?;
        if !force {
            ensure_holder(&task, session, now)?;
        }
        ensure!(
            task.state == TaskState::InProgress,
            "task `{id}` is not claimed"
        );
        set_state(
            &tx,
            id,
            TaskState::InProgress,
            TaskState::Open,
            None,
            None,
            None,
            now,
        )?;
        emit_if_ready(&tx, id, EventKind::TaskReady, now)?;
        commit_task(tx, id)
    }

    pub fn complete(
        &mut self,
        id: &str,
        session: &str,
        notes: Option<&str>,
        review: bool,
        force: bool,
        now: i64,
    ) -> Result<Completion> {
        let tx = self.write(now)?;
        let tasks = load_tasks(&tx)?;
        let task = find(&tasks, id)?;
        let from = match status_of(&tasks, task) {
            TaskStatus::InProgress if force || task.claimed_by.as_deref() == Some(session) => {
                TaskState::InProgress
            }
            TaskStatus::InProgress => bail!(
                "task `{id}` is claimed by {}; use --force to finish it anyway",
                lease_holder(task, now)
            ),
            TaskStatus::Ready if force => TaskState::Open,
            TaskStatus::Ready => bail!("task `{id}` is not claimed; claim it first or use --force"),
            TaskStatus::Blocked => bail!("task `{id}` is blocked by unfinished prerequisites"),
            status => bail!("task `{id}` is already {}", status.as_str()),
        };
        let to = if review {
            TaskState::InReview
        } else {
            TaskState::Done
        };
        set_state(&tx, id, from, to, None, None, notes, now)?;
        let now_ready = if to == TaskState::Done {
            newly_ready(&tx, id)?
        } else {
            let role = &task.role;
            emit(
                &tx,
                now,
                EventKind::TaskInReview,
                COORDINATORS,
                Some(role),
                Some(id),
                None,
                None,
            )?;
            Vec::new()
        };
        for ready in &now_ready {
            emit_if_ready(&tx, ready, EventKind::TaskReady, now)?;
        }
        let task = load_task(&tx, id)?;
        tx.commit()?;
        Ok(Completion { task, now_ready })
    }

    pub fn approve(&mut self, id: &str, notes: Option<&str>, now: i64) -> Result<Completion> {
        let tx = self.write(now)?;
        let task = load_task(&tx, id)?;
        ensure!(
            task.state == TaskState::InReview,
            "task `{id}` is not in review"
        );
        set_state(
            &tx,
            id,
            TaskState::InReview,
            TaskState::Done,
            None,
            None,
            notes,
            now,
        )?;
        let now_ready = newly_ready(&tx, id)?;
        for ready in &now_ready {
            emit_if_ready(&tx, ready, EventKind::TaskReady, now)?;
        }
        let task = load_task(&tx, id)?;
        tx.commit()?;
        Ok(Completion { task, now_ready })
    }

    pub fn reject(&mut self, id: &str, notes: &str, now: i64) -> Result<Task> {
        let tx = self.write(now)?;
        let task = load_task(&tx, id)?;
        ensure!(
            task.state == TaskState::InReview,
            "task `{id}` is not in review"
        );
        set_state(
            &tx,
            id,
            TaskState::InReview,
            TaskState::Open,
            None,
            None,
            Some(notes),
            now,
        )?;
        // Ready again first, so the rejection is the task's latest event.
        emit_if_ready(&tx, id, EventKind::TaskReady, now)?;
        emit(
            &tx,
            now,
            EventKind::TaskRejected,
            &task.role,
            None,
            Some(id),
            None,
            None,
        )?;
        commit_task(tx, id)
    }

    /// Records a question from `session`; `role` is the asker's role, if known,
    /// so the answer reaches it (Spec 01 §2.6).
    pub fn ask(
        &mut self,
        topic: &str,
        question: &str,
        session: &str,
        role: Option<&str>,
        now: i64,
    ) -> Result<Question> {
        let tx = self.write(now)?;
        tx.execute(
            "INSERT INTO questions (topic, asked_by, question, status, created_at) VALUES (?1, ?2, ?3, 'open', ?4)",
            params![topic, session, question, now],
        )?;
        let id = u64::try_from(tx.last_insert_rowid())?;
        emit(
            &tx,
            now,
            EventKind::QuestionAsked,
            COORDINATORS,
            role,
            None,
            Some(id),
            None,
        )?;
        let question = load_question(&tx, id)?;
        tx.commit()?;
        Ok(question)
    }

    /// Asks unless `session` already has an open question on `topic`, in which
    /// case that question is returned unchanged.
    pub fn ask_once(
        &mut self,
        topic: &str,
        question: &str,
        session: &str,
        role: Option<&str>,
        now: i64,
    ) -> Result<Question> {
        let tx = self.write(now)?;
        let open: Option<i64> = tx
            .query_row(
                "SELECT id FROM questions WHERE topic = ?1 AND asked_by = ?2 AND status = 'open'
                  ORDER BY id LIMIT 1",
                params![topic, session],
                |row| row.get(0),
            )
            .optional()?;
        let id = match open {
            Some(id) => u64::try_from(id)?,
            None => {
                tx.execute(
                    "INSERT INTO questions (topic, asked_by, question, status, created_at) VALUES (?1, ?2, ?3, 'open', ?4)",
                    params![topic, session, question, now],
                )?;
                let id = u64::try_from(tx.last_insert_rowid())?;
                emit(
                    &tx,
                    now,
                    EventKind::QuestionAsked,
                    COORDINATORS,
                    role,
                    None,
                    Some(id),
                    None,
                )?;
                id
            }
        };
        let question = load_question(&tx, id)?;
        tx.commit()?;
        Ok(question)
    }

    /// Records the answer. On a question that already has one (answered or
    /// resolved) this is a correction: the current answer moves to the
    /// history, the status stays, and `corrected_at`/`corrected_by` are set.
    pub fn answer(
        &mut self,
        id: u64,
        answer: &str,
        session: &str,
        spec_file: Option<&str>,
        now: i64,
    ) -> Result<Question> {
        let tx = self.write(now)?;
        let question = load_question(&tx, id)?;
        if let Some(previous) = &question.answer {
            tx.execute(
                "INSERT INTO answer_history (question_id, seq, answer, answered_by, answered_at, spec_file)
                 VALUES (?1, (SELECT coalesce(max(seq), 0) + 1 FROM answer_history WHERE question_id = ?1),
                         ?2, ?3, ?4, ?5)",
                params![
                    sql_id(id)?,
                    previous,
                    question.answered_by,
                    question.answered_at,
                    question.spec_file
                ],
            )?;
            tx.execute(
                "UPDATE questions
                    SET answer = ?1, answered_by = ?2, answered_at = ?3,
                        spec_file = coalesce(?4, spec_file),
                        corrected_at = ?3, corrected_by = ?2
                  WHERE id = ?5",
                params![answer, session, now, spec_file, sql_id(id)?],
            )?;
        } else {
            tx.execute(
                "UPDATE questions
                    SET status = 'answered', answer = ?1, answered_by = ?2, answered_at = ?3,
                        spec_file = coalesce(?4, spec_file)
                  WHERE id = ?5",
                params![answer, session, now, spec_file, sql_id(id)?],
            )?;
        }
        // An asker whose role is unknown saw nothing addressed to it; the
        // coordinating roles, who were told about the question, get the answer.
        let role = asker_role(&tx, id, &question.asked_by)?;
        let role = role.as_deref().unwrap_or(COORDINATORS);
        emit(
            &tx,
            now,
            EventKind::QuestionAnswered,
            role,
            None,
            None,
            Some(id),
            None,
        )?;
        let question = load_question(&tx, id)?;
        tx.commit()?;
        Ok(question)
    }

    pub fn resolve(&mut self, id: u64, spec_file: Option<&str>, now: i64) -> Result<Question> {
        let tx = self.write(now)?;
        let question = load_question(&tx, id)?;
        match question.status {
            QuestionStatus::Answered => {}
            QuestionStatus::Open => bail!("question #{id} has no answer yet"),
            QuestionStatus::Resolved => bail!("question #{id} is already resolved"),
        }
        tx.execute(
            "UPDATE questions SET status = 'resolved', spec_file = coalesce(?1, spec_file) WHERE id = ?2",
            params![spec_file, sql_id(id)?],
        )?;
        let question = load_question(&tx, id)?;
        tx.commit()?;
        Ok(question)
    }

    /// Starts `session`'s inbox for `role` at the newest event unless it has
    /// one already, so news from now on is not lost (Spec 01 §2.6). The
    /// briefing and a claim call it; reading the inbox does it too.
    pub fn open_inbox(&mut self, session: &str, role: &str, now: i64) -> Result<()> {
        let tx = self.write(now)?;
        tx.execute(
            "INSERT OR IGNORE INTO inbox_cursors (session, role, seq)
             VALUES (?1, ?2, (SELECT coalesce(max(seq), 0) FROM events))",
            params![session, role],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// What changed for `role` since `session` last looked (Spec 01 §2.6).
    /// `coordinating` is true for roles that are not clean-room: they also see
    /// new questions and tasks in review from other roles. Reading advances the
    /// session's cursor unless `peek`; a session without a cursor starts at the
    /// newest event and gets an empty inbox.
    pub fn inbox(
        &mut self,
        session: &str,
        role: &str,
        coordinating: bool,
        peek: bool,
        now: i64,
    ) -> Result<Inbox> {
        let tx = self.write(now)?;
        let cursor: Option<i64> = tx
            .query_row(
                "SELECT seq FROM inbox_cursors WHERE session = ?1 AND role = ?2",
                params![session, role],
                |row| row.get(0),
            )
            .optional()?;
        let Some(cursor) = cursor else {
            tx.execute(
                "INSERT INTO inbox_cursors (session, role, seq)
                 VALUES (?1, ?2, (SELECT coalesce(max(seq), 0) FROM events))",
                params![session, role],
            )?;
            tx.commit()?;
            return Ok(Inbox::default());
        };

        let mut stmt = tx.prepare(
            "SELECT seq, at, kind, task_id, question_id, track FROM events
              WHERE seq > ?1
                AND (role = ?2 OR (?3 AND role = ?4 AND (origin IS NULL OR origin <> ?2)))
              ORDER BY seq",
        )?;
        type Row6 = (
            i64,
            i64,
            String,
            Option<String>,
            Option<i64>,
            Option<String>,
        );
        let rows: Vec<Row6> = stmt
            .query_map(params![cursor, role, coordinating, COORDINATORS], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        let last = rows.last().map_or(cursor, |r| r.0);

        // One item per subject: its latest event.
        let mut latest: HashMap<(u8, String), usize> = HashMap::new();
        for (i, (_, _, _, task_id, question_id, track)) in rows.iter().enumerate() {
            let key = match (task_id, question_id, track) {
                (Some(t), _, _) => (0, t.clone()),
                (_, Some(q), _) => (1, q.to_string()),
                (_, _, Some(t)) => (2, t.clone()),
                _ => continue,
            };
            latest.insert(key, i);
        }
        let mut picked: Vec<usize> = latest.into_values().collect();
        picked.sort_unstable();

        let tasks = load_tasks(&tx)?;
        let mut items = Vec::new();
        for i in picked {
            let (_, at, kind, task_id, question_id, track) = &rows[i];
            let Some(kind) = EventKind::parse(kind) else {
                continue;
            };
            let item = match kind {
                EventKind::TaskReady
                | EventKind::TaskChanged
                | EventKind::TaskRejected
                | EventKind::TaskInReview => {
                    let Some(task) = task_id
                        .as_deref()
                        .and_then(|id| tasks.iter().find(|t| t.id == id))
                    else {
                        continue;
                    };
                    let status = status_of(&tasks, task);
                    let current = match kind {
                        EventKind::TaskInReview => status == TaskStatus::InReview,
                        EventKind::TaskRejected => task.role == role,
                        _ => task.role == role && status == TaskStatus::Ready,
                    };
                    if !current {
                        continue;
                    }
                    InboxItem {
                        kind,
                        at: *at,
                        task: Some(InboxTask {
                            id: task.id.clone(),
                            title: task.title.clone(),
                            priority: task.priority,
                            track: task.track.clone(),
                            status,
                        }),
                        question: None,
                        track: None,
                    }
                }
                EventKind::QuestionAsked | EventKind::QuestionAnswered => {
                    let Some(id) = question_id.and_then(|q| u64::try_from(q).ok()) else {
                        continue;
                    };
                    let Some(q) = load_question(&tx, id).ok() else {
                        continue;
                    };
                    let asked = kind == EventKind::QuestionAsked;
                    if asked && q.status != QuestionStatus::Open {
                        continue;
                    }
                    InboxItem {
                        kind,
                        at: *at,
                        task: None,
                        question: Some(InboxQuestion {
                            id: q.id,
                            topic: q.topic,
                            question: asked.then_some(q.question),
                            answer: if asked { None } else { q.answer },
                            spec_file: if asked { None } else { q.spec_file },
                            corrected: !asked && q.corrected_at.is_some(),
                        }),
                        track: None,
                    }
                }
                EventKind::TrackChanged => {
                    let Some(t) = track.as_deref().and_then(|name| load_track(&tx, name).ok())
                    else {
                        continue;
                    };
                    InboxItem {
                        kind,
                        at: *at,
                        task: None,
                        question: None,
                        track: Some(InboxTrack {
                            name: t.name,
                            priority: t.priority,
                        }),
                    }
                }
            };
            items.push(item);
        }

        if !peek && last > cursor {
            tx.execute(
                "UPDATE inbox_cursors SET seq = ?1 WHERE session = ?2 AND role = ?3",
                params![last, session, role],
            )?;
        }
        tx.commit()?;
        Ok(Inbox { items })
    }

    /// Whether this hub wrote or imported a snapshot with this SHA-256 (hex).
    pub fn is_known_export(&self, sha256: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row("SELECT 1 FROM exports WHERE sha256 = ?1", [sha256], |_| {
                Ok(())
            })
            .optional()?
            .is_some())
    }

    pub fn record_export(&mut self, sha256: &str, now: i64) -> Result<()> {
        let tx = self.write(now)?;
        tx.execute(
            "INSERT OR IGNORE INTO exports (sha256, recorded_at) VALUES (?1, ?2)",
            params![sha256, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Replaces all tasks, tracks, and questions with the snapshot's content and records
    /// the snapshot's hash (`sha256`) as known, in one transaction. The caller
    /// validates the snapshot first. Question ids are kept; new questions
    /// continue after the highest id this store ever used.
    pub fn replace_all(&mut self, snapshot: &Snapshot, sha256: &str, now: i64) -> Result<()> {
        let tx = self.write(now)?;
        tx.execute(
            "INSERT OR IGNORE INTO exports (sha256, recorded_at) VALUES (?1, ?2)",
            params![sha256, now],
        )?;
        // Tasks reference tracks, so tasks go first and come back last.
        tx.execute_batch(
            "DELETE FROM task_deps; DELETE FROM tasks; DELETE FROM tracks;
             DELETE FROM answer_history; DELETE FROM questions;",
        )?;
        for track in &snapshot.tracks {
            tx.execute(
                "INSERT INTO tracks (name, priority, description, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    track.name,
                    track.priority.rank(),
                    track.description,
                    track.created_at,
                    now
                ],
            )?;
        }
        for t in &snapshot.tasks {
            let state = match t.state {
                TaskState::InProgress => TaskState::Open,
                state => state,
            };
            tx.execute(
                "INSERT INTO tasks (id, title, role, state, priority, track, read, touch, do, verify, notes, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    t.id,
                    t.title,
                    t.role,
                    state.as_str(),
                    t.priority.rank(),
                    t.track,
                    serde_json::to_string(&t.read)?,
                    serde_json::to_string(&t.touch)?,
                    t.r#do,
                    t.verify,
                    t.notes,
                    t.created_at,
                    now
                ],
            )?;
        }
        for t in &snapshot.tasks {
            for dep in &t.blocked_by {
                tx.execute(
                    "INSERT INTO task_deps (task_id, blocked_by) VALUES (?1, ?2)",
                    params![t.id, dep],
                )?;
            }
        }
        for q in &snapshot.questions {
            tx.execute(
                "INSERT INTO questions (id, topic, asked_by, question, status, answer, answered_by, spec_file, created_at, answered_at, corrected_at, corrected_by)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    sql_id(q.id)?,
                    q.topic,
                    q.asked_by,
                    q.question,
                    q.status.as_str(),
                    q.answer,
                    q.answered_by,
                    q.spec_file,
                    q.created_at,
                    q.answered_at,
                    q.corrected_at,
                    q.corrected_by
                ],
            )?;
            for (seq, version) in (1_i64..).zip(&q.history) {
                tx.execute(
                    "INSERT INTO answer_history (question_id, seq, answer, answered_by, answered_at, spec_file)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        sql_id(q.id)?,
                        seq,
                        version.answer,
                        version.answered_by,
                        version.answered_at,
                        version.spec_file
                    ],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// All worktree bindings, keyed by canonical root path.
    pub fn bindings(&self) -> Result<Bindings> {
        let mut stmt = self
            .conn
            .prepare("SELECT root, role, session, locked FROM bindings ORDER BY root")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get(0)?,
                Binding {
                    role: row.get(1)?,
                    session: row.get(2)?,
                    locked: row.get(3)?,
                },
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn binding(&self, root: &str) -> Result<Option<Binding>> {
        Ok(self
            .conn
            .query_row(
                "SELECT role, session, locked FROM bindings WHERE root = ?1",
                [root],
                |row| {
                    Ok(Binding {
                        role: row.get(0)?,
                        session: row.get(1)?,
                        locked: row.get(2)?,
                    })
                },
            )
            .optional()?)
    }

    /// Binds `root`; a locked binding is only replaced with `force`. Check and
    /// write share one transaction, so concurrent `use` calls cannot lose updates.
    pub fn bind(&mut self, root: &str, binding: &Binding, force: bool, now: i64) -> Result<()> {
        let tx = self.write(now)?;
        ensure_unlocked(&tx, root, force)?;
        tx.execute(
            "INSERT INTO bindings (root, role, session, locked) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (root) DO UPDATE SET role = ?2, session = ?3, locked = ?4",
            params![root, binding.role, binding.session, binding.locked],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Removes the binding of `root`; returns whether there was one.
    pub fn unbind(&mut self, root: &str, force: bool, now: i64) -> Result<bool> {
        let tx = self.write(now)?;
        ensure_unlocked(&tx, root, force)?;
        let removed = tx.execute("DELETE FROM bindings WHERE root = ?1", [root])? > 0;
        tx.commit()?;
        Ok(removed)
    }

    /// Starts a write transaction and releases expired leases.
    fn write(&mut self, now: i64) -> Result<Transaction<'_>> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE tasks SET state = 'open', claimed_by = NULL, lease_expires_at = NULL, updated_at = ?1
              WHERE state = 'in_progress' AND lease_expires_at <= ?1",
            params![now],
        )?;
        tx.execute(
            "DELETE FROM events WHERE at < ?1",
            [now - EVENT_RETENTION_SECS],
        )?;
        Ok(tx)
    }
}

/// One past the highest `<prefix>-<n>` among all tasks, done ones included.
fn next_task_number(conn: &Connection, prefix: &str) -> Result<u64> {
    let mut stmt = conn.prepare("SELECT id FROM tasks")?;
    let ids = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut highest = 0;
    for id in ids {
        if let Some(n) = task_number(&id?, prefix) {
            highest = highest.max(n);
        }
    }
    Ok(highest + 1)
}

/// Switches to WAL. On a fresh file SQLite answers a concurrent switch with
/// SQLITE_BUSY without consulting the busy timeout, so retry for a while.
pub(crate) fn enable_wal(conn: &Connection) -> Result<String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0)) {
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::DatabaseBusy && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            other => return Ok(other?),
        }
    }
}

fn task_exists(conn: &Connection, id: &str) -> Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM tasks WHERE id = ?1", [id], |_| Ok(()))
        .optional()?
        .is_some())
}

fn load_tasks(conn: &Connection) -> Result<Vec<Task>> {
    let mut stmt = conn.prepare(SELECT_TASKS)?;
    let rows = stmt.query_map([], task_from_row)?;
    let mut tasks: Vec<Task> = rows.collect::<rusqlite::Result<_>>()?;
    tasks.sort_by(|a, b| cmp_task_ids(&a.id, &b.id));
    Ok(tasks)
}

fn load_task(conn: &Connection, id: &str) -> Result<Task> {
    conn.query_row(
        &format!("{SELECT_TASKS} WHERE id = ?1"),
        [id],
        task_from_row,
    )
    .optional()?
    .ok_or_else(|| NotFound(format!("no task `{id}`")).into())
}

fn load_track(conn: &Connection, name: &str) -> Result<Track> {
    conn.query_row(
        &format!("{SELECT_TRACKS} WHERE name = ?1"),
        [name],
        track_from_row,
    )
    .optional()?
    .ok_or_else(|| NotFound(format!("no track `{name}`")).into())
}

fn track_exists(conn: &Connection, name: &str) -> Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM tracks WHERE name = ?1", [name], |_| Ok(()))
        .optional()?
        .is_some())
}

/// Fails with the defined track names unless `name` is a track.
fn ensure_track(conn: &Connection, name: &str) -> Result<()> {
    if track_exists(conn, name)? {
        return Ok(());
    }
    let mut stmt = conn.prepare("SELECT name FROM tracks ORDER BY name")?;
    let names: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if names.is_empty() {
        bail!("no track `{name}`; no tracks defined (airlock track add)");
    }
    bail!("no track `{name}`; defined tracks: {}", names.join(", "))
}

fn load_question(conn: &Connection, id: u64) -> Result<Question> {
    let mut question = conn
        .query_row(
            &format!("{SELECT_QUESTIONS} WHERE id = ?1"),
            [sql_id(id)?],
            question_from_row,
        )
        .optional()?
        .ok_or_else(|| NotFound(format!("no question #{id}")))?;
    question.history = load_history(conn, id)?;
    Ok(question)
}

/// The answers a question's corrections replaced, oldest first.
fn load_history(conn: &Connection, id: u64) -> Result<Vec<AnswerVersion>> {
    let mut stmt = conn.prepare(
        "SELECT answer, answered_by, answered_at, spec_file FROM answer_history
          WHERE question_id = ?1 ORDER BY seq",
    )?;
    let rows = stmt.query_map([sql_id(id)?], answer_version_from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Every question's replaced answers, oldest first, keyed by question id.
fn load_all_history(conn: &Connection) -> Result<HashMap<u64, Vec<AnswerVersion>>> {
    let mut stmt = conn.prepare(
        "SELECT answer, answered_by, answered_at, spec_file, question_id FROM answer_history
          ORDER BY question_id, seq",
    )?;
    let mut rows = stmt.query([])?;
    let mut history: HashMap<u64, Vec<AnswerVersion>> = HashMap::new();
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(4)?;
        history
            .entry(u64::try_from(id)?)
            .or_default()
            .push(answer_version_from_row(row)?);
    }
    Ok(history)
}

fn answer_version_from_row(row: &Row) -> rusqlite::Result<AnswerVersion> {
    Ok(AnswerVersion {
        answer: row.get(0)?,
        answered_by: row.get(1)?,
        answered_at: row.get(2)?,
        spec_file: row.get(3)?,
    })
}

fn find<'a>(tasks: &'a [Task], id: &str) -> Result<&'a Task> {
    tasks
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| NotFound(format!("no task `{id}`")).into())
}

fn status_of(tasks: &[Task], task: &Task) -> TaskStatus {
    let done: HashSet<&str> = tasks
        .iter()
        .filter(|t| t.state == TaskState::Done)
        .map(|t| t.id.as_str())
        .collect();
    task.status(|id| done.contains(id))
}

/// Open dependents of `id` whose prerequisites are now all done.
fn newly_ready(conn: &Connection, id: &str) -> Result<Vec<String>> {
    let tasks = load_tasks(conn)?;
    let by_id: HashMap<&str, &Task> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();
    Ok(dependents(&tasks, id)
        .into_iter()
        .filter(|dep| status_of(&tasks, by_id[dep]) == TaskStatus::Ready)
        .map(str::to_string)
        .collect())
}

/// Appends an event (Spec 01 §2.6) inside the caller's write transaction.
#[allow(clippy::too_many_arguments)]
fn emit(
    conn: &Connection,
    now: i64,
    kind: EventKind,
    role: &str,
    origin: Option<&str>,
    task_id: Option<&str>,
    question_id: Option<u64>,
    track: Option<&str>,
) -> Result<()> {
    let question_id = question_id.map(sql_id).transpose()?;
    conn.execute(
        "INSERT INTO events (at, kind, role, origin, task_id, question_id, track)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            now,
            kind.as_str(),
            role,
            origin,
            task_id,
            question_id,
            track
        ],
    )?;
    Ok(())
}

/// Emits `kind` for the task's role when the task is ready.
fn emit_if_ready(conn: &Connection, id: &str, kind: EventKind, now: i64) -> Result<()> {
    let tasks = load_tasks(conn)?;
    let task = find(&tasks, id)?;
    if status_of(&tasks, task) == TaskStatus::Ready {
        emit(conn, now, kind, &task.role, None, Some(id), None, None)?;
    }
    Ok(())
}

/// The role that asked question `id`: from its `question_asked` event, else
/// from a worktree binding of the asking session. `None` when neither is known
/// (an imported question, or an unbound session after the event was pruned).
fn asker_role(conn: &Connection, id: u64, asked_by: &str) -> Result<Option<String>> {
    let from_event: Option<String> = conn
        .query_row(
            "SELECT origin FROM events WHERE kind = 'question_asked' AND question_id = ?1
              ORDER BY seq LIMIT 1",
            [sql_id(id)?],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    if from_event.is_some() {
        return Ok(from_event);
    }
    Ok(conn
        .query_row(
            "SELECT role FROM bindings WHERE session = ?1 ORDER BY root LIMIT 1",
            [asked_by],
            |row| row.get(0),
        )
        .optional()?)
}

fn ensure_holder(task: &Task, session: &str, now: i64) -> Result<()> {
    match task.claimed_by.as_deref() {
        Some(holder) if holder == session => Ok(()),
        Some(_) => bail!(
            "task `{}` is claimed by {}",
            task.id,
            lease_holder(task, now)
        ),
        None => bail!("task `{}` is not claimed", task.id),
    }
}

fn lease_holder(task: &Task, now: i64) -> String {
    match (&task.claimed_by, task.lease_minutes_left(now)) {
        (Some(session), Some(minutes)) => format!("{session} (lease expires in {minutes} min)"),
        (Some(session), None) => session.clone(),
        _ => "nobody".to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
fn set_state(
    conn: &Connection,
    id: &str,
    from: TaskState,
    to: TaskState,
    claimed_by: Option<&str>,
    lease_expires_at: Option<i64>,
    notes: Option<&str>,
    now: i64,
) -> Result<()> {
    let changed = conn.execute(
        "UPDATE tasks
            SET state = ?1, claimed_by = ?2, lease_expires_at = ?3, notes = coalesce(?4, notes), updated_at = ?5
          WHERE id = ?6 AND state = ?7",
        params![to.as_str(), claimed_by, lease_expires_at, notes, now, id, from.as_str()],
    )?;
    ensure!(changed == 1, "task `{id}` changed concurrently; retry");
    Ok(())
}

fn commit_task(tx: Transaction<'_>, id: &str) -> Result<Task> {
    let task = load_task(&tx, id)?;
    tx.commit()?;
    Ok(task)
}

fn released_if_expired(mut task: Task, now: i64) -> Task {
    if task.lease_expired(now) {
        task.state = TaskState::Open;
        task.claimed_by = None;
        task.lease_expires_at = None;
    }
    task
}

fn ensure_unlocked(conn: &Connection, root: &str, force: bool) -> Result<()> {
    let locked_role: Option<String> = conn
        .query_row(
            "SELECT role FROM bindings WHERE root = ?1 AND locked = 1",
            [root],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(role) = locked_role.filter(|_| !force) {
        bail!("this worktree is locked to role `{role}`; pass --force to change it");
    }
    Ok(())
}

/// Question ids are `u64` in the model and `INTEGER` (i64) in SQLite.
fn sql_id(id: u64) -> Result<i64> {
    i64::try_from(id).with_context(|| format!("question id {id} out of range"))
}

fn json_column<T: serde::de::DeserializeOwned>(row: &Row<'_>, idx: usize) -> rusqlite::Result<T> {
    let text: String = row.get(idx)?;
    serde_json::from_str(&text).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, Box::new(e))
    })
}

/// Parses a snake_case enum column through its serde representation.
fn enum_column<T: serde::de::DeserializeOwned>(row: &Row<'_>, idx: usize) -> rusqlite::Result<T> {
    let text: String = row.get(idx)?;
    serde_json::from_value(serde_json::Value::String(text)).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, Box::new(e))
    })
}

/// Reads a priority rank, rejecting values outside 0 to 3; `owner` names the
/// row in the error ("task `L-48`").
fn priority_column(row: &Row<'_>, idx: usize, owner: &str) -> rusqlite::Result<Priority> {
    let rank: i64 = row.get(idx)?;
    Priority::from_rank(rank).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            idx,
            rusqlite::types::Type::Integer,
            format!("{owner} has priority rank {rank}, outside 0 to 3").into(),
        )
    })
}

fn task_from_row(row: &Row<'_>) -> rusqlite::Result<Task> {
    let id: String = row.get(0)?;
    let priority = priority_column(row, 4, &format!("task `{id}`"))?;
    Ok(Task {
        id,
        title: row.get(1)?,
        role: row.get(2)?,
        state: enum_column(row, 3)?,
        priority,
        blocked_by: json_column(row, 5)?,
        read: json_column(row, 6)?,
        touch: json_column(row, 7)?,
        r#do: row.get(8)?,
        verify: row.get(9)?,
        notes: row.get(10)?,
        claimed_by: row.get(11)?,
        lease_expires_at: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
        track: row.get(15)?,
        progress: row.get(16)?,
    })
}

fn track_from_row(row: &Row<'_>) -> rusqlite::Result<Track> {
    let name: String = row.get(0)?;
    let priority = priority_column(row, 1, &format!("track `{name}`"))?;
    Ok(Track {
        name,
        priority,
        description: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
    })
}

fn question_from_row(row: &Row<'_>) -> rusqlite::Result<Question> {
    let id: i64 = row.get(0)?;
    Ok(Question {
        id: u64::try_from(id).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Integer,
                Box::new(e),
            )
        })?,
        topic: row.get(1)?,
        asked_by: row.get(2)?,
        question: row.get(3)?,
        status: enum_column(row, 4)?,
        answer: row.get(5)?,
        answered_by: row.get(6)?,
        spec_file: row.get(7)?,
        created_at: row.get(8)?,
        answered_at: row.get(9)?,
        corrected_at: row.get(10)?,
        corrected_by: row.get(11)?,
        history: Vec::new(),
    })
}
