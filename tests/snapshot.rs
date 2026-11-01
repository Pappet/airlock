//! Canonical snapshot format (Spec 05 §3): the exact bytes are a contract.

mod common;

use airlock::config::ProjectConfig;
use airlock::core::{AnswerVersion, Priority, QuestionStatus, TaskState, Track};
use airlock::snapshot::Snapshot;
use airlock::store::Store;
use airlock::sync::check_import;
use common::{question, task};

const GOLDEN: &str = r#"{
  "format_version": 3,
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
  "tracks": [
    {
      "name": "comfort",
      "priority": "low",
      "description": "",
      "created_at": 1700000010
    },
    {
      "name": "core",
      "priority": "high",
      "description": "Game core",
      "created_at": 1700000020
    }
  ],
  "tasks": [
    {
      "id": "E-1",
      "title": "Task E-1",
      "role": "engine",
      "state": "open",
      "priority": "normal",
      "track": "core",
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
      "priority": "normal",
      "track": null,
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
      "priority": "normal",
      "track": null,
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

fn track(name: &str, priority: Priority, description: &str, created_at: i64) -> Track {
    Track {
        name: name.into(),
        priority,
        description: description.into(),
        created_at,
        updated_at: created_at + 999,
    }
}

const PROJECT_ID: &str = "6f1c2a3e-0000-4000-8000-000000000001";

fn sample() -> Snapshot {
    // Live state is deliberately unsorted and carries session state.
    let mut claimed = task("E-1", TaskState::InProgress, &["L-2", "L-1", "L-2"]);
    claimed.claimed_by = Some("engine-1".into());
    claimed.lease_expires_at = Some(1_700_003_600);
    claimed.updated_at = 1_700_001_234;
    claimed.track = Some("core".into());
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

    // Live tracks are unsorted and carry `updated_at`, which is not exported.
    let tracks = [
        track("core", Priority::High, "Game core", 1_700_000_020),
        track("comfort", Priority::Low, "", 1_700_000_010),
    ];

    Snapshot::new(
        PROJECT_ID,
        &ProjectConfig::default(),
        &tasks,
        &tracks,
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
    unclaimed.track = Some("core".into());
    let tasks = [
        task("L-1", TaskState::Done, &[]),
        task("L-2", TaskState::InReview, &[]),
        unclaimed,
    ];
    let snapshot = Snapshot::new(PROJECT_ID, &ProjectConfig::default(), &tasks, &[], &[]);
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
        &[],
    );
    assert!(snapshot.validate().is_err());
}

/// Priorities on both sides of each migration boundary; shared with the store
/// migration test so both encodings of the mapping see identical inputs.
const LEGACY_PRIORITIES: [(i64, Priority); 7] = [
    (40, Priority::High),
    (30, Priority::High),
    (29, Priority::Normal),
    (22, Priority::Normal),
    (10, Priority::Normal),
    (9, Priority::Low),
    (5, Priority::Low),
];

fn v1_json(priorities: &[i64]) -> String {
    let tasks: Vec<String> = priorities
        .iter()
        .enumerate()
        .map(|(i, p)| {
            format!(
                r#"{{
      "id": "T-{}", "title": "t", "role": "engine", "state": "open", "priority": {p},
      "blocked_by": [], "read": [], "touch": [], "do": "d", "verify": "v",
      "notes": null, "created_at": 1700000000
    }}"#,
                i + 1
            )
        })
        .collect();
    format!(
        r#"{{
  "format_version": 1,
  "project_id": "{PROJECT_ID}",
  "roles": {{ "engine": {{ "description": "e", "clean_room": true }} }},
  "tasks": [{}],
  "questions": []
}}
"#,
        tasks.join(",")
    )
}

fn load(json: &str) -> anyhow::Result<airlock::sync::LoadedSnapshot> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    std::fs::write(&path, json).unwrap();
    airlock::sync::load_snapshot(&path)
}

fn open_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("airlock.db")).unwrap();
    (dir, store)
}

#[test]
fn version_1_snapshots_import_with_converted_priorities() {
    let priorities: Vec<i64> = LEGACY_PRIORITIES.iter().map(|(p, _)| *p).collect();
    let loaded = load(&v1_json(&priorities)).expect("v1 must load");
    assert!(loaded.snapshot.tracks.is_empty());
    assert!(loaded.snapshot.tasks.iter().all(|t| t.track.is_none()));
    check_import(&loaded.snapshot, PROJECT_ID).expect("v1 must pass the import checks");

    let (_dir, mut store) = open_store();
    store
        .replace_all(&loaded.snapshot, &loaded.sha256, 10)
        .unwrap();
    let imported: Vec<Priority> = store
        .tasks(10)
        .unwrap()
        .iter()
        .map(|t| t.priority)
        .collect();
    let expected: Vec<Priority> = LEGACY_PRIORITIES.iter().map(|(_, p)| *p).collect();
    assert_eq!(imported, expected);
    assert!(store.tracks().unwrap().is_empty());

    // The next export is the current format.
    let tasks = store.tasks(10).unwrap();
    let exported = Snapshot::new(PROJECT_ID, &ProjectConfig::default(), &tasks, &[], &[]);
    assert!(
        exported
            .to_canonical_json()
            .unwrap()
            .contains("\"format_version\": 3")
    );
}

#[test]
fn unsupported_versions_are_rejected_naming_the_supported_ones() {
    let json = v1_json(&[1]).replace("\"format_version\": 1", "\"format_version\": 4");
    let message = format!("{:#}", load(&json).err().expect("v4 must not load"));
    assert!(
        message.contains("format version 4 is not supported"),
        "{message}"
    );
    assert!(message.contains("1, 2, 3"), "{message}");
}

#[test]
fn check_import_validates_tracks() {
    let mut snapshot = sample();
    snapshot.roles = ProjectConfig::default().roles;
    check_import(&snapshot, PROJECT_ID).expect("sample is importable");

    let mut undefined = snapshot.clone();
    undefined.tasks[0].track = Some("x".into());
    let message = check_import(&undefined, PROJECT_ID)
        .unwrap_err()
        .to_string();
    assert_eq!(
        message,
        "task `E-1` has track `x`, which the snapshot does not define"
    );

    let mut duplicate = snapshot.clone();
    duplicate.tracks.push(duplicate.tracks[0].clone());
    let message = check_import(&duplicate, PROJECT_ID)
        .unwrap_err()
        .to_string();
    assert!(message.contains("duplicate track `comfort`"), "{message}");

    let mut bad_name = snapshot;
    bad_name.tracks[0].name = "not valid".into();
    assert!(check_import(&bad_name, PROJECT_ID).is_err());
}

#[test]
fn tracks_round_trip_through_the_store() {
    let (_dir, mut source) = open_store();
    source
        .add_track("core", Priority::High, "Game core", 5)
        .unwrap();
    source.add_track("comfort", Priority::Low, "", 6).unwrap();
    let mut new = airlock::store::NewTask {
        prefix: "T".into(),
        title: "t".into(),
        role: "engine".into(),
        priority: Priority::Critical,
        track: Some("core".into()),
        blocked_by: vec![],
        read: vec![],
        touch: vec![],
        r#do: "d".into(),
        verify: "v".into(),
    };
    source.add_task(new.clone(), 7).unwrap();
    new.track = None;
    source.add_task(new, 8).unwrap();
    let snapshot = Snapshot::new(
        PROJECT_ID,
        &ProjectConfig::default(),
        &source.tasks(9).unwrap(),
        &source.tracks().unwrap(),
        &[],
    );
    let names: Vec<&str> = snapshot.tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["comfort", "core"], "sorted by name");

    let (_dir2, mut target) = open_store();
    target.add_track("stale", Priority::Normal, "", 1).unwrap();
    target.replace_all(&snapshot, "h", 20).unwrap();

    let tracks = target.tracks().unwrap();
    assert_eq!(
        tracks.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        ["core", "comfort"],
        "the stale track is gone"
    );
    assert_eq!(tracks[0].priority, Priority::High);
    assert_eq!(tracks[0].description, "Game core");
    assert_eq!((tracks[0].created_at, tracks[0].updated_at), (5, 20));
    let tasks = target.tasks(20).unwrap();
    assert_eq!(tasks[0].track.as_deref(), Some("core"));
    assert_eq!(tasks[1].track, None);

    // Importing again over tasks that use tracks works (tasks go before tracks).
    target.replace_all(&snapshot, "h", 21).unwrap();
}

fn corrected_question() -> airlock::core::Question {
    let mut q = question(3);
    q.status = QuestionStatus::Resolved;
    q.answer = Some("second".into());
    q.answered_by = Some("lead-1".into());
    q.answered_at = Some(1_700_000_400);
    q.corrected_at = Some(1_700_000_400);
    q.corrected_by = Some("lead-1".into());
    q.history = vec![
        AnswerVersion {
            answer: "zeroth".into(),
            answered_by: None,
            answered_at: None,
            spec_file: None,
        },
        AnswerVersion {
            answer: "first".into(),
            answered_by: Some("lead".into()),
            answered_at: Some(1_700_000_200),
            spec_file: Some("docs/spec/melee.md".into()),
        },
    ];
    q
}

#[test]
fn snapshot_round_trips_answer_history() {
    let original = corrected_question();
    let snapshot = Snapshot::new(
        PROJECT_ID,
        &ProjectConfig::default(),
        &[],
        &[],
        std::slice::from_ref(&original),
    );
    let json = snapshot.to_canonical_json().unwrap();
    assert!(json.contains("\"history\""), "{json}");
    let loaded = load(&json).expect("v3 must load");
    assert_eq!(loaded.snapshot, snapshot);

    let (_dir, mut store) = open_store();
    store
        .replace_all(&loaded.snapshot, &loaded.sha256, 10)
        .unwrap();
    assert_eq!(store.questions().unwrap(), [original]);
    let again = Snapshot::new(
        PROJECT_ID,
        &ProjectConfig::default(),
        &[],
        &[],
        &store.questions().unwrap(),
    );
    assert_eq!(again.to_canonical_json().unwrap(), json, "export is stable");
}

#[test]
fn version_two_snapshot_imports_without_history() {
    let json = GOLDEN.replace("\"format_version\": 3", "\"format_version\": 2");
    let loaded = load(&json).expect("v2 must load");
    assert_eq!(loaded.snapshot.format_version, 3);
    assert!(
        loaded
            .snapshot
            .questions
            .iter()
            .all(|q| q.history.is_empty() && q.corrected_at.is_none() && q.corrected_by.is_none())
    );
    check_import(&loaded.snapshot, PROJECT_ID).expect("v2 must pass the import checks");

    let (_dir, mut store) = open_store();
    store
        .replace_all(&loaded.snapshot, &loaded.sha256, 10)
        .unwrap();
    assert!(
        store
            .questions()
            .unwrap()
            .iter()
            .all(|q| q.history.is_empty())
    );
}
