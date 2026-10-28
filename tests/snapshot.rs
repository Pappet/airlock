//! Canonical snapshot format (Spec 05 §3): the exact bytes are a contract.

mod common;

use airlock::config::ProjectConfig;
use airlock::core::{QuestionStatus, TaskState};
use airlock::snapshot::Snapshot;
use common::{question, task};

const GOLDEN: &str = r#"{
  "format_version": 1,
  "project_id": "6f1c2a3e-0000-4000-8000-000000000001",
  "roles": {
    "engine": {
      "description": "Clean-room engine implementation",
      "clean_room": true
    },
    "lead": {
      "description": "System architecture and coordination",
      "clean_room": false
    },
    "spec": {
      "description": "Behavioral specification authoring",
      "clean_room": false
    }
  },
  "tasks": [
    {
      "id": "E-1",
      "title": "Task E-1",
      "role": "engine",
      "state": "open",
      "priority": 0,
      "blocked_by": [
        "L-1",
        "L-2"
      ],
      "read": [
        "docs/spec/missiles.md"
      ],
      "touch": [
        "src/missiles.rs"
      ],
      "do": "Implement shot clock",
      "verify": "cargo test missiles",
      "notes": null,
      "created_at": 1700000000
    },
    {
      "id": "L-1",
      "title": "Task L-1",
      "role": "engine",
      "state": "done",
      "priority": 0,
      "blocked_by": [],
      "read": [
        "docs/spec/missiles.md"
      ],
      "touch": [
        "src/missiles.rs"
      ],
      "do": "Implement shot clock",
      "verify": "cargo test missiles",
      "notes": null,
      "created_at": 1700000000
    },
    {
      "id": "L-2",
      "title": "Task L-2",
      "role": "engine",
      "state": "in_review",
      "priority": 0,
      "blocked_by": [],
      "read": [
        "docs/spec/missiles.md"
      ],
      "touch": [
        "src/missiles.rs"
      ],
      "do": "Implement shot clock",
      "verify": "cargo test missiles",
      "notes": null,
      "created_at": 1700000000
    }
  ],
  "questions": [
    {
      "id": 1,
      "topic": "melee.md",
      "asked_by": "engine-1",
      "question": "How is the front arc calculated?",
      "status": "answered",
      "answer": "Truncated towards zero.",
      "answered_by": "lead",
      "spec_file": "docs/spec/melee.md",
      "created_at": 1700000100,
      "answered_at": 1700000200
    },
    {
      "id": 2,
      "topic": "melee.md",
      "asked_by": "engine-1",
      "question": "How is the front arc calculated?",
      "status": "open",
      "answer": null,
      "answered_by": null,
      "spec_file": null,
      "created_at": 1700000100,
      "answered_at": null
    }
  ]
}
"#;

const PROJECT_ID: &str = "6f1c2a3e-0000-4000-8000-000000000001";

fn sample() -> Snapshot {
    // Live state is deliberately unsorted and carries session state.
    let mut claimed = task("E-1", TaskState::InProgress, &["L-2", "L-1", "L-2"]);
    claimed.claimed_by = Some("engine-1".into());
    claimed.lease_expires_at = Some(1_700_003_600);
    claimed.updated_at = 1_700_001_234;
    let tasks = [
        task("L-2", TaskState::InReview, &[]),
        claimed,
        task("L-1", TaskState::Done, &[]),
    ];

    let mut answered = question(1);
    answered.status = QuestionStatus::Answered;
    answered.answer = Some("Truncated towards zero.".into());
    answered.answered_by = Some("lead".into());
    answered.spec_file = Some("docs/spec/melee.md".into());
    answered.answered_at = Some(1_700_000_200);

    Snapshot::new(
        PROJECT_ID,
        &ProjectConfig::default(),
        &tasks,
        &[question(2), answered],
    )
}

#[test]
fn canonical_json_matches_golden_file() {
    assert_eq!(
        sample().to_canonical_json().expect("must serialize"),
        GOLDEN
    );
}

#[test]
fn session_state_does_not_change_the_snapshot() {
    let mut unclaimed = task("E-1", TaskState::Open, &["L-1", "L-2"]);
    unclaimed.updated_at = 1;
    let tasks = [
        task("L-1", TaskState::Done, &[]),
        task("L-2", TaskState::InReview, &[]),
        unclaimed,
    ];
    let snapshot = Snapshot::new(PROJECT_ID, &ProjectConfig::default(), &tasks, &[]);
    assert_eq!(snapshot.tasks, sample().tasks);
}

#[test]
fn golden_file_round_trips() {
    let parsed: Snapshot = serde_json::from_str(GOLDEN).expect("must parse");
    assert_eq!(parsed, sample());
    assert_eq!(parsed.validate(), Ok(()));
}

#[test]
fn validate_rejects_unknown_prerequisites() {
    let snapshot = Snapshot::new(
        PROJECT_ID,
        &ProjectConfig::default(),
        &[task("E-1", TaskState::Open, &["L-9"])],
        &[],
    );
    assert!(snapshot.validate().is_err());
}
