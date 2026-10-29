//! SQLite hub store for one project (Spec 01 §3.2).
//!
//! Every mutation runs in a `BEGIN IMMEDIATE` transaction that first resets
//! expired leases, then checks the transition against current state, then
//! applies it with an update conditioned on the expected prior state. Reads
//! present expired leases as already released. Callers pass `now` (unix
//! seconds) so behavior is testable without a clock.

use crate::core::{
    Question, QuestionStatus, Task, TaskState, TaskStatus, check_task_id, dependents,
};
use crate::hub::{Binding, Bindings};
use crate::snapshot::Snapshot;
use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

/// Schema migrations; entry `n` upgrades `user_version` `n` to `n + 1`.
const MIGRATIONS: &[&str] = &[SCHEMA_V1, SCHEMA_V2, SCHEMA_V3];

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

const SELECT_TASKS: &str = "
SELECT id, title, role, state, priority,
       (SELECT json_group_array(blocked_by)
          FROM (SELECT blocked_by FROM task_deps WHERE task_id = t.id ORDER BY rowid)),
       read, touch, do, verify, notes, claimed_by, lease_expires_at, created_at, updated_at
  FROM tasks t";

const SELECT_QUESTIONS: &str = "
SELECT id, topic, asked_by, question, status, answer, answered_by, spec_file, created_at, answered_at
  FROM questions";

/// Fields of a task to be created.
#[derive(Debug, Clone)]
pub struct NewTask {
    pub id: String,
    pub title: String,
    pub role: String,
    pub priority: i32,
    pub blocked_by: Vec<String>,
    pub read: Vec<String>,
    pub touch: Vec<String>,
    pub r#do: String,
    pub verify: String,
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
        let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
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
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn question(&self, id: u64) -> Result<Question> {
        load_question(&self.conn, id)
    }

    pub fn add_task(&mut self, new: NewTask, now: i64) -> Result<Task> {
        check_task_id(&new.id).map_err(anyhow::Error::msg)?;
        let tx = self.write(now)?;
        ensure!(
            !task_exists(&tx, &new.id)?,
            "task `{}` already exists",
            new.id
        );
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
            "INSERT INTO tasks (id, title, role, state, priority, read, touch, do, verify, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'open', ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
            params![
                new.id,
                new.title,
                new.role,
                new.priority,
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
                params![new.id, dep],
            )?;
        }
        let task = load_task(&tx, &new.id)?;
        tx.commit()?;
        Ok(task)
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

    pub fn heartbeat(
        &mut self,
        id: &str,
        session: &str,
        lease_secs: i64,
        now: i64,
    ) -> Result<Task> {
        let tx = self.write(now)?;
        let task = load_task(&tx, id)?;
        ensure_holder(&task, session, now)?;
        let changed = tx.execute(
            "UPDATE tasks SET lease_expires_at = ?1, updated_at = ?2
              WHERE id = ?3 AND state = 'in_progress' AND claimed_by = ?4",
            params![now + lease_secs, now, id, session],
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
            Vec::new()
        };
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
        commit_task(tx, id)
    }

    pub fn ask(
        &mut self,
        topic: &str,
        question: &str,
        session: &str,
        now: i64,
    ) -> Result<Question> {
        let tx = self.write(now)?;
        tx.execute(
            "INSERT INTO questions (topic, asked_by, question, status, created_at) VALUES (?1, ?2, ?3, 'open', ?4)",
            params![topic, session, question, now],
        )?;
        let id = u64::try_from(tx.last_insert_rowid())?;
        let question = load_question(&tx, id)?;
        tx.commit()?;
        Ok(question)
    }

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
        ensure!(
            question.status != QuestionStatus::Resolved,
            "question #{id} is already resolved"
        );
        tx.execute(
            "UPDATE questions
                SET status = 'answered', answer = ?1, answered_by = ?2, answered_at = ?3,
                    spec_file = coalesce(?4, spec_file)
              WHERE id = ?5",
            params![answer, session, now, spec_file, sql_id(id)?],
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

    /// Replaces all tasks and questions with the snapshot's content and records
    /// the snapshot's hash (`sha256`) as known, in one transaction. The caller
    /// validates the snapshot first. Question ids are kept; new questions
    /// continue after the highest id this store ever used.
    pub fn replace_all(&mut self, snapshot: &Snapshot, sha256: &str, now: i64) -> Result<()> {
        let tx = self.write(now)?;
        tx.execute(
            "INSERT OR IGNORE INTO exports (sha256, recorded_at) VALUES (?1, ?2)",
            params![sha256, now],
        )?;
        tx.execute_batch("DELETE FROM task_deps; DELETE FROM tasks; DELETE FROM questions;")?;
        for t in &snapshot.tasks {
            let state = match t.state {
                TaskState::InProgress => TaskState::Open,
                state => state,
            };
            tx.execute(
                "INSERT INTO tasks (id, title, role, state, priority, read, touch, do, verify, notes, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    t.id,
                    t.title,
                    t.role,
                    state.as_str(),
                    t.priority,
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
                "INSERT INTO questions (id, topic, asked_by, question, status, answer, answered_by, spec_file, created_at, answered_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
                    q.answered_at
                ],
            )?;
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
        Ok(tx)
    }
}

fn task_exists(conn: &Connection, id: &str) -> Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM tasks WHERE id = ?1", [id], |_| Ok(()))
        .optional()?
        .is_some())
}

fn load_tasks(conn: &Connection) -> Result<Vec<Task>> {
    let mut stmt = conn.prepare(&format!("{SELECT_TASKS} ORDER BY id"))?;
    let rows = stmt.query_map([], task_from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
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

fn load_question(conn: &Connection, id: u64) -> Result<Question> {
    conn.query_row(
        &format!("{SELECT_QUESTIONS} WHERE id = ?1"),
        [sql_id(id)?],
        question_from_row,
    )
    .optional()?
    .ok_or_else(|| NotFound(format!("no question #{id}")).into())
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

fn task_from_row(row: &Row<'_>) -> rusqlite::Result<Task> {
    Ok(Task {
        id: row.get(0)?,
        title: row.get(1)?,
        role: row.get(2)?,
        state: enum_column(row, 3)?,
        priority: row.get(4)?,
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
    })
}
