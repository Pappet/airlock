//! Canonical JSON state snapshot (`docs/.airlock-state.json`), see Spec 05 §3.
//!
//! The snapshot holds semantic project state only. Session state (claims,
//! leases, `updated_at`) is excluded, so claiming or renewing a task never
//! changes the exported file.

use crate::config::{ProjectConfig, RoleConfig};
use crate::core::{GraphError, Question, Task, TaskState, check_dependencies};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Current snapshot format version.
pub const FORMAT_VERSION: u32 = 1;

/// The canonical, lossless export of one project's state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snapshot {
    /// Snapshot format version
    pub format_version: u32,
    /// Project id, matching the `.airlock` marker
    pub project_id: String,
    /// Role definitions
    pub roles: BTreeMap<String, RoleConfig>,
    /// Tasks, sorted by id
    pub tasks: Vec<TaskRecord>,
    /// Questions, sorted by id
    pub questions: Vec<Question>,
}

/// Exported form of a [`Task`], without session state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskRecord {
    pub id: String,
    pub title: String,
    pub role: String,
    /// Never `in_progress`: claims are session state and export as `open`
    pub state: TaskState,
    pub priority: i32,
    /// Sorted and deduplicated
    pub blocked_by: Vec<String>,
    pub read: Vec<String>,
    pub touch: Vec<String>,
    pub r#do: String,
    pub verify: String,
    pub notes: Option<String>,
    pub created_at: i64,
}

impl From<&Task> for TaskRecord {
    fn from(task: &Task) -> Self {
        let mut blocked_by = task.blocked_by.clone();
        blocked_by.sort();
        blocked_by.dedup();
        Self {
            id: task.id.clone(),
            title: task.title.clone(),
            role: task.role.clone(),
            state: match task.state {
                TaskState::InProgress => TaskState::Open,
                state => state,
            },
            priority: task.priority,
            blocked_by,
            read: task.read.clone(),
            touch: task.touch.clone(),
            r#do: task.r#do.clone(),
            verify: task.verify.clone(),
            notes: task.notes.clone(),
            created_at: task.created_at,
        }
    }
}

impl Snapshot {
    /// Builds a snapshot in canonical order from live state.
    pub fn new(
        project_id: &str,
        config: &ProjectConfig,
        tasks: &[Task],
        questions: &[Question],
    ) -> Self {
        let mut tasks: Vec<TaskRecord> = tasks.iter().map(TaskRecord::from).collect();
        tasks.sort_by(|a, b| a.id.cmp(&b.id));
        let mut questions = questions.to_vec();
        questions.sort_by_key(|q| q.id);

        Self {
            format_version: FORMAT_VERSION,
            project_id: project_id.to_string(),
            roles: config.roles.clone(),
            tasks,
            questions,
        }
    }

    /// Serializes byte-stably: fixed field order, 2-space indent, LF, one trailing newline.
    pub fn to_canonical_json(&self) -> serde_json::Result<String> {
        let mut json = serde_json::to_string_pretty(self)?;
        json.push('\n');
        Ok(json)
    }

    /// Checks the task graph before an import: unique ids, known prerequisites, no cycles.
    pub fn validate(&self) -> Result<(), GraphError> {
        check_dependencies(
            self.tasks
                .iter()
                .map(|t| (t.id.as_str(), t.blocked_by.as_slice())),
        )
    }
}
