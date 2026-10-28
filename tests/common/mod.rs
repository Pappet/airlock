//! Shared fixtures for integration tests.
// Each test binary uses a different subset of these fixtures.
#![allow(dead_code)]

use airlock::core::{Question, QuestionStatus, Task, TaskState};

pub fn task(id: &str, state: TaskState, blocked_by: &[&str]) -> Task {
    Task {
        id: id.to_string(),
        title: format!("Task {id}"),
        role: "engine".to_string(),
        state,
        priority: 0,
        blocked_by: blocked_by.iter().map(|s| s.to_string()).collect(),
        read: vec!["docs/spec/missiles.md".to_string()],
        touch: vec!["src/missiles.rs".to_string()],
        r#do: "Implement shot clock".to_string(),
        verify: "cargo test missiles".to_string(),
        notes: None,
        claimed_by: None,
        lease_expires_at: None,
        created_at: 1_700_000_000,
        updated_at: 1_700_000_000,
    }
}

pub fn question(id: u64) -> Question {
    Question {
        id,
        topic: "melee.md".to_string(),
        asked_by: "engine-1".to_string(),
        question: "How is the front arc calculated?".to_string(),
        status: QuestionStatus::Open,
        answer: None,
        answered_by: None,
        spec_file: None,
        created_at: 1_700_000_100,
        answered_at: None,
    }
}
