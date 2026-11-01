//! Core domain model: tasks, questions, and derived task status.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;

/// Persisted lifecycle state of a task.
///
/// Whether an `Open` task is blocked or ready is never stored: it is derived
/// from the state of its `blocked_by` prerequisites (see [`Task::status`]).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Not started; claimable once every prerequisite is `Done`
    Open,
    /// Claimed by a session holding a lease
    InProgress,
    /// Work submitted and awaiting review
    InReview,
    /// Completed and verified
    Done,
}

/// Effective task status as shown to users and agents.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
#[value(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Open, but at least one prerequisite is not `Done`
    Blocked,
    /// Open and every prerequisite is `Done`
    Ready,
    /// Claimed by a session
    InProgress,
    /// Awaiting review
    InReview,
    /// Completed and verified
    Done,
}

impl TaskState {
    /// The snake_case name used in JSON and the database.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::InProgress => "in_progress",
            Self::InReview => "in_review",
            Self::Done => "done",
        }
    }
}

impl TaskStatus {
    /// The snake_case name used in JSON and CLI filters.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::Ready => "ready",
            Self::InProgress => "in_progress",
            Self::InReview => "in_review",
            Self::Done => "done",
        }
    }
}

/// How urgently a task is handed out: `Critical` first, `Low` last.
///
/// Stored as its [`rank`](Priority::rank) (0 to 3) and shown by name.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
#[value(rename_all = "lowercase")]
pub enum Priority {
    /// Low priority: handed out last
    Low,
    /// The default
    #[default]
    Normal,
    /// Handed out before normal work
    High,
    /// Handed out before everything else
    Critical,
}

impl Priority {
    /// The lowercase name used in JSON, the CLI, and listings.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }

    /// The stored integer: low 0, normal 1, high 2, critical 3.
    pub fn rank(self) -> i64 {
        match self {
            Self::Low => 0,
            Self::Normal => 1,
            Self::High => 2,
            Self::Critical => 3,
        }
    }

    /// The level stored as `rank`, or `None` outside 0 to 3.
    pub fn from_rank(rank: i64) -> Option<Self> {
        match rank {
            0 => Some(Self::Low),
            1 => Some(Self::Normal),
            2 => Some(Self::High),
            3 => Some(Self::Critical),
            _ => None,
        }
    }

    /// Maps an old integer priority onto a level (schema migration 4):
    /// 30 and up is high, 10 to 29 normal, below 10 low. Never critical.
    pub fn from_legacy(value: i64) -> Self {
        match value {
            30.. => Self::High,
            10..=29 => Self::Normal,
            _ => Self::Low,
        }
    }
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Priority {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "critical" => Ok(Self::Critical),
            "high" => Ok(Self::High),
            "normal" => Ok(Self::Normal),
            "low" => Ok(Self::Low),
            _ => Err(anyhow::anyhow!(
                "invalid priority `{s}`: use critical, high, normal, or low"
            )),
        }
    }
}

/// A named line of work with its own priority; tasks may belong to one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Track {
    /// Unique name: letters, digits, `-`, `_`; case is preserved
    pub name: String,
    pub priority: Priority,
    pub description: String,
    /// Unix timestamp (seconds) of creation
    pub created_at: i64,
    /// Unix timestamp (seconds) of the last modification
    pub updated_at: i64,
}

/// A structured work package.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Task {
    /// Unique identifier (e.g. "E-6d", "L-12", "TASK-101")
    pub id: String,
    /// Short title
    pub title: String,
    /// Assigned role (e.g. "engine", "lead", "spec")
    pub role: String,
    /// Persisted lifecycle state
    pub state: TaskState,
    /// Higher priorities are handed out first by `task next`
    #[serde(default)]
    pub priority: Priority,
    /// The track this task belongs to, if any
    #[serde(default)]
    pub track: Option<String>,
    /// Prerequisites that must be `Done` before this task is ready
    #[serde(default)]
    pub blocked_by: Vec<String>,
    /// Documentation / spec files to read
    #[serde(default)]
    pub read: Vec<String>,
    /// Source files to create or modify
    #[serde(default)]
    pub touch: Vec<String>,
    /// Concrete instructions describing the behavior to implement
    pub r#do: String,
    /// Verification command or test scenario (never executed by Airlock)
    pub verify: String,
    /// Completion, review, or status notes
    pub notes: Option<String>,
    /// Session currently holding the lease (only while `InProgress`)
    pub claimed_by: Option<String>,
    /// Unix timestamp (seconds) at which the lease expires
    pub lease_expires_at: Option<i64>,
    /// The lease holder's running note for its successor (done so far, remaining,
    /// next step). Session state like `claimed_by`: never exported, kept across
    /// release and lease expiry, cleared when the task leaves `in_progress` for
    /// `done` or `in_review`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<String>,
    /// Unix timestamp (seconds) of creation
    pub created_at: i64,
    /// Unix timestamp (seconds) of the last modification
    pub updated_at: i64,
}

impl Task {
    /// Whether this task is `InProgress` with a lease that has run out at `now`.
    pub fn lease_expired(&self, now: i64) -> bool {
        self.state == TaskState::InProgress && self.lease_expires_at.is_some_and(|t| t <= now)
    }

    /// Whole minutes left on the lease at `now`, rounded up; `None` without a lease.
    pub fn lease_minutes_left(&self, now: i64) -> Option<i64> {
        self.lease_expires_at
            .map(|until| ((until - now).max(0) + 59) / 60)
    }

    /// Derives the effective status. `is_done` reports whether the task with
    /// the given id is `Done`; unknown ids must report `false`.
    pub fn status(&self, is_done: impl Fn(&str) -> bool) -> TaskStatus {
        match self.state {
            TaskState::Open if self.blocked_by.iter().all(|id| is_done(id)) => TaskStatus::Ready,
            TaskState::Open => TaskStatus::Blocked,
            TaskState::InProgress => TaskStatus::InProgress,
            TaskState::InReview => TaskStatus::InReview,
            TaskState::Done => TaskStatus::Done,
        }
    }
}

/// Checks a task id: non-empty ASCII letters, digits, `-`, `_`, or `.`.
pub fn check_task_id(id: &str) -> Result<(), String> {
    if !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        Ok(())
    } else {
        Err(format!(
            "invalid task id `{id}`: use letters, digits, `-`, `_`, or `.`"
        ))
    }
}

/// Checks a track name: non-empty ASCII letters, digits, `-`, or `_`.
pub fn check_track_name(name: &str) -> Result<(), String> {
    if !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        Ok(())
    } else {
        Err(format!(
            "invalid track name `{name}`: use letters, digits, `-`, or `_`"
        ))
    }
}

/// Checks a task id prefix: one or more ASCII letters or digits.
pub fn check_task_prefix(prefix: &str) -> Result<(), String> {
    if !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_alphanumeric()) {
        Ok(())
    } else {
        Err(format!(
            "invalid task prefix `{prefix}`: use one or more letters or digits"
        ))
    }
}

/// The sequence number of `id` under `prefix`: `Some(n)` for `<prefix>-<n>`.
pub fn task_number(id: &str, prefix: &str) -> Option<u64> {
    let digits = id.strip_prefix(prefix)?.strip_prefix('-')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Orders task ids the way people read them: runs of digits compare as numbers,
/// so `E-2` sorts before `E-10`. Ties (`E-02` and `E-2`) fall back to the text,
/// keeping the order total.
pub fn cmp_task_ids(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (x, y) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < x.len() && j < y.len() {
        if x[i].is_ascii_digit() && y[j].is_ascii_digit() {
            let run = |s: &[u8], k: usize| {
                let end = s[k..]
                    .iter()
                    .position(|c| !c.is_ascii_digit())
                    .map_or(s.len(), |n| k + n);
                (
                    end,
                    k + s[k..end].iter().take_while(|&&c| c == b'0').count(),
                )
            };
            let ((ei, si), (ej, sj)) = (run(x, i), run(y, j));
            // Without leading zeros, the longer run is the bigger number.
            let order = (ei - si)
                .cmp(&(ej - sj))
                .then_with(|| x[si..ei].cmp(&y[sj..ej]));
            if order != Ordering::Equal {
                return order;
            }
            (i, j) = (ei, ej);
        } else {
            if x[i] != y[j] {
                return x[i].cmp(&y[j]);
            }
            i += 1;
            j += 1;
        }
    }
    (x.len() - i).cmp(&(y.len() - j)).then_with(|| a.cmp(b))
}

/// Ids of the tasks that list `id` as a prerequisite (the inverse of `blocked_by`).
pub fn dependents<'a>(tasks: &'a [Task], id: &str) -> Vec<&'a str> {
    tasks
        .iter()
        .filter(|t| t.blocked_by.iter().any(|b| b == id))
        .map(|t| t.id.as_str())
        .collect()
}

/// A structural defect in a task dependency graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphError {
    /// Two tasks share the same id
    DuplicateId(String),
    /// A task lists a prerequisite that does not exist
    UnknownDependency { task: String, dependency: String },
    /// The prerequisites form a cycle; the path starts and ends with the same id
    Cycle(Vec<String>),
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateId(id) => write!(f, "duplicate task id `{id}`"),
            Self::UnknownDependency { task, dependency } => {
                write!(f, "task `{task}` is blocked by unknown task `{dependency}`")
            }
            Self::Cycle(path) => write!(f, "dependency cycle: {}", path.join(" -> ")),
        }
    }
}

impl std::error::Error for GraphError {}

/// Validates a dependency graph given as `(id, blocked_by)` pairs: ids must be
/// unique, every prerequisite must exist, and the graph must be acyclic.
pub fn check_dependencies<'a, I>(nodes: I) -> Result<(), GraphError>
where
    I: IntoIterator<Item = (&'a str, &'a [String])>,
{
    let mut edges: HashMap<&str, &[String]> = HashMap::new();
    let mut order = Vec::new();
    for (id, blocked_by) in nodes {
        if edges.insert(id, blocked_by).is_some() {
            return Err(GraphError::DuplicateId(id.to_string()));
        }
        order.push(id);
    }
    for &id in &order {
        if let Some(dep) = edges[id].iter().find(|d| !edges.contains_key(d.as_str())) {
            return Err(GraphError::UnknownDependency {
                task: id.to_string(),
                dependency: dep.clone(),
            });
        }
    }

    // Iterative DFS; `path` holds the current chain of unfinished nodes.
    let mut finished: HashSet<&str> = HashSet::new();
    for &root in &order {
        if finished.contains(root) {
            continue;
        }
        let mut path: Vec<&str> = vec![root];
        let mut stack: Vec<std::slice::Iter<'_, String>> = vec![edges[root].iter()];
        while let Some(iter) = stack.last_mut() {
            match iter.next() {
                Some(dep) => {
                    let dep = dep.as_str();
                    if let Some(pos) = path.iter().position(|&p| p == dep) {
                        let mut cycle: Vec<String> =
                            path[pos..].iter().map(|s| s.to_string()).collect();
                        cycle.push(dep.to_string());
                        return Err(GraphError::Cycle(cycle));
                    }
                    if !finished.contains(dep) {
                        path.push(dep);
                        stack.push(edges[dep].iter());
                    }
                }
                None => {
                    stack.pop();
                    finished.insert(path.pop().expect("path tracks stack"));
                }
            }
        }
    }
    Ok(())
}

/// Lifecycle state of a question crossing the airlock.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
#[value(rename_all = "snake_case")]
pub enum QuestionStatus {
    /// Awaiting an answer
    Open,
    /// Answered; the answer may still be revised
    Answered,
    /// Closed, optionally codified in a spec file
    Resolved,
}

impl QuestionStatus {
    /// The snake_case name used in JSON, the database, and CLI filters.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Answered => "answered",
            Self::Resolved => "resolved",
        }
    }
}

/// A behavioral question asked across the airlock.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Question {
    /// Numeric identifier, assigned by the hub store
    pub id: u64,
    /// Spec topic or file name (e.g. "missiles.md", "melee.md")
    pub topic: String,
    /// Session that asked the question
    pub asked_by: String,
    /// The concrete behavioral question
    pub question: String,
    /// Current status
    pub status: QuestionStatus,
    /// Answer in behavioral prose
    pub answer: Option<String>,
    /// Session that answered
    pub answered_by: Option<String>,
    /// Spec file in which the answer was codified
    pub spec_file: Option<String>,
    /// Unix timestamp (seconds) of creation
    pub created_at: i64,
    /// Unix timestamp (seconds) of the latest answer
    pub answered_at: Option<i64>,
    /// Unix timestamp (seconds) of the latest correction of the answer
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corrected_at: Option<i64>,
    /// Session that made the latest correction
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corrected_by: Option<String>,
    /// Earlier answers replaced by corrections, oldest first
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<AnswerVersion>,
}

/// An answer that a correction replaced.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AnswerVersion {
    /// The replaced answer
    pub answer: String,
    /// Session that gave it
    pub answered_by: Option<String>,
    /// Unix timestamp (seconds) when it was given
    pub answered_at: Option<i64>,
    /// Spec file in which it was codified
    pub spec_file: Option<String>,
}

/// What happened, as recorded in a session's inbox (Spec 01 §2.6).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// A task became ready: created ready, unblocked, released, or rejected
    TaskReady,
    /// The priority, track, or work package of a ready task changed
    TaskChanged,
    /// A track's priority changed
    TrackChanged,
    /// A review sent the task back
    TaskRejected,
    /// A task waits for review
    TaskInReview,
    /// A question was asked
    QuestionAsked,
    /// A question was answered or its answer corrected
    QuestionAnswered,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TaskReady => "task_ready",
            Self::TaskChanged => "task_changed",
            Self::TrackChanged => "track_changed",
            Self::TaskRejected => "task_rejected",
            Self::TaskInReview => "task_in_review",
            Self::QuestionAsked => "question_asked",
            Self::QuestionAnswered => "question_answered",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [
            Self::TaskReady,
            Self::TaskChanged,
            Self::TrackChanged,
            Self::TaskRejected,
            Self::TaskInReview,
            Self::QuestionAsked,
            Self::QuestionAnswered,
        ]
        .into_iter()
        .find(|k| k.as_str() == s)
    }
}

/// What changed for a role since a session last looked.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Inbox {
    /// Oldest first; one item per task, question, or track (its latest event)
    pub items: Vec<InboxItem>,
}

/// One inbox entry: the event and what the role may read about its subject.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InboxItem {
    pub kind: EventKind,
    /// Unix timestamp (seconds) of the event
    pub at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<InboxTask>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<InboxQuestion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<InboxTrack>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InboxTask {
    pub id: String,
    pub title: String,
    pub priority: Priority,
    pub track: Option<String>,
    pub status: TaskStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InboxQuestion {
    pub id: u64,
    pub topic: String,
    /// The question, for coordinating roles (`question_asked`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    /// The current answer (`question_answered`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_file: Option<String>,
    /// The answer replaced an earlier one
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub corrected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InboxTrack {
    pub name: String,
    pub priority: Priority,
}
