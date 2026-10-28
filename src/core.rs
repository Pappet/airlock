//! Core data models for tasks, questions, and session leases.

use serde::{Deserialize, Serialize};

/// Task status in the workflow.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Task is defined but blocked by incomplete dependencies
    Blocked,
    /// Task dependencies are satisfied; ready to be claimed
    Ready,
    /// An agent session has claimed and is currently working on the task
    InProgress,
    /// Task is completed and awaiting review or verification
    InReview,
    /// Task is completed and verified
    Done,
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
    /// Current status
    pub status: TaskStatus,
    /// Prerequisites that must be 'Done' before this task is ready
    #[serde(default)]
    pub blocked_by: Vec<String>,
    /// Tasks unblocked when this task is completed
    #[serde(default)]
    pub unblocks: Vec<String>,
    /// Exact documentation / spec files to read
    #[serde(default)]
    pub read: Vec<String>,
    /// Source files to create or modify
    #[serde(default)]
    pub touch: Vec<String>,
    /// Concrete instructions describing the behavior to implement
    pub r#do: String,
    /// Verification command or test scenario
    pub verify: String,
    /// Optional completion notes or engine status notes
    pub notes: Option<String>,
    /// Session currently holding the lease, if in progress
    pub claimed_by: Option<String>,
}

/// Status of an open question across the airlock.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuestionStatus {
    /// Awaiting answer
    Open,
    /// Answered by lead/spec session
    Answered,
    /// Resolved and integrated into target specification
    Resolved,
}

/// An airlock question between sessions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Question {
    /// Numeric identifier
    pub id: u64,
    /// Spec topic or file name (e.g. "missiles.md", "melee.md")
    pub topic: String,
    /// Session identifier that asked the question
    pub asked_by: String,
    /// The concrete behavioral question
    pub question: String,
    /// Current status
    pub status: QuestionStatus,
    /// Answer provided by lead/spec author
    pub answer: Option<String>,
    /// Optional answer author
    pub answered_by: Option<String>,
}
