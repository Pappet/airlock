//! Hub store: transitions, leases, validation, concurrency (Spec 01 §2, §3.2).

use airlock::core::{QuestionStatus, TaskState};
use airlock::store::{NewTask, NotFound, Store, TaskEdit};
use std::sync::{Arc, Barrier};
use tempfile::TempDir;

const MIN: i64 = 60;

fn open() -> (TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("airlock.db")).expect("open store");
    (dir, store)
}

fn new_task(blocked_by: &[&str]) -> NewTask {
    NewTask {
        prefix: "T".to_string(),
        title: "Task".to_string(),
        role: "engine".to_string(),
        priority: 0,
        blocked_by: blocked_by.iter().map(|s| s.to_string()).collect(),
        read: vec!["docs/spec/a.md".to_string()],
        touch: vec!["src/a.rs".to_string()],
        r#do: "Do it".to_string(),
        verify: "cargo test".to_string(),
    }
}

fn err(result: anyhow::Result<impl std::fmt::Debug>) -> String {
    result.expect_err("operation must fail").to_string()
}

#[test]
fn claim_and_complete_unblocks_dependents() {
    let (_dir, mut store) = open();
    store.add_task(new_task(&[]), 0).unwrap();
    store.add_task(new_task(&["T-1"]), 0).unwrap();
    store.add_task(new_task(&["T-1", "T-2"]), 0).unwrap();

    assert!(err(store.claim("T-2", "s1", 60 * MIN, 1)).contains("blocked"));
    let a = store.claim("T-1", "s1", 60 * MIN, 1).unwrap();
    assert_eq!(a.state, TaskState::InProgress);
    assert_eq!(a.claimed_by.as_deref(), Some("s1"));
    assert_eq!(a.lease_expires_at, Some(1 + 60 * MIN));

    assert!(err(store.claim("T-1", "s2", 60 * MIN, 2)).contains("claimed by s1"));
    assert!(err(store.claim("T-1", "s1", 60 * MIN, 2)).contains("heartbeat"));
    assert!(err(store.complete("T-1", "s2", None, false, false, 3)).contains("--force"));

    let done = store
        .complete("T-1", "s1", Some("all green"), false, false, 4)
        .unwrap();
    assert_eq!(done.task.state, TaskState::Done);
    assert_eq!(done.task.claimed_by, None);
    assert_eq!(done.task.notes.as_deref(), Some("all green"));
    assert_eq!(done.now_ready, ["T-2"], "C still waits for B");
}

#[test]
fn leases_expire_lazily_and_can_be_renewed() {
    let (_dir, mut store) = open();
    store.add_task(new_task(&[]), 0).unwrap();
    store.claim("T-1", "s1", 10 * MIN, 0).unwrap();

    store.heartbeat("T-1", "s1", 10 * MIN, 5 * MIN).unwrap();
    assert_eq!(
        store.task("T-1", 14 * MIN).unwrap().state,
        TaskState::InProgress
    );
    assert!(err(store.heartbeat("T-1", "s2", 10 * MIN, 6 * MIN)).contains("claimed by s1"));

    // Expired at 15 min: reads show it released, before any write happens.
    let expired = store.task("T-1", 15 * MIN).unwrap();
    assert_eq!(expired.state, TaskState::Open);
    assert_eq!(expired.claimed_by, None);

    assert!(err(store.heartbeat("T-1", "s1", 10 * MIN, 16 * MIN)).contains("not claimed"));
    let reclaimed = store.claim("T-1", "s2", 10 * MIN, 16 * MIN).unwrap();
    assert_eq!(reclaimed.claimed_by.as_deref(), Some("s2"));
}

#[test]
fn review_approve_and_reject() {
    let (_dir, mut store) = open();
    store.add_task(new_task(&[]), 0).unwrap();
    store.add_task(new_task(&["T-1"]), 0).unwrap();

    store.claim("T-1", "s1", MIN, 1).unwrap();
    let submitted = store.complete("T-1", "s1", None, true, false, 2).unwrap();
    assert_eq!(submitted.task.state, TaskState::InReview);
    assert!(submitted.now_ready.is_empty());

    let rejected = store.reject("T-1", "missing edge case", 3).unwrap();
    assert_eq!(rejected.state, TaskState::Open);
    assert_eq!(rejected.notes.as_deref(), Some("missing edge case"));
    assert!(err(store.approve("T-1", None, 4)).contains("not in review"));

    store.claim("T-1", "s1", MIN, 5).unwrap();
    store.complete("T-1", "s1", None, true, false, 6).unwrap();
    let approved = store.approve("T-1", Some("lgtm"), 7).unwrap();
    assert_eq!(approved.task.state, TaskState::Done);
    assert_eq!(approved.now_ready, ["T-2"]);
}

#[test]
fn force_overrides_ownership_but_not_dependencies() {
    let (_dir, mut store) = open();
    store.add_task(new_task(&[]), 0).unwrap();
    store.add_task(new_task(&["T-1"]), 0).unwrap();

    assert!(err(store.complete("T-1", "s1", None, false, false, 1)).contains("not claimed"));
    assert!(err(store.complete("T-2", "s1", None, false, true, 1)).contains("blocked"));

    store.claim("T-1", "s1", MIN, 1).unwrap();
    assert!(err(store.release("T-1", "s2", false, 2)).contains("claimed by s1"));
    assert_eq!(
        store.release("T-1", "s2", true, 2).unwrap().state,
        TaskState::Open
    );
    assert!(err(store.release("T-1", "s2", true, 3)).contains("not claimed"));

    let done = store.complete("T-1", "lead", None, false, true, 4).unwrap();
    assert_eq!(done.task.state, TaskState::Done);
    assert!(err(store.complete("T-1", "lead", None, false, true, 5)).contains("already done"));
}

#[test]
fn add_numbers_tasks_per_prefix() {
    let (_dir, mut store) = open();
    assert_eq!(store.add_task(new_task(&[]), 0).unwrap().id, "T-1");
    assert_eq!(store.add_task(new_task(&[]), 0).unwrap().id, "T-2");
    let mut other = new_task(&[]);
    other.prefix = "E".to_string();
    assert_eq!(store.add_task(other, 0).unwrap().id, "E-1", "own sequence");
    assert_eq!(store.add_task(new_task(&[]), 0).unwrap().id, "T-3");
}

#[test]
fn add_continues_after_the_highest_number_including_done_tasks() {
    use airlock::config::ProjectConfig;
    use airlock::snapshot::Snapshot;

    // Ids from before automatic numbering: gaps, done tasks, and odd forms.
    let (_src_dir, mut source) = open();
    for _ in 0..12 {
        source.add_task(new_task(&[]), 0).unwrap();
    }
    source
        .complete("T-12", "lead", None, false, true, 1)
        .unwrap();
    let mut tasks = source.tasks(2).unwrap();
    tasks.retain(|t| ["T-2", "T-12"].contains(&t.id.as_str()));
    let mut odd = tasks[0].clone();
    odd.id = "T-99d".to_string();
    tasks.push(odd);
    let mut lower = tasks[0].clone();
    lower.id = "t-50".to_string();
    tasks.push(lower);
    let snapshot = Snapshot::new("id", &ProjectConfig::default(), &tasks, &[]);

    let (_dir, mut store) = open();
    store.replace_all(&snapshot, "hash", 3).unwrap();
    assert_eq!(store.task("T-12", 3).unwrap().state, TaskState::Done);
    let next = store.add_task(new_task(&[]), 4).unwrap();
    assert_eq!(
        next.id, "T-13",
        "`T-99d` and `t-50` are not in the sequence"
    );
}

#[test]
fn concurrent_adds_get_distinct_ids() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("airlock.db");
    Store::open(&path).unwrap();

    let sessions = 8;
    let barrier = Arc::new(Barrier::new(sessions));
    let handles: Vec<_> = (0..sessions)
        .map(|_| {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let mut store = Store::open(&path).unwrap();
                barrier.wait();
                store.add_task(new_task(&[]), 0).unwrap().id
            })
        })
        .collect();
    let mut ids: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    ids.sort_by_key(|id| id[2..].parse::<u32>().unwrap());
    let expected: Vec<String> = (1..=sessions).map(|n| format!("T-{n}")).collect();
    assert_eq!(ids, expected);
}

#[test]
fn add_validates_prefix_and_prerequisites() {
    let (_dir, mut store) = open();
    store.add_task(new_task(&[]), 0).unwrap();

    assert!(err(store.add_task(new_task(&["Z"]), 0)).contains("`Z` does not exist"));
    for bad in ["", "E-", "E 1", "É"] {
        let mut spec = new_task(&[]);
        spec.prefix = bad.to_string();
        assert!(
            err(store.add_task(spec, 0)).contains("invalid task prefix"),
            "{bad:?}"
        );
    }

    let b = store.add_task(new_task(&["T-1", "T-1"]), 0).unwrap();
    assert_eq!(b.id, "T-2", "failed adds use no number");
    assert_eq!(b.blocked_by, ["T-1"]);
}

#[test]
fn tasks_round_trip_through_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("airlock.db");
    let mut store = Store::open(&path).unwrap();
    store.add_task(new_task(&[]), 0).unwrap();
    store.add_task(new_task(&[]), 0).unwrap();
    let mut spec = new_task(&["T-2", "T-1"]);
    spec.priority = -3;
    let added = store.add_task(spec, 42).unwrap();
    drop(store);

    let store = Store::open(&path).unwrap();
    let ids: Vec<String> = store.tasks(42).unwrap().into_iter().map(|t| t.id).collect();
    assert_eq!(ids, ["T-1", "T-2", "T-3"]);
    let c = store.task("T-3", 42).unwrap();
    assert_eq!(c, added);
    assert_eq!(c.blocked_by, ["T-2", "T-1"], "authored order preserved");
    assert_eq!((c.priority, c.created_at, c.updated_at), (-3, 42, 42));
}

#[test]
fn question_lifecycle() {
    let (_dir, mut store) = open();
    let q1 = store.ask("melee.md", "Front arc?", "engine-1", 10).unwrap();
    let q2 = store
        .ask("missiles.md", "Blast radius?", "engine-1", 11)
        .unwrap();
    assert_eq!((q1.id, q2.id), (1, 2));
    assert_eq!(q1.status, QuestionStatus::Open);

    assert!(err(store.resolve(1, None, 12)).contains("no answer yet"));
    let answered = store
        .answer(1, "Truncated.", "lead", Some("docs/spec/melee.md"), 13)
        .unwrap();
    assert_eq!(answered.status, QuestionStatus::Answered);
    assert_eq!(answered.answered_at, Some(13));

    let revised = store
        .answer(1, "Truncated towards zero.", "lead", None, 14)
        .unwrap();
    assert_eq!(revised.answer.as_deref(), Some("Truncated towards zero."));
    assert_eq!(
        revised.spec_file.as_deref(),
        Some("docs/spec/melee.md"),
        "spec kept"
    );

    let resolved = store.resolve(1, None, 15).unwrap();
    assert_eq!(resolved.status, QuestionStatus::Resolved);
    assert!(err(store.answer(1, "late", "lead", None, 16)).contains("already resolved"));
    assert!(err(store.question(9).map(|_| ())).contains("no question #9"));

    let ids: Vec<u64> = store.questions().unwrap().iter().map(|q| q.id).collect();
    assert_eq!(ids, [2, 1], "newest first");
}

#[test]
fn concurrent_claims_from_separate_connections_have_one_winner() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("airlock.db");
    Store::open(&path)
        .unwrap()
        .add_task(new_task(&[]), 0)
        .unwrap();

    let sessions = 8;
    let barrier = Arc::new(Barrier::new(sessions));
    let handles: Vec<_> = (0..sessions)
        .map(|i| {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let mut store = Store::open(&path).unwrap();
                barrier.wait();
                store.claim("T-1", &format!("s{i}"), MIN, 1).is_ok()
            })
        })
        .collect();
    let winners = handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .filter(|&won| won)
        .count();
    assert_eq!(winners, 1);
}

#[test]
fn version_1_databases_are_migrated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("airlock.db");
    Store::open(&path).unwrap();
    // Roll back to the version 1 layout.
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("DROP TABLE exports; DROP TABLE bindings; PRAGMA user_version = 1;")
        .unwrap();
    drop(conn);

    let mut store = Store::open(&path).unwrap();
    store.record_export("abc", 1).unwrap();
    assert!(store.is_known_export("abc").unwrap());
    assert!(!store.is_known_export("def").unwrap());
}

#[test]
fn replace_all_swaps_state_and_never_reuses_question_ids() {
    use airlock::config::ProjectConfig;
    use airlock::snapshot::Snapshot;

    let (_dir, mut store) = open();
    store.add_task(new_task(&[]), 0).unwrap();
    for _ in 0..5 {
        store.ask("t", "q", "s", 0).unwrap();
    }

    let (_src_dir, mut source) = open();
    source.add_task(new_task(&[]), 0).unwrap();
    source.add_task(new_task(&["T-1"]), 0).unwrap();
    source.claim("T-1", "s1", MIN, 1).unwrap();
    source.ask("melee.md", "Front arc?", "engine-1", 2).unwrap();
    let snapshot = Snapshot::new(
        "id",
        &ProjectConfig::default(),
        &source.tasks(3).unwrap(),
        &source.questions().unwrap(),
    );

    store.replace_all(&snapshot, "imported-hash", 10).unwrap();
    assert!(store.is_known_export("imported-hash").unwrap());
    let tasks = store.tasks(10).unwrap();
    assert_eq!(
        tasks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        ["T-1", "T-2"]
    );
    assert_eq!(tasks[0].state, TaskState::Open, "claims are not imported");
    assert_eq!(tasks[1].blocked_by, ["T-1"]);
    assert_eq!(store.questions().unwrap()[0].id, 1);

    let next = store.ask("t", "q", "s", 11).unwrap();
    assert_eq!(next.id, 6, "continues after the highest id ever used");
}

#[test]
fn unknown_ids_are_not_found() {
    let (_dir, mut store) = open();
    store.add_task(new_task(&[]), 0).unwrap();
    store.add_task(new_task(&["T-1"]), 0).unwrap();
    let not_found = |e: anyhow::Error| e.downcast_ref::<NotFound>().is_some();

    assert!(not_found(store.task("nope", 0).unwrap_err()));
    assert!(not_found(store.release("nope", "s", true, 0).unwrap_err()));
    assert!(not_found(store.claim("nope", "s", MIN, 0).unwrap_err()));
    assert!(not_found(store.question(99).unwrap_err()));
    let retitle = TaskEdit {
        title: Some("x".into()),
        ..TaskEdit::default()
    };
    assert!(not_found(
        store.edit_task("nope", retitle, false, 0).unwrap_err()
    ));
    assert_eq!(err(store.task("nope", 0)), "no task `nope`");
    // A failed transition on an existing task is not a lookup failure.
    assert!(!not_found(store.claim("T-2", "s", MIN, 0).unwrap_err()));
}

#[test]
fn edit_changes_only_given_fields() {
    let (_dir, mut store) = open();
    store.add_task(new_task(&[]), 0).unwrap();
    store.add_task(new_task(&[]), 0).unwrap();
    let before = store.task("T-2", 0).unwrap();

    let edited = store
        .edit_task(
            "T-2",
            TaskEdit {
                title: Some("New title".into()),
                priority: Some(7),
                touch: Some(vec!["src/b.rs".into(), "src/c.rs".into()]),
                blocked_by: Some(vec!["T-1".into(), "T-1".into()]),
                ..TaskEdit::default()
            },
            false,
            5,
        )
        .unwrap();
    assert_eq!(edited.title, "New title");
    assert_eq!(edited.priority, 7);
    assert_eq!(edited.touch, ["src/b.rs", "src/c.rs"]);
    assert_eq!(edited.blocked_by, ["T-1"], "duplicates dropped");
    assert_eq!(edited.updated_at, 5);
    assert_eq!(
        (
            &edited.role,
            &edited.read,
            &edited.r#do,
            &edited.verify,
            edited.created_at
        ),
        (
            &before.role,
            &before.read,
            &before.r#do,
            &before.verify,
            before.created_at
        ),
        "untouched fields keep their values"
    );
    assert_eq!(store.task("T-2", 5).unwrap(), edited, "persisted");

    let cleared = store
        .edit_task(
            "T-2",
            TaskEdit {
                blocked_by: Some(vec![]),
                read: Some(vec![]),
                ..TaskEdit::default()
            },
            false,
            6,
        )
        .unwrap();
    assert!(cleared.blocked_by.is_empty() && cleared.read.is_empty());
}

#[test]
fn edit_validates_the_dependency_graph() {
    let (_dir, mut store) = open();
    store.add_task(new_task(&[]), 0).unwrap();
    store.add_task(new_task(&["T-1"]), 0).unwrap();
    let deps = |ids: &[&str]| TaskEdit {
        blocked_by: Some(ids.iter().map(|s| s.to_string()).collect()),
        ..TaskEdit::default()
    };

    assert!(
        err(store.edit_task("T-1", deps(&["T-2"]), false, 1))
            .contains("dependency cycle: T-1 -> T-2 -> T-1")
    );
    assert!(err(store.edit_task("T-1", deps(&["T-1"]), false, 1)).contains("dependency cycle"));
    assert!(err(store.edit_task("T-1", deps(&["Z"]), false, 1)).contains("unknown task `Z`"));
    assert!(
        err(store.edit_task("T-1", TaskEdit::default(), false, 1)).contains("nothing to change")
    );
    assert!(
        store.task("T-1", 1).unwrap().blocked_by.is_empty(),
        "refused edits change nothing"
    );
}

#[test]
fn edit_of_started_tasks_needs_force() {
    let (_dir, mut store) = open();
    store.add_task(new_task(&[]), 0).unwrap();
    store.add_task(new_task(&["T-1"]), 0).unwrap();
    let retitle = || TaskEdit {
        title: Some("Changed".into()),
        ..TaskEdit::default()
    };

    // Blocked and ready tasks are open and editable.
    store.edit_task("T-2", retitle(), false, 1).unwrap();
    store.claim("T-1", "s1", 60 * MIN, 1).unwrap();
    assert!(err(store.edit_task("T-1", retitle(), false, 2)).contains("in_progress; use --force"));
    let forced = store.edit_task("T-1", retitle(), true, 2).unwrap();
    assert_eq!(forced.title, "Changed");
    assert_eq!(forced.state, TaskState::InProgress);
    assert_eq!(
        forced.claimed_by.as_deref(),
        Some("s1"),
        "claim and lease survive"
    );
    assert_eq!(forced.lease_expires_at, Some(1 + 60 * MIN));

    store.complete("T-1", "s1", None, true, false, 3).unwrap();
    assert!(err(store.edit_task("T-1", retitle(), false, 4)).contains("in_review"));
    let reviewed = store.edit_task("T-1", retitle(), true, 4).unwrap();
    assert_eq!(reviewed.state, TaskState::InReview);
    store.approve("T-1", None, 5).unwrap();
    assert!(err(store.edit_task("T-1", retitle(), false, 6)).contains("done"));
    let done = store.edit_task("T-1", retitle(), true, 6).unwrap();
    assert_eq!((done.state, done.updated_at), (TaskState::Done, 6));

    // An expired lease counts as released.
    store.claim("T-2", "s2", MIN, 7).unwrap();
    store
        .edit_task("T-2", retitle(), false, 7 + 2 * MIN)
        .unwrap();
}

#[test]
fn ask_once_keeps_one_open_question_per_session_and_topic() {
    let (_dir, mut store) = open();
    let first = store
        .ask_once("overlap:E-2", "Continue?", "engine-1", 1)
        .unwrap();
    let again = store
        .ask_once("overlap:E-2", "Continue now?", "engine-1", 2)
        .unwrap();
    assert_eq!(again.id, first.id, "the open question is reused");
    assert_eq!(again.question, "Continue?");

    let other = store
        .ask_once("overlap:E-2", "Continue?", "engine-2", 3)
        .unwrap();
    assert_ne!(other.id, first.id, "another session gets its own question");

    store
        .answer(first.id, "Go ahead.", "lead-1", None, 4)
        .unwrap();
    let after_answer = store
        .ask_once("overlap:E-2", "Continue?", "engine-1", 5)
        .unwrap();
    assert_ne!(
        after_answer.id, first.id,
        "an answered question does not swallow a new claim"
    );
    assert_eq!(store.questions().unwrap().len(), 3);
}
