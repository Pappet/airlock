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
    /// Higher values are handed out first by `task next`
    #[serde(default)]
    pub priority: i32,
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
}
