//! Hub store: transitions, leases, validation, concurrency (Spec 01 §2, §3.2).

use airlock::core::{QuestionStatus, TaskState};
use airlock::store::{NewTask, Store};
use std::sync::{Arc, Barrier};
use tempfile::TempDir;

const MIN: i64 = 60;

fn open() -> (TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("airlock.db")).expect("open store");
    (dir, store)
}

fn new_task(id: &str, blocked_by: &[&str]) -> NewTask {
    NewTask {
        id: id.to_string(),
        title: format!("Task {id}"),
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
    store.add_task(new_task("A", &[]), 0).unwrap();
    store.add_task(new_task("B", &["A"]), 0).unwrap();
    store.add_task(new_task("C", &["A", "B"]), 0).unwrap();

    assert!(err(store.claim("B", "s1", 60 * MIN, 1)).contains("blocked"));
    let a = store.claim("A", "s1", 60 * MIN, 1).unwrap();
    assert_eq!(a.state, TaskState::InProgress);
    assert_eq!(a.claimed_by.as_deref(), Some("s1"));
    assert_eq!(a.lease_expires_at, Some(1 + 60 * MIN));

    assert!(err(store.claim("A", "s2", 60 * MIN, 2)).contains("claimed by s1"));
    assert!(err(store.claim("A", "s1", 60 * MIN, 2)).contains("heartbeat"));
    assert!(err(store.complete("A", "s2", None, false, false, 3)).contains("--force"));

    let done = store
        .complete("A", "s1", Some("all green"), false, false, 4)
        .unwrap();
    assert_eq!(done.task.state, TaskState::Done);
    assert_eq!(done.task.claimed_by, None);
    assert_eq!(done.task.notes.as_deref(), Some("all green"));
    assert_eq!(done.now_ready, ["B"], "C still waits for B");
}

#[test]
fn leases_expire_lazily_and_can_be_renewed() {
    let (_dir, mut store) = open();
    store.add_task(new_task("A", &[]), 0).unwrap();
    store.claim("A", "s1", 10 * MIN, 0).unwrap();

    store.heartbeat("A", "s1", 10 * MIN, 5 * MIN).unwrap();
    assert_eq!(
        store.task("A", 14 * MIN).unwrap().state,
        TaskState::InProgress
    );
    assert!(err(store.heartbeat("A", "s2", 10 * MIN, 6 * MIN)).contains("claimed by s1"));

    // Expired at 15 min: reads show it released, before any write happens.
    let expired = store.task("A", 15 * MIN).unwrap();
    assert_eq!(expired.state, TaskState::Open);
    assert_eq!(expired.claimed_by, None);

    assert!(err(store.heartbeat("A", "s1", 10 * MIN, 16 * MIN)).contains("not claimed"));
    let reclaimed = store.claim("A", "s2", 10 * MIN, 16 * MIN).unwrap();
    assert_eq!(reclaimed.claimed_by.as_deref(), Some("s2"));
}

#[test]
fn review_approve_and_reject() {
    let (_dir, mut store) = open();
    store.add_task(new_task("A", &[]), 0).unwrap();
    store.add_task(new_task("B", &["A"]), 0).unwrap();

    store.claim("A", "s1", MIN, 1).unwrap();
    let submitted = store.complete("A", "s1", None, true, false, 2).unwrap();
    assert_eq!(submitted.task.state, TaskState::InReview);
    assert!(submitted.now_ready.is_empty());

    let rejected = store.reject("A", "missing edge case", 3).unwrap();
    assert_eq!(rejected.state, TaskState::Open);
    assert_eq!(rejected.notes.as_deref(), Some("missing edge case"));
    assert!(err(store.approve("A", None, 4)).contains("not in review"));

    store.claim("A", "s1", MIN, 5).unwrap();
    store.complete("A", "s1", None, true, false, 6).unwrap();
    let approved = store.approve("A", Some("lgtm"), 7).unwrap();
    assert_eq!(approved.task.state, TaskState::Done);
    assert_eq!(approved.now_ready, ["B"]);
}

#[test]
fn force_overrides_ownership_but_not_dependencies() {
    let (_dir, mut store) = open();
    store.add_task(new_task("A", &[]), 0).unwrap();
    store.add_task(new_task("B", &["A"]), 0).unwrap();

    assert!(err(store.complete("A", "s1", None, false, false, 1)).contains("not claimed"));
    assert!(err(store.complete("B", "s1", None, false, true, 1)).contains("blocked"));

    store.claim("A", "s1", MIN, 1).unwrap();
    assert!(err(store.release("A", "s2", false, 2)).contains("claimed by s1"));
    assert_eq!(
        store.release("A", "s2", true, 2).unwrap().state,
        TaskState::Open
    );
    assert!(err(store.release("A", "s2", true, 3)).contains("not claimed"));

    let done = store.complete("A", "lead", None, false, true, 4).unwrap();
    assert_eq!(done.task.state, TaskState::Done);
    assert!(err(store.complete("A", "lead", None, false, true, 5)).contains("already done"));
}

#[test]
fn add_validates_ids_and_prerequisites() {
    let (_dir, mut store) = open();
    store.add_task(new_task("A", &[]), 0).unwrap();

    assert!(err(store.add_task(new_task("A", &[]), 0)).contains("already exists"));
    assert!(err(store.add_task(new_task("B", &["Z"]), 0)).contains("`Z` does not exist"));
    assert!(err(store.add_task(new_task("B C", &[]), 0)).contains("invalid task id"));
    assert!(err(store.add_task(new_task("", &[]), 0)).contains("invalid task id"));

    let b = store.add_task(new_task("B", &["A", "A"]), 0).unwrap();
    assert_eq!(b.blocked_by, ["A"]);
}

#[test]
fn tasks_round_trip_through_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("airlock.db");
    let mut store = Store::open(&path).unwrap();
    store.add_task(new_task("A", &[]), 0).unwrap();
    store.add_task(new_task("C", &[]), 0).unwrap();
    let mut spec = new_task("B", &["C", "A"]);
    spec.priority = -3;
    let added = store.add_task(spec, 42).unwrap();
    drop(store);

    let store = Store::open(&path).unwrap();
    let ids: Vec<String> = store.tasks(42).unwrap().into_iter().map(|t| t.id).collect();
    assert_eq!(ids, ["A", "B", "C"]);
    let b = store.task("B", 42).unwrap();
    assert_eq!(b, added);
    assert_eq!(b.blocked_by, ["C", "A"], "authored order preserved");
    assert_eq!((b.priority, b.created_at, b.updated_at), (-3, 42, 42));
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
        .add_task(new_task("A", &[]), 0)
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
                store.claim("A", &format!("s{i}"), MIN, 1).is_ok()
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
    conn.execute_batch("DROP TABLE exports; PRAGMA user_version = 1;")
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
    store.add_task(new_task("OLD", &[]), 0).unwrap();
    for _ in 0..5 {
        store.ask("t", "q", "s", 0).unwrap();
    }

    let (_src_dir, mut source) = open();
    source.add_task(new_task("A", &[]), 0).unwrap();
    source.add_task(new_task("B", &["A"]), 0).unwrap();
    source.claim("A", "s1", MIN, 1).unwrap();
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
        ["A", "B"]
    );
    assert_eq!(tasks[0].state, TaskState::Open, "claims are not imported");
    assert_eq!(tasks[1].blocked_by, ["A"]);
    assert_eq!(store.questions().unwrap()[0].id, 1);

    let next = store.ask("t", "q", "s", 11).unwrap();
    assert_eq!(next.id, 6, "continues after the highest id ever used");
}
