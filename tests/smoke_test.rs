//! Smoke test for Airlock data structures and configuration.

use airlock::config::ProjectConfig;
use airlock::core::{Question, QuestionStatus, Task, TaskStatus};

#[test]
fn test_default_config() {
    let cfg = ProjectConfig::default();
    assert_eq!(cfg.version, "0.1.0");
    assert!(cfg.roles.contains_key("lead"));
    assert!(cfg.roles.contains_key("engine"));
    assert!(cfg.roles["engine"].clean_room);
    assert!(!cfg.roles["lead"].clean_room);
}

#[test]
fn test_task_model() {
    let task = Task {
        id: "E-6d".to_string(),
        title: "Missiles implementation".to_string(),
        role: "engine".to_string(),
        status: TaskStatus::Ready,
        blocked_by: vec!["L-5".to_string()],
        unblocks: vec!["E-6e".to_string()],
        read: vec!["docs/spec/missiles.md".to_string()],
        touch: vec!["crates/shr-engine/src/missiles.rs".to_string()],
        r#do: "Implement shot clock and volley firing cycle".to_string(),
        verify: "cargo test -p shr-engine --test bf003_missiles".to_string(),
        notes: None,
        claimed_by: None,
    };

    assert_eq!(task.status, TaskStatus::Ready);
    let serialized = serde_json::to_string(&task).expect("must serialize");
    let deserialized: Task = serde_json::from_str(&serialized).expect("must deserialize");
    assert_eq!(task, deserialized);
}

#[test]
fn test_question_model() {
    let question = Question {
        id: 1,
        topic: "melee.md".to_string(),
        asked_by: "engine-session-1".to_string(),
        question: "How is front arc calculated for irregular units?".to_string(),
        status: QuestionStatus::Open,
        answer: None,
        answered_by: None,
    };

    assert_eq!(question.status, QuestionStatus::Open);
    let serialized = serde_json::to_string(&question).expect("must serialize");
    let deserialized: Question = serde_json::from_str(&serialized).expect("must deserialize");
    assert_eq!(question, deserialized);
}
