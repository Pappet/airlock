//! Activity log: who called which airlock command, when, and what changed.
//!
//! Lives in `activity.db` next to a project's `airlock.db`, separate from the
//! hub store so logging can never contend with or corrupt task state. Entries
//! carry metadata only (no free-text bodies); they are pruned after
//! [`RETENTION_SECS`] on every write. A second table registers the running
//! airlock processes (MCP servers and dashboards) with their build identity.

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, Row, TransactionBehavior, params, params_from_iter};
use serde::{Deserialize, Serialize};
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

    /// When `session` last ran a briefing command, if ever.
    pub fn last_briefing(&self, session: &str) -> Result<Option<i64>> {
        let placeholders = vec!["?"; BRIEFING_COMMANDS.len()].join(", ");
        let sql = format!(
            "SELECT MAX(at) FROM entries WHERE session = ? AND command IN ({placeholders})"
        );
        let args = std::iter::once(session).chain(BRIEFING_COMMANDS.iter().copied());
        let at: Option<i64> = self
            .conn
            .query_row(&sql, params_from_iter(args), |row| row.get(0))?;
        Ok(at)
    }

    /// Register the current process and return its registry id.
    pub fn register(
        &mut self,
        kind: &str,
        session: Option<&str>,
        role: Option<&str>,
        now: i64,
    ) -> Result<u64> {
        self.conn.execute(
            "INSERT INTO processes (pid, build, kind, session, role, started_at, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![std::process::id(), build(), kind, session, role, now],
        )?;
        Ok(self.conn.last_insert_rowid() as u64)
    }

    /// Heartbeat: mark the process as seen at `now`.
    pub fn touch(&mut self, id: u64, now: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE processes SET last_seen = ?2 WHERE id = ?1",
            params![id as i64, now],
        )?;
        Ok(())
    }

    pub fn unregister(&mut self, id: u64) -> Result<()> {
        self.conn
            .execute("DELETE FROM processes WHERE id = ?1", [id as i64])?;
        Ok(())
    }

    /// Processes seen within [`LIVE_SECS`] of `now`; older rows are deleted.
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
