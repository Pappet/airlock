//! Activity log: who called which airlock command, when, and what changed.
//!
//! Lives in `activity.db` next to a project's `airlock.db`, separate from the
//! hub store so logging can never contend with or corrupt task state. Entries
//! carry metadata only (no free-text bodies); they are pruned after
//! [`RETENTION_SECS`] on every write. A second table registers the running
//! airlock processes (MCP servers and dashboards) with their build identity.

use crate::cli::{Command, QuestionCommand, RoleCommand, TaskCommand, TrackCommand};
use crate::store::Store;
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, Row, TransactionBehavior, params, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Entries older than this many seconds (30 days) are pruned on write.
pub const RETENTION_SECS: i64 = 30 * 86_400;
/// A process counts as live for this long after its last heartbeat.
pub const LIVE_SECS: i64 = 180;
/// How often a running process should refresh its heartbeat.
pub const TOUCH_SECS: u64 = 60;
/// Longest `error` text kept, in characters.
pub const ERROR_MAX: usize = 300;
/// Commands that count as an agent's session briefing.
pub const BRIEFING_COMMANDS: &[&str] = &["session_start", "prompt start", "start"];

/// Arguments whose short values are kept in `changed` as `name: value`.
/// Every other argument contributes its key only.
const VALUE_KEYS: &[&str] = &[
    "priority",
    "track",
    "role",
    "status",
    "review",
    "lease_minutes",
    "clean_room",
    "lock",
];
/// Argument keys that name what a call acts on, most specific first.
const TARGET_KEYS: &[&str] = &["task_id", "question_id", "id", "name"];
/// Longest value kept for a [`VALUE_KEYS`] argument, in characters.
const VALUE_MAX: usize = 40;

/// Commands that only read: the MCP and CLI reads, plus every REST `GET`.
const READ_COMMANDS: &[&str] = &[
    "session_start",
    "prompt start",
    "task_next",
    "task_get",
    "question_list",
    "track_list",
    "start",
    "task list",
    "task next",
    "task show",
    "track list",
    "question list",
    "role list",
];

const ACTIVITY_FILE: &str = "activity.db";

const MIGRATIONS: &[&str] = &[SCHEMA_V1];

const SCHEMA_V1: &str = "
CREATE TABLE entries (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    at         INTEGER NOT NULL,
    session    TEXT NOT NULL,
    role       TEXT,
    clean_room INTEGER NOT NULL,
    channel    TEXT NOT NULL CHECK (channel IN ('mcp', 'cli', 'rest')),
    agent      TEXT,
    command    TEXT NOT NULL,
    target     TEXT,
    changed    TEXT NOT NULL,
    write      INTEGER NOT NULL,
    ok         INTEGER NOT NULL,
    error      TEXT,
    pid        INTEGER NOT NULL,
    build      TEXT NOT NULL
) STRICT;
CREATE INDEX entries_at ON entries (at);

CREATE TABLE processes (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    pid        INTEGER NOT NULL,
    build      TEXT NOT NULL,
    kind       TEXT NOT NULL,
    session    TEXT,
    role       TEXT,
    started_at INTEGER NOT NULL,
    last_seen  INTEGER NOT NULL
) STRICT;
";

const SELECT_ENTRIES: &str = "
SELECT id, at, session, role, clean_room, channel, agent, command, target, changed,
       write, ok, error, pid, build
  FROM entries";

/// Where a call came in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Mcp,
    Cli,
    Rest,
}

impl Channel {
    fn as_str(self) -> &'static str {
        match self {
            Channel::Mcp => "mcp",
            Channel::Cli => "cli",
            Channel::Rest => "rest",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "mcp" => Some(Channel::Mcp),
            "cli" => Some(Channel::Cli),
            "rest" => Some(Channel::Rest),
            _ => None,
        }
    }
}

/// One recorded call. `id` is 0 until the entry is recorded.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: u64,
    pub at: i64,
    pub session: String,
    pub role: Option<String>,
    pub clean_room: bool,
    pub channel: Channel,
    pub agent: Option<String>,
    pub command: String,
    pub target: Option<String>,
    pub changed: Vec<String>,
    pub write: bool,
    pub ok: bool,
    pub error: Option<String>,
    pub pid: u32,
    pub build: String,
}

/// Which entries to read; every set field narrows the result.
#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub session: Option<String>,
    pub role: Option<String>,
    pub channel: Option<Channel>,
    pub since: Option<i64>,
    pub writes_only: bool,
    pub failures_only: bool,
    pub limit: Option<usize>,
}

/// The start of a time window: some time ago, or an absolute time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Since {
    /// This many seconds before now
    Ago(i64),
    /// This unix time
    At(i64),
}

impl Since {
    /// The unix time the window starts at, seen from `now`.
    pub fn resolve(self, now: i64) -> i64 {
        match self {
            Since::Ago(secs) => now - secs,
            Since::At(at) => at,
        }
    }
}

impl std::str::FromStr for Since {
    type Err = String;

    /// `<n>m`, `<n>h`, `<n>d`, or plain unix seconds.
    fn from_str(text: &str) -> Result<Self, String> {
        let invalid = || format!("invalid time `{text}`: use 30m, 2h, 1d, or unix seconds");
        let (digits, unit) = match text.chars().last() {
            Some('m') => (&text[..text.len() - 1], Some(60)),
            Some('h') => (&text[..text.len() - 1], Some(3_600)),
            Some('d') => (&text[..text.len() - 1], Some(86_400)),
            _ => (text, None),
        };
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid());
        }
        let n: i64 = digits.parse().map_err(|_| invalid())?;
        match unit {
            Some(unit) => n.checked_mul(unit).map(Since::Ago).ok_or_else(invalid),
            None => Ok(Since::At(n)),
        }
    }
}

/// A running airlock process; `kind` is `"mcp"` or `"serve"`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Process {
    pub id: u64,
    pub pid: u32,
    pub build: String,
    pub kind: String,
    pub session: Option<String>,
    pub role: Option<String>,
    pub started_at: i64,
    pub last_seen: i64,
}

pub struct ActivityLog {
    conn: Connection,
}

impl ActivityLog {
    /// The activity log file belonging to a hub database: its sibling
    /// `activity.db`.
    pub fn path_for(database: &Path) -> PathBuf {
        database.with_file_name(ACTIVITY_FILE)
    }

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

    /// Append an entry (pruning anything past retention in the same
    /// transaction) and return its id.
    pub fn record(&mut self, entry: &Entry) -> Result<u64> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM entries WHERE at < ?1",
            [entry.at - RETENTION_SECS],
        )?;
        tx.execute(
            "INSERT INTO entries (at, session, role, clean_room, channel, agent, command, target,
                                  changed, write, ok, error, pid, build)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                entry.at,
                entry.session,
                entry.role,
                entry.clean_room,
                entry.channel.as_str(),
                entry.agent,
                entry.command,
                entry.target,
                serde_json::to_string(&entry.changed)?,
                entry.write,
                entry.ok,
                entry.error.as_deref().map(truncate_error),
                entry.pid,
                entry.build,
            ],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id as u64)
    }

    /// Entries matching `filter`, newest first.
    pub fn entries(&self, filter: &Filter) -> Result<Vec<Entry>> {
        let mut clauses: Vec<&str> = Vec::new();
        let mut args: Vec<rusqlite::types::Value> = Vec::new();
        if let Some(session) = &filter.session {
            clauses.push("session = ?");
            args.push(session.clone().into());
        }
        if let Some(role) = &filter.role {
            clauses.push("role = ?");
            args.push(role.clone().into());
        }
        if let Some(channel) = filter.channel {
            clauses.push("channel = ?");
            args.push(channel.as_str().to_string().into());
        }
        if let Some(since) = filter.since {
            clauses.push("at >= ?");
            args.push(since.into());
        }
        if filter.writes_only {
            clauses.push("write = 1");
        }
        if filter.failures_only {
            clauses.push("ok = 0");
        }
        let mut sql = SELECT_ENTRIES.to_string();
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        sql.push_str(" ORDER BY id DESC");
        if let Some(limit) = filter.limit {
            sql.push_str(&format!(" LIMIT {limit}"));
        }
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args), entry_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Id of the newest entry, 0 when the log is empty.
    pub fn latest_id(&self) -> Result<u64> {
        let id: Option<i64> = self
            .conn
            .query_row("SELECT MAX(id) FROM entries", [], |row| row.get(0))?;
        Ok(id.unwrap_or(0) as u64)
    }

    /// When `session` last got a briefing (a successful briefing command), if ever.
    pub fn last_briefing(&self, session: &str) -> Result<Option<i64>> {
        let placeholders = vec!["?"; BRIEFING_COMMANDS.len()].join(", ");
        let sql = format!(
            "SELECT MAX(at) FROM entries WHERE session = ? AND ok = 1 AND command IN ({placeholders})"
        );
        let args = std::iter::once(session).chain(BRIEFING_COMMANDS.iter().copied());
        let at: Option<i64> = self
            .conn
            .query_row(&sql, params_from_iter(args), |row| row.get(0))?;
        Ok(at)
    }

    /// Register the current process and return its registry row. Keep it:
    /// [`touch`](Self::touch) needs it to re-register after a gap.
    pub fn register(
        &mut self,
        kind: &str,
        session: Option<&str>,
        role: Option<&str>,
        now: i64,
    ) -> Result<Process> {
        let pid = std::process::id();
        self.conn.execute(
            "INSERT INTO processes (pid, build, kind, session, role, started_at, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![pid, build(), kind, session, role, now],
        )?;
        Ok(Process {
            id: self.conn.last_insert_rowid() as u64,
            pid,
            build: build().to_string(),
            kind: kind.to_string(),
            session: session.map(str::to_string),
            role: role.map(str::to_string),
            started_at: now,
            last_seen: now,
        })
    }

    /// Heartbeat: mark the process as seen at `now`. A row that a reader
    /// pruned during a heartbeat gap (a suspend, a clock jump) comes back
    /// under the same id, so a live process never stays unlisted.
    pub fn touch(&mut self, process: &Process, now: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO processes (id, pid, build, kind, session, role, started_at, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (id) DO UPDATE SET last_seen = excluded.last_seen",
            params![
                process.id as i64,
                process.pid,
                process.build,
                process.kind,
                process.session,
                process.role,
                process.started_at,
                now
            ],
        )?;
        Ok(())
    }

    pub fn unregister(&mut self, id: u64) -> Result<()> {
        self.conn
            .execute("DELETE FROM processes WHERE id = ?1", [id as i64])?;
        Ok(())
    }

    /// Processes seen within [`LIVE_SECS`] of `now`; older rows are deleted
    /// (a live process puts its row back on its next heartbeat).
    pub fn processes(&mut self, now: i64) -> Result<Vec<Process>> {
        self.conn.execute(
            "DELETE FROM processes WHERE last_seen < ?1",
            [now - LIVE_SECS],
        )?;
        let mut stmt = self.conn.prepare(
            "SELECT id, pid, build, kind, session, role, started_at, last_seen
               FROM processes ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Process {
                id: row.get::<_, i64>(0)? as u64,
                pid: row.get(1)?,
                build: row.get(2)?,
                kind: row.get(3)?,
                session: row.get(4)?,
                role: row.get(5)?,
                started_at: row.get(6)?,
                last_seen: row.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

/// Switch to WAL. On a fresh file SQLite answers a concurrent switch with
/// SQLITE_BUSY without consulting the busy timeout, so retry for a while.
fn enable_wal(conn: &Connection) -> Result<String> {
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

fn entry_from_row(row: &Row) -> rusqlite::Result<Entry> {
    let bad = |column: usize, message: String| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            message.into(),
        )
    };
    let channel: String = row.get(5)?;
    let changed: String = row.get(9)?;
    Ok(Entry {
        id: row.get::<_, i64>(0)? as u64,
        at: row.get(1)?,
        session: row.get(2)?,
        role: row.get(3)?,
        clean_room: row.get(4)?,
        channel: Channel::parse(&channel)
            .ok_or_else(|| bad(5, format!("unknown channel {channel:?}")))?,
        agent: row.get(6)?,
        command: row.get(7)?,
        target: row.get(8)?,
        changed: serde_json::from_str(&changed).map_err(|e| bad(9, e.to_string()))?,
        write: row.get(10)?,
        ok: row.get(11)?,
        error: row.get(12)?,
        pid: row.get(13)?,
        build: row.get(14)?,
    })
}

/// Record an entry for the project whose hub database is `database`. A
/// failure never propagates: it is reported on stderr and swallowed, since
/// logging must not break the call it describes.
pub fn record_quietly(database: &Path, entry: &Entry) {
    let result = ActivityLog::open(&ActivityLog::path_for(database))
        .and_then(|mut log| log.record(entry).map(|_| ()));
    if let Err(e) = result {
        eprintln!("airlock: activity log: {e:#}");
    }
}

/// When `session` last ran a briefing command in the project whose hub
/// database is `database`. A log that cannot be opened or read counts as no
/// briefing yet (`None`); the failure is reported on stderr.
pub fn last_briefing_quietly(database: &Path, session: &str) -> Option<i64> {
    let result = ActivityLog::open(&ActivityLog::path_for(database))
        .and_then(|log| log.last_briefing(session));
    match result {
        Ok(at) => at,
        Err(e) => {
            eprintln!("airlock: activity log: {e:#}");
            None
        }
    }
}

/// Build identity of this binary: `<version>+<unix seconds at build>`.
pub fn build() -> &'static str {
    env!("AIRLOCK_BUILD")
}

/// The build time in a [`build`] string, 0 when absent or unparsable.
pub fn build_time(build: &str) -> i64 {
    build
        .split_once('+')
        .and_then(|(_, time)| time.parse().ok())
        .unwrap_or(0)
}

/// Which agent runs this process, from its environment: `AI_AGENT` (text
/// before the first `_`), else `CLAUDECODE=1`.
pub fn agent_from_env(var: impl Fn(&str) -> Option<String>) -> Option<String> {
    if let Some(agent) = var("AI_AGENT") {
        let name = agent.split('_').next().unwrap_or_default();
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    (var("CLAUDECODE").as_deref() == Some("1")).then(|| "claude-code".to_string())
}

/// Cut an error message to [`ERROR_MAX`] characters.
pub fn truncate_error(message: &str) -> String {
    message.chars().take(ERROR_MAX).collect()
}

/// Whether answering question `id` now would correct an earlier answer: the
/// question exists and already has one. Callers check this before the answer
/// runs and add `correction` to the entry's `changed` list.
pub fn is_correction(store: &Store, id: u64) -> bool {
    store.question(id).is_ok_and(|q| q.answer.is_some())
}

/// Whether `command` changes state: false for the reads (MCP, CLI and REST
/// `GET`), true for everything else.
pub fn is_write(command: &str) -> bool {
    !(READ_COMMANDS.contains(&command) || command.starts_with("GET "))
}

/// The target and changed fields of an entry, from a call's JSON arguments.
///
/// The log holds no free text, so this never copies an argument value except
/// the short ones of [`VALUE_KEYS`]: a value that is not a short plain word
/// (letters, digits, `-`, `_`, `.`) is reduced to its key. `target` is the
/// first of `task_id`, `question_id`, `id`, `name` that holds a string or a
/// number (`question_id` numbers read `#<n>`); `changed` lists every other
/// key, with `new_name` recorded as plain `name`. `_command` is unused so far:
/// no command needs rules of its own.
pub fn summarize_json(_command: &str, args: &Map<String, Value>) -> (Option<String>, Vec<String>) {
    let target = TARGET_KEYS.iter().find_map(|&key| {
        let text = match args.get(key)? {
            Value::String(s) => s.clone(),
            Value::Number(n) if key == "question_id" => format!("#{n}"),
            Value::Number(n) => n.to_string(),
            _ => return None,
        };
        Some((key, text))
    });
    let target_key = target.as_ref().map(|(key, _)| *key);
    let changed = args
        .iter()
        .filter(|(key, _)| Some(key.as_str()) != target_key)
        .map(|(key, value)| {
            let key = if key == "new_name" { "name" } else { key };
            match value_text(key, value) {
                Some(text) => format!("{key}: {text}"),
                None => key.to_string(),
            }
        })
        .collect();
    (target.map(|(_, text)| text), changed)
}

/// The target and changed fields of an entry, from a parsed CLI command.
///
/// `target` is what the command acts on: a task id, a question as `#<id>`, a
/// track or role name (for `use`, the role it binds). `changed` lists the
/// arguments the caller gave, in the key vocabulary of the MCP tools (`--lease`
/// is `lease_minutes`, `--spec` is `spec_file`, dashes become underscores) and
/// under the same rule as [`summarize_json`]: a key only, except for the short
/// values of [`VALUE_KEYS`]. Flags with a default (`--priority` of `task add`
/// and `track add`) cannot be told from given ones and are always listed.
pub fn summarize_cli(command: &Command) -> (Option<String>, Vec<String>) {
    let mut changed = Changed::default();
    let target = match command {
        Command::Use {
            role,
            lock,
            force,
            clear,
        } => {
            changed.flag("lock", *lock);
            changed.flag("force", *force);
            changed.flag("clear", *clear);
            role.clone()
        }
        Command::Start { role } => {
            changed.opt("role", role);
            None
        }
        Command::Role(sub) => match sub {
            RoleCommand::List => None,
            RoleCommand::Set {
                name,
                description,
                clean_room,
                instructions,
                read,
                clear_read,
                prefix,
            } => {
                changed.opt("description", description);
                changed.opt("clean_room", clean_room);
                changed.opt("instructions", instructions);
                changed.list("read", read);
                changed.flag("clear_read", *clear_read);
                changed.opt("prefix", prefix);
                Some(name.clone())
            }
            RoleCommand::Remove { name } => Some(name.clone()),
        },
        Command::Task(sub) => summarize_task(sub, &mut changed),
        Command::Track(sub) => match sub {
            TrackCommand::List => None,
            TrackCommand::Add {
                name,
                priority,
                description,
            } => {
                changed.set("priority", priority);
                changed.flag("description", !description.is_empty());
                Some(name.clone())
            }
            TrackCommand::Edit {
                name,
                new_name,
                priority,
                description,
            } => {
                changed.flag("name", new_name.is_some());
                changed.opt("priority", priority);
                changed.opt("description", description);
                Some(name.clone())
            }
            TrackCommand::Remove { name } => Some(name.clone()),
        },
        Command::Question(sub) => match sub {
            QuestionCommand::List { topic, status } => {
                changed.opt("topic", topic);
                changed.opt("status", status);
                None
            }
            QuestionCommand::Ask { .. } => {
                changed.flag("question", true);
                changed.flag("topic", true);
                None
            }
            QuestionCommand::Answer { id, spec, .. } => {
                changed.flag("answer", true);
                changed.opt("spec_file", spec);
                Some(format!("#{id}"))
            }
            QuestionCommand::Resolve { id, spec } => {
                changed.opt("spec_file", spec);
                Some(format!("#{id}"))
            }
        },
        Command::Sync { force } => {
            changed.flag("force", *force);
            None
        }
        Command::Import { from, yes } => {
            changed.opt("from", from);
            changed.flag("yes", *yes);
            None
        }
        // Not logged: nothing to summarize.
        Command::Init { .. }
        | Command::Doctor
        | Command::Setup { .. }
        | Command::Activity { .. }
        | Command::Mcp { .. }
        | Command::Serve { .. } => None,
    };
    (target, changed.0)
}

fn summarize_task(sub: &TaskCommand, changed: &mut Changed) -> Option<String> {
    match sub {
        TaskCommand::List {
            role,
            status,
            track,
            no_track,
            priority,
        } => {
            changed.opt("role", role);
            changed.opt("status", status);
            changed.opt("track", track);
            changed.flag("no_track", *no_track);
            changed.opt("priority", priority);
            None
        }
        TaskCommand::Next { role } => {
            changed.opt("role", role);
            None
        }
        TaskCommand::Show { id } => Some(id.clone()),
        TaskCommand::Add(spec) => {
            changed.flag("title", true);
            changed.set("role", &spec.role);
            changed.list("read", &spec.read);
            changed.list("touch", &spec.touch);
            changed.flag("do", true);
            changed.flag("verify", true);
            changed.list("blocked_by", &spec.blocked_by);
            changed.set("priority", spec.priority);
            changed.opt("track", &spec.track);
            None
        }
        TaskCommand::Edit(spec) => {
            changed.flag("title", spec.title.is_some());
            changed.opt("role", &spec.role);
            changed.opt("priority", &spec.priority);
            changed.opt("track", &spec.track);
            changed.flag("clear_track", spec.clear_track);
            changed.list("read", &spec.read);
            changed.flag("clear_read", spec.clear_read);
            changed.list("touch", &spec.touch);
            changed.flag("clear_touch", spec.clear_touch);
            changed.flag("do", spec.r#do.is_some());
            changed.flag("verify", spec.verify.is_some());
            changed.list("blocked_by", &spec.blocked_by);
            changed.flag("clear_blocked_by", spec.clear_blocked_by);
            changed.flag("force", spec.force);
            Some(spec.id.clone())
        }
        TaskCommand::Claim { id, lease } | TaskCommand::Heartbeat { id, lease } => {
            changed.opt("lease_minutes", lease);
            Some(id.clone())
        }
        TaskCommand::Release { id, force } => {
            changed.flag("force", *force);
            Some(id.clone())
        }
        TaskCommand::Done {
            id,
            notes,
            review,
            force,
        } => {
            changed.flag("notes", notes.is_some());
            changed.flag("review", *review);
            changed.flag("force", *force);
            Some(id.clone())
        }
        TaskCommand::Approve { id, notes } => {
            changed.flag("notes", notes.is_some());
            Some(id.clone())
        }
        TaskCommand::Reject { id, .. } => {
            changed.flag("notes", true);
            Some(id.clone())
        }
    }
}

/// The `changed` list of a CLI call, built with the rules of [`summarize_json`].
#[derive(Default)]
struct Changed(Vec<String>);

impl Changed {
    /// `key`, with its value when [`value_text`] allows it.
    fn set(&mut self, key: &str, value: impl Serialize) {
        let value = serde_json::to_value(value).unwrap_or(Value::Null);
        self.0.push(match value_text(key, &value) {
            Some(text) => format!("{key}: {text}"),
            None => key.to_string(),
        });
    }

    /// A boolean flag: listed when set, with `true` if its key is enumerated.
    fn flag(&mut self, key: &str, given: bool) {
        if given {
            self.set(key, true);
        }
    }

    /// An optional argument: listed when given.
    fn opt<T: Serialize>(&mut self, key: &str, value: &Option<T>) {
        if let Some(value) = value {
            self.set(key, value);
        }
    }

    /// A repeatable argument: its key, when any value was given. Never the values.
    fn list<T>(&mut self, key: &str, values: &[T]) {
        if !values.is_empty() {
            self.0.push(key.to_string());
        }
    }
}

/// The value of an enumerated key as short plain text, else `None`.
fn value_text(key: &str, value: &Value) -> Option<String> {
    if !VALUE_KEYS.contains(&key) {
        return None;
    }
    let text = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        _ => return None,
    };
    let plain = text
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'));
    (plain && !text.is_empty() && text.chars().count() <= VALUE_MAX).then_some(text)
}
