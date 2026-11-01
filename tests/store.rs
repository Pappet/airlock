//! Hub store: transitions, leases, validation, concurrency (Spec 01 §2, §3.2).

use airlock::core::{AnswerVersion, Priority, QuestionStatus, TaskState};
use airlock::store::{NewTask, NotFound, Store, TaskEdit, TrackEdit};
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
        priority: Priority::Normal,
        track: None,
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

    store
        .heartbeat("T-1", "s1", 10 * MIN, None, 5 * MIN)
        .unwrap();
    assert_eq!(
        store.task("T-1", 14 * MIN).unwrap().state,
        TaskState::InProgress
    );
    assert!(err(store.heartbeat("T-1", "s2", 10 * MIN, None, 6 * MIN)).contains("claimed by s1"));

    // Expired at 15 min: reads show it released, before any write happens.
    let expired = store.task("T-1", 15 * MIN).unwrap();
    assert_eq!(expired.state, TaskState::Open);
    assert_eq!(expired.claimed_by, None);

    assert!(err(store.heartbeat("T-1", "s1", 10 * MIN, None, 16 * MIN)).contains("not claimed"));
    let reclaimed = store.claim("T-1", "s2", 10 * MIN, 16 * MIN).unwrap();
    assert_eq!(reclaimed.claimed_by.as_deref(), Some("s2"));
}

#[test]
fn heartbeat_records_progress_and_complete_clears_it() {
    let (_dir, mut store) = open();
    store.add_task(new_task(&[]), 0).unwrap();
    store.claim("T-1", "s1", 10 * MIN, 0).unwrap();
    assert_eq!(store.task("T-1", 0).unwrap().progress, None);

    let t = store
        .heartbeat("T-1", "s1", 10 * MIN, Some("parser done; next: tests"), 1)
        .unwrap();
    assert_eq!(t.progress.as_deref(), Some("parser done; next: tests"));
    assert_eq!(t.notes, None, "heartbeats never touch notes");
    let t = store.heartbeat("T-1", "s1", 10 * MIN, None, 2).unwrap();
    assert_eq!(
        t.progress.as_deref(),
        Some("parser done; next: tests"),
        "a heartbeat without notes keeps the progress"
    );
    let t = store
        .heartbeat("T-1", "s1", 10 * MIN, Some("tests half done"), 3)
        .unwrap();
    assert_eq!(
        t.progress.as_deref(),
        Some("tests half done"),
        "replaced, not appended"
    );

    let done = store
        .complete("T-1", "s1", Some("all green"), false, false, 4)
        .unwrap();
    assert_eq!(done.task.progress, None);
    assert_eq!(done.task.notes.as_deref(), Some("all green"));

    // Submitting for review clears it as well; an empty note removes it.
    store.add_task(new_task(&[]), 5).unwrap();
    store.claim("T-2", "s1", 10 * MIN, 5).unwrap();
    store
        .heartbeat("T-2", "s1", 10 * MIN, Some("x"), 6)
        .unwrap();
    let t = store.heartbeat("T-2", "s1", 10 * MIN, Some(""), 7).unwrap();
    assert_eq!(t.progress, None);
    store
        .heartbeat("T-2", "s1", 10 * MIN, Some("y"), 8)
        .unwrap();
    let review = store.complete("T-2", "s1", None, true, false, 9).unwrap();
    assert_eq!(review.task.progress, None);
}

#[test]
fn progress_survives_release_and_expiry() {
    let (_dir, mut store) = open();
    store.add_task(new_task(&[]), 0).unwrap();
    store.claim("T-1", "s1", 10 * MIN, 0).unwrap();
    store
        .heartbeat("T-1", "s1", 10 * MIN, Some("step 1 of 3"), 1)
        .unwrap();
    let released = store.release("T-1", "s1", false, 2).unwrap();
    assert_eq!(released.state, TaskState::Open);
    assert_eq!(released.progress.as_deref(), Some("step 1 of 3"));

    let t = store.claim("T-1", "s2", 10 * MIN, 3).unwrap();
    assert_eq!(
        t.progress.as_deref(),
        Some("step 1 of 3"),
        "the next holder reads it"
    );
    store
        .heartbeat("T-1", "s2", 10 * MIN, Some("step 2 of 3"), 4)
        .unwrap();

    // The lease runs out at 14 min; the next write releases it and keeps the note.
    let t = store.claim("T-1", "s3", 10 * MIN, 20 * MIN).unwrap();
    assert_eq!(t.claimed_by.as_deref(), Some("s3"));
    assert_eq!(t.progress.as_deref(), Some("step 2 of 3"));
    assert_eq!(
        store.task("T-1", 20 * MIN).unwrap().progress.as_deref(),
        Some("step 2 of 3")
    );
}

#[test]
fn progress_is_cleared_by_any_writer_finishing_the_task() {
    // A process from before schema 6 finishes tasks without knowing `progress`;
    // the trigger clears it anyway.
    let (dir, mut store) = open();
    store.add_task(new_task(&[]), 0).unwrap();
    store.claim("T-1", "s1", 10 * MIN, 0).unwrap();
    store
        .heartbeat("T-1", "s1", 10 * MIN, Some("x"), 1)
        .unwrap();
    let raw = rusqlite::Connection::open(dir.path().join("airlock.db")).unwrap();
    raw.execute(
        "UPDATE tasks SET state = 'done', claimed_by = NULL, lease_expires_at = NULL WHERE id = 'T-1'",
        [],
    )
    .unwrap();
    assert_eq!(store.task("T-1", 2).unwrap().progress, None);
}

#[test]
fn only_the_holder_writes_progress() {
    let (_dir, mut store) = open();
    store.add_task(new_task(&[]), 0).unwrap();
    assert!(err(store.heartbeat("T-1", "s1", 10 * MIN, Some("x"), 1)).contains("not claimed"));
    store.claim("T-1", "s1", 10 * MIN, 1).unwrap();
    assert!(err(store.heartbeat("T-1", "s2", 10 * MIN, Some("x"), 2)).contains("claimed by s1"));
    assert_eq!(store.task("T-1", 2).unwrap().progress, None);
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
    let snapshot = Snapshot::new("id", &ProjectConfig::default(), &tasks, &[], &[]);

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
    spec.priority = Priority::Low;
    let added = store.add_task(spec, 42).unwrap();
    drop(store);

    let store = Store::open(&path).unwrap();
    let ids: Vec<String> = store.tasks(42).unwrap().into_iter().map(|t| t.id).collect();
    assert_eq!(ids, ["T-1", "T-2", "T-3"]);
    let c = store.task("T-3", 42).unwrap();
    assert_eq!(c, added);
    assert_eq!(c.blocked_by, ["T-2", "T-1"], "authored order preserved");
    assert_eq!(
        (c.priority, c.created_at, c.updated_at),
        (Priority::Low, 42, 42)
    );
}

#[test]
fn question_lifecycle() {
    let (_dir, mut store) = open();
    let q1 = store
        .ask("melee.md", "Front arc?", "engine-1", None, 10)
        .unwrap();
    let q2 = store
        .ask("missiles.md", "Blast radius?", "engine-1", None, 11)
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
    assert!(err(store.resolve(1, None, 16)).contains("already resolved"));
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

/// The version 1 layout (tasks, task_deps, questions), as it was released.
const LEGACY_V1: &str = "
CREATE TABLE tasks (
    id               TEXT PRIMARY KEY,
    title            TEXT NOT NULL,
    role             TEXT NOT NULL,
    state            TEXT NOT NULL CHECK (state IN ('open', 'in_progress', 'in_review', 'done')),
    priority         INTEGER NOT NULL DEFAULT 0,
    read             TEXT NOT NULL,
    touch            TEXT NOT NULL,
    do               TEXT NOT NULL,
    verify           TEXT NOT NULL,
    notes            TEXT,
    claimed_by       TEXT,
    lease_expires_at INTEGER,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL
) STRICT;
CREATE TABLE task_deps (
    task_id    TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    blocked_by TEXT NOT NULL REFERENCES tasks (id),
    PRIMARY KEY (task_id, blocked_by)
) STRICT;
CREATE TABLE questions (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    topic       TEXT NOT NULL,
    asked_by    TEXT NOT NULL,
    question    TEXT NOT NULL,
    status      TEXT NOT NULL CHECK (status IN ('open', 'answered', 'resolved')),
    answer      TEXT,
    answered_by TEXT,
    spec_file   TEXT,
    created_at  INTEGER NOT NULL,
    answered_at INTEGER
) STRICT;
";

const LEGACY_V2_V3: &str = "
CREATE TABLE exports (
    sha256      TEXT PRIMARY KEY,
    recorded_at INTEGER NOT NULL
) STRICT;
CREATE TABLE bindings (
    root    TEXT PRIMARY KEY,
    role    TEXT NOT NULL,
    session TEXT NOT NULL,
    locked  INTEGER NOT NULL DEFAULT 0
) STRICT;
";

/// Builds a database at `version` (1 or 3) holding one task per entry of
/// `priorities`, ids `T-1`, `T-2`, ...
fn legacy_db(path: &std::path::Path, version: i64, priorities: &[i64]) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(LEGACY_V1).unwrap();
    if version >= 3 {
        conn.execute_batch(LEGACY_V2_V3).unwrap();
    }
    for (i, p) in priorities.iter().enumerate() {
        conn.execute(
            "INSERT INTO tasks (id, title, role, state, priority, read, touch, do, verify, created_at, updated_at)
             VALUES (?1, 't', 'engine', 'open', ?2, '[]', '[]', 'd', 'v', 0, 0)",
            rusqlite::params![format!("T-{}", i + 1), p],
        )
        .unwrap();
    }
    conn.pragma_update(None, "user_version", version).unwrap();
}

#[test]
fn version_3_priorities_are_migrated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("airlock.db");
    legacy_db(&path, 3, &[40, 30, 29, 22, 10, 9, 5, -1]);

    let store = Store::open(&path).unwrap();
    let tasks = store.tasks(0).unwrap();
    let priorities: Vec<Priority> = tasks.iter().map(|t| t.priority).collect();
    assert_eq!(
        priorities,
        [
            Priority::High,
            Priority::High,
            Priority::Normal,
            Priority::Normal,
            Priority::Normal,
            Priority::Low,
            Priority::Low,
            Priority::Low
        ]
    );
    drop(store);

    // The migration also created the track storage Task 2 builds on.
    let conn = rusqlite::Connection::open(&path).unwrap();
    let tracks: i64 = conn
        .query_row("SELECT count(*) FROM tracks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(tracks, 0);
    let unset: i64 = conn
        .query_row("SELECT count(*) FROM tasks WHERE track IS NULL", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(unset, 8, "migrated tasks have no track");

    let mut store = Store::open(&path).unwrap();
    let track = store.add_track("core", Priority::High, "", 0).unwrap();
    assert_eq!(track.name, "core");
    assert_eq!(store.tracks().unwrap(), [track]);
}

#[test]
fn out_of_range_priority_fails_to_load_and_names_the_task() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("airlock.db");
    let mut store = Store::open(&path).unwrap();
    store.add_task(new_task(&[]), 0).unwrap();
    drop(store);
    let conn = rusqlite::Connection::open(&path).unwrap();
    // Only a database without the guard triggers can hold such a rank.
    conn.execute_batch(
        "DROP TRIGGER tasks_priority_rank_insert; DROP TRIGGER tasks_priority_rank_update;",
    )
    .unwrap();
    conn.execute("UPDATE tasks SET priority = 9", []).unwrap();
    drop(conn);

    let store = Store::open(&path).unwrap();
    let message = err(store.tasks(0));
    assert!(message.contains("task `T-1`"), "{message}");
    assert!(message.contains("rank 9"), "{message}");
}

#[test]
fn old_processes_cannot_write_priorities_outside_the_scale() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("airlock.db");
    let mut store = Store::open(&path).unwrap();
    store.add_task(new_task(&[]), 0).unwrap();
    drop(store);

    // What an airlock process from before the migration would still do.
    let conn = rusqlite::Connection::open(&path).unwrap();
    let insert = conn
        .execute(
            "INSERT INTO tasks (id, title, role, state, priority, read, touch, do, verify, created_at, updated_at)
             VALUES ('T-9', 't', 'engine', 'open', 25, '[]', '[]', 'd', 'v', 0, 0)",
            [],
        )
        .expect_err("insert of rank 25 must be refused");
    assert!(
        insert.to_string().contains("restart old airlock processes"),
        "{insert}"
    );
    let update = conn
        .execute("UPDATE tasks SET priority = 25 WHERE id = 'T-1'", [])
        .expect_err("update to rank 25 must be refused");
    assert!(
        update.to_string().contains("restart old airlock processes"),
        "{update}"
    );
    // A valid rank still goes through.
    conn.execute("UPDATE tasks SET priority = 3 WHERE id = 'T-1'", [])
        .unwrap();
    drop(conn);

    let store = Store::open(&path).unwrap();
    let tasks = store.tasks(0).unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].priority, Priority::Critical);
}

#[test]
fn renaming_a_track_after_a_v3_migration_carries_its_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("airlock.db");
    legacy_db(&path, 3, &[40, 10]);

    let mut store = Store::open(&path).unwrap();
    store.add_track("core", Priority::High, "", 0).unwrap();
    store
        .edit_task(
            "T-1",
            TaskEdit {
                track: Some(Some("core".to_string())),
                ..TaskEdit::default()
            },
            false,
            1,
        )
        .unwrap();
    store
        .edit_track(
            "core",
            TrackEdit {
                name: Some("engine-core".to_string()),
                ..TrackEdit::default()
            },
            2,
        )
        .unwrap();
    let tasks = store.tasks(3).unwrap();
    assert_eq!(tasks[0].track.as_deref(), Some("engine-core"));
    assert_eq!(tasks[1].track, None);
}

#[test]
fn version_1_databases_are_migrated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("airlock.db");
    legacy_db(&path, 1, &[]);

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
        store.ask("t", "q", "s", None, 0).unwrap();
    }

    let (_src_dir, mut source) = open();
    source.add_task(new_task(&[]), 0).unwrap();
    source.add_task(new_task(&["T-1"]), 0).unwrap();
    source.claim("T-1", "s1", MIN, 1).unwrap();
    source
        .ask("melee.md", "Front arc?", "engine-1", None, 2)
        .unwrap();
    let snapshot = Snapshot::new(
        "id",
        &ProjectConfig::default(),
        &source.tasks(3).unwrap(),
        &[],
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

    let next = store.ask("t", "q", "s", None, 11).unwrap();
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
                priority: Some(Priority::High),
                touch: Some(vec!["src/b.rs".into(), "src/c.rs".into()]),
                blocked_by: Some(vec!["T-1".into(), "T-1".into()]),
                ..TaskEdit::default()
            },
            false,
            5,
        )
        .unwrap();
    assert_eq!(edited.title, "New title");
    assert_eq!(edited.priority, Priority::High);
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
        .ask_once("overlap:E-2", "Continue?", "engine-1", None, 1)
        .unwrap();
    let again = store
        .ask_once("overlap:E-2", "Continue now?", "engine-1", None, 2)
        .unwrap();
    assert_eq!(again.id, first.id, "the open question is reused");
    assert_eq!(again.question, "Continue?");

    let other = store
        .ask_once("overlap:E-2", "Continue?", "engine-2", None, 3)
        .unwrap();
    assert_ne!(other.id, first.id, "another session gets its own question");

    store
        .answer(first.id, "Go ahead.", "lead-1", None, 4)
        .unwrap();
    let after_answer = store
        .ask_once("overlap:E-2", "Continue?", "engine-1", None, 5)
        .unwrap();
    assert_ne!(
        after_answer.id, first.id,
        "an answered question does not swallow a new claim"
    );
    assert_eq!(store.questions().unwrap().len(), 3);
}

fn track_task(track: Option<&str>) -> NewTask {
    NewTask {
        track: track.map(str::to_string),
        ..new_task(&[])
    }
}

#[test]
fn tracks_are_created_listed_and_validated() {
    let (_dir, mut store) = open();
    let core = store
        .add_track("core", Priority::Critical, "Core fidelity", 5)
        .unwrap();
    store.add_track("comfort", Priority::Low, "", 6).unwrap();
    store
        .add_track("infra", Priority::default(), "", 7)
        .unwrap();

    assert_eq!(core.priority, Priority::Critical);
    assert_eq!(core.description, "Core fidelity");
    assert_eq!((core.created_at, core.updated_at), (5, 5));
    let names: Vec<String> = store
        .tracks()
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["core", "infra", "comfort"]);
    assert_eq!(store.track("infra").unwrap().priority, Priority::Normal);

    assert!(err(store.add_track("core", Priority::Low, "", 8)).contains("already exists"));
    store.add_track("Core", Priority::Low, "", 8).unwrap();
    assert!(
        store.track("Core").is_ok(),
        "names are unique exactly, case kept"
    );
    assert!(
        err(store.add_track("Core fidelity", Priority::Low, "", 8)).contains("invalid track name")
    );
}

#[test]
fn tasks_reference_existing_tracks() {
    let (_dir, mut store) = open();
    store.add_track("comfort", Priority::Low, "", 0).unwrap();
    store.add_track("core", Priority::High, "", 0).unwrap();

    let task = store.add_task(track_task(Some("comfort")), 1).unwrap();
    assert_eq!(task.track.as_deref(), Some("comfort"));
    assert_eq!(
        store.task(&task.id, 1).unwrap().track.as_deref(),
        Some("comfort")
    );

    let message = err(store.add_task(track_task(Some("nope")), 2));
    assert!(message.contains("no track `nope`"), "{message}");
    assert!(
        message.contains("defined tracks: comfort, core"),
        "{message}"
    );
    assert_eq!(store.tasks(2).unwrap().len(), 1, "nothing was created");

    let clear = TaskEdit {
        track: Some(None),
        ..TaskEdit::default()
    };
    assert_eq!(
        store.edit_task(&task.id, clear, false, 3).unwrap().track,
        None
    );
    let set = TaskEdit {
        track: Some(Some("core".to_string())),
        ..TaskEdit::default()
    };
    assert_eq!(
        store
            .edit_task(&task.id, set, false, 4)
            .unwrap()
            .track
            .as_deref(),
        Some("core")
    );
    let unknown = TaskEdit {
        track: Some(Some("nope".to_string())),
        ..TaskEdit::default()
    };
    assert!(err(store.edit_task(&task.id, unknown, false, 5)).contains("no track `nope`"));
    assert_eq!(
        store.task(&task.id, 5).unwrap().track.as_deref(),
        Some("core")
    );
}

#[test]
fn no_tracks_defined_hints_at_the_command() {
    let (_dir, mut store) = open();
    let message = err(store.add_task(track_task(Some("core")), 0));
    assert!(
        message.contains("no tracks defined (airlock track add)"),
        "{message}"
    );
}

#[test]
fn renaming_a_track_moves_its_tasks() {
    let (_dir, mut store) = open();
    store.add_track("comfort", Priority::Low, "", 0).unwrap();
    store.add_track("core", Priority::High, "", 0).unwrap();
    store.add_task(track_task(Some("comfort")), 1).unwrap();
    let t2 = store.add_task(track_task(Some("comfort")), 1).unwrap();
    store.add_task(track_task(None), 1).unwrap();
    store.claim(&t2.id, "s", 600, 2).unwrap();
    store.complete(&t2.id, "s", None, false, false, 3).unwrap();

    let renamed = store
        .edit_track(
            "comfort",
            TrackEdit {
                name: Some("polish".to_string()),
                ..TrackEdit::default()
            },
            10,
        )
        .unwrap();
    assert_eq!(renamed.name, "polish");
    assert_eq!(renamed.updated_at, 10);
    assert_eq!(renamed.created_at, 0);

    let tracks: Vec<Option<String>> = store
        .tasks(10)
        .unwrap()
        .into_iter()
        .map(|t| t.track)
        .collect();
    assert_eq!(tracks, [Some("polish".into()), Some("polish".into()), None]);
    assert_eq!(store.task("T-1", 10).unwrap().updated_at, 1);
    let missing = store.track("comfort").unwrap_err();
    assert!(missing.downcast_ref::<NotFound>().is_some());

    let clash = TrackEdit {
        name: Some("core".to_string()),
        ..TrackEdit::default()
    };
    assert!(err(store.edit_track("polish", clash, 11)).contains("already exists"));
    let bad = TrackEdit {
        name: Some("a b".to_string()),
        ..TrackEdit::default()
    };
    assert!(err(store.edit_track("polish", bad, 11)).contains("invalid track name"));
    assert_eq!(store.track("polish").unwrap().updated_at, 10);
}

#[test]
fn tracks_are_edited_in_place() {
    let (_dir, mut store) = open();
    store.add_track("core", Priority::Normal, "old", 0).unwrap();
    assert!(err(store.edit_track("core", TrackEdit::default(), 1)).contains("nothing to change"));
    assert!(
        err(store.edit_track(
            "nope",
            TrackEdit {
                priority: Some(Priority::Low),
                ..TrackEdit::default()
            },
            1
        ))
        .contains("no track `nope`")
    );

    let edited = store
        .edit_track(
            "core",
            TrackEdit {
                priority: Some(Priority::Critical),
                description: Some("new".to_string()),
                ..TrackEdit::default()
            },
            2,
        )
        .unwrap();
    assert_eq!(edited.priority, Priority::Critical);
    assert_eq!(edited.description, "new");
    assert_eq!((edited.created_at, edited.updated_at), (0, 2));
}

#[test]
fn tracks_in_use_cannot_be_removed() {
    let (_dir, mut store) = open();
    store.add_track("core", Priority::High, "", 0).unwrap();
    let task = store.add_task(track_task(Some("core")), 1).unwrap();
    store.claim(&task.id, "s", 600, 2).unwrap();
    store
        .complete(&task.id, "s", None, false, false, 3)
        .unwrap();

    assert!(err(store.remove_track("core", 5)).contains("track `core` is used by 1 task(s)"));
    assert!(store.track("core").is_ok());

    let clear = TaskEdit {
        track: Some(None),
        ..TaskEdit::default()
    };
    store.edit_task(&task.id, clear, true, 6).unwrap();
    store.remove_track("core", 7).unwrap();
    assert!(store.tracks().unwrap().is_empty());
    let gone = store.remove_track("core", 8).unwrap_err();
    assert!(gone.downcast_ref::<NotFound>().is_some());
}

#[test]
fn correcting_a_resolved_answer_keeps_status_and_history() {
    let (_dir, mut store) = open();
    store.ask("melee.md", "How?", "engine-1", None, 10).unwrap();
    store
        .answer(1, "first", "lead", Some("docs/spec/melee.md"), 11)
        .unwrap();
    store.resolve(1, None, 12).unwrap();

    let corrected = store.answer(1, "second", "lead-1", None, 13).unwrap();
    assert_eq!(corrected.status, QuestionStatus::Resolved);
    assert_eq!(corrected.answer.as_deref(), Some("second"));
    assert_eq!(corrected.answered_by.as_deref(), Some("lead-1"));
    assert_eq!(corrected.answered_at, Some(13));
    assert_eq!(corrected.corrected_at, Some(13));
    assert_eq!(corrected.corrected_by.as_deref(), Some("lead-1"));
    assert_eq!(
        corrected.history,
        [AnswerVersion {
            answer: "first".into(),
            answered_by: Some("lead".into()),
            answered_at: Some(11),
            spec_file: Some("docs/spec/melee.md".into()),
        }]
    );
    assert_eq!(store.question(1).unwrap(), corrected, "persisted");
    assert_eq!(store.questions().unwrap(), [corrected]);
}

#[test]
fn answering_an_open_question_is_not_a_correction() {
    let (_dir, mut store) = open();
    store.ask("melee.md", "How?", "engine-1", None, 10).unwrap();
    let answered = store.answer(1, "first", "lead", None, 11).unwrap();
    assert_eq!(answered.status, QuestionStatus::Answered);
    assert_eq!(answered.corrected_at, None);
    assert_eq!(answered.corrected_by, None);
    assert!(answered.history.is_empty());
}

#[test]
fn two_corrections_keep_both_earlier_answers() {
    let (_dir, mut store) = open();
    store.ask("melee.md", "How?", "engine-1", None, 10).unwrap();
    for (i, text) in ["first", "second", "third"].into_iter().enumerate() {
        store.answer(1, text, "lead", None, 11 + i as i64).unwrap();
    }
    let q = store.question(1).unwrap();
    assert_eq!(q.status, QuestionStatus::Answered);
    assert_eq!(q.answer.as_deref(), Some("third"));
    assert_eq!(q.corrected_at, Some(13));
    let earlier: Vec<&str> = q.history.iter().map(|v| v.answer.as_str()).collect();
    assert_eq!(earlier, ["first", "second"], "oldest first");
}

#[test]
fn answered_questions_from_older_databases_can_be_corrected_after_migration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("airlock.db");
    legacy_db(&path, 3, &[]);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "INSERT INTO questions (topic, asked_by, question, status, answer, answered_by, created_at, answered_at)
             VALUES ('melee.md', 'engine-1', 'Arc?', 'resolved', 'old', 'lead', 1, 2)",
            [],
        )
        .unwrap();

    let mut store = Store::open(&path).unwrap();
    let before = store.question(1).unwrap();
    assert_eq!(before.corrected_at, None);
    assert!(before.history.is_empty());

    let corrected = store.answer(1, "new", "lead", None, 10).unwrap();
    assert_eq!(corrected.status, QuestionStatus::Resolved);
    assert_eq!(corrected.history[0].answer, "old");
    assert_eq!(corrected.history[0].answered_at, Some(2));
}

#[test]
fn old_processes_cannot_replace_an_answer_without_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("airlock.db");
    let mut store = Store::open(&path).unwrap();
    store.ask("melee.md", "How?", "engine-1", None, 10).unwrap();
    store.answer(1, "first", "lead", None, 11).unwrap();
    drop(store);

    // What an airlock process from before the migration would still do.
    let conn = rusqlite::Connection::open(&path).unwrap();
    let update = conn
        .execute(
            "UPDATE questions SET status = 'answered', answer = 'overwritten', answered_by = 'lead',
                    answered_at = 12 WHERE id = 1",
            [],
        )
        .expect_err("an answer change without history must be refused");
    assert!(
        update.to_string().contains("restart old airlock processes"),
        "{update}"
    );
    // Other columns of an answered question still change freely.
    conn.execute("UPDATE questions SET status = 'resolved' WHERE id = 1", [])
        .unwrap();
    drop(conn);

    // The normal correction path passes, also twice within one second.
    let mut store = Store::open(&path).unwrap();
    store.answer(1, "second", "lead-1", None, 13).unwrap();
    let q = store.answer(1, "third", "lead-1", None, 13).unwrap();
    assert_eq!(q.answer.as_deref(), Some("third"));
    assert_eq!(q.status, QuestionStatus::Resolved);
    let earlier: Vec<&str> = q.history.iter().map(|v| v.answer.as_str()).collect();
    assert_eq!(earlier, ["first", "second"]);
    drop(store);

    // Earlier history does not cover the current answer.
    let conn = rusqlite::Connection::open(&path).unwrap();
    let again = conn
        .execute("UPDATE questions SET answer = 'lost' WHERE id = 1", [])
        .expect_err("history of earlier answers does not cover the current one");
    assert!(
        again.to_string().contains("restart old airlock processes"),
        "{again}"
    );
}

#[test]
fn listed_questions_carry_each_their_own_history() {
    let (_dir, mut store) = open();
    for topic in ["a.md", "b.md", "c.md"] {
        store.ask(topic, "How?", "engine-1", None, 10).unwrap();
    }
    // #1 corrected twice, #2 never answered, #3 corrected once.
    for text in ["1a", "1b", "1c"] {
        store.answer(1, text, "lead", None, 11).unwrap();
    }
    for text in ["3a", "3b"] {
        store.answer(3, text, "lead", None, 12).unwrap();
    }
    let listed = store.questions().unwrap();
    let ids: Vec<u64> = listed.iter().map(|q| q.id).collect();
    assert_eq!(ids, [3, 2, 1], "newest first");
    for q in &listed {
        assert_eq!(*q, store.question(q.id).unwrap(), "#{}", q.id);
    }
    let earlier = |q: &airlock::core::Question| -> Vec<String> {
        q.history.iter().map(|v| v.answer.clone()).collect()
    };
    assert_eq!(earlier(&listed[0]), ["3a"]);
    assert!(listed[1].history.is_empty());
    assert_eq!(earlier(&listed[2]), ["1a", "1b"]);
}

#[test]
fn tasks_are_listed_in_numeric_id_order() {
    let (_dir, mut store) = open();
    for _ in 0..11 {
        store.add_task(new_task(&[]), 0).unwrap();
    }
    let ids: Vec<String> = store.tasks(0).unwrap().into_iter().map(|t| t.id).collect();
    let expected: Vec<String> = (1..=11).map(|n| format!("T-{n}")).collect();
    assert_eq!(ids, expected, "T-2 before T-10");
}

#[test]
fn concurrent_first_opens_of_a_fresh_store_all_succeed() {
    for _ in 0..5 {
        let dir = Arc::new(TempDir::new().unwrap());
        let path = dir.path().join("airlock.db");
        let sessions = 8;
        let barrier = Arc::new(Barrier::new(sessions));
        let handles: Vec<_> = (0..sessions)
            .map(|_| {
                let path = path.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    Store::open(&path).map(|_| ()).map_err(|e| format!("{e:#}"))
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap().unwrap();
        }
    }
}

// --- Events and the inbox (Spec 01 §2.6) ---

fn role_task(role: &str, prefix: &str, blocked_by: &[&str]) -> NewTask {
    NewTask {
        prefix: prefix.to_string(),
        role: role.to_string(),
        ..new_task(blocked_by)
    }
}

/// `(kind, role, origin, task_id, question_id, track)` of every event, oldest first.
type EventRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
);

fn events(dir: &TempDir) -> Vec<EventRow> {
    let conn = rusqlite::Connection::open(dir.path().join("airlock.db")).unwrap();
    let mut stmt = conn
        .prepare("SELECT kind, role, origin, task_id, question_id, track FROM events ORDER BY seq")
        .unwrap();
    stmt.query_map([], |r| {
        Ok((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
        ))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn kinds(dir: &TempDir) -> Vec<String> {
    events(dir)
        .into_iter()
        .map(|e| format!("{} {}", e.0, e.1))
        .collect()
}

fn inbox_kinds(inbox: &airlock::core::Inbox) -> Vec<String> {
    inbox
        .items
        .iter()
        .map(|i| {
            let subject = i
                .task
                .as_ref()
                .map(|t| t.id.clone())
                .or_else(|| i.question.as_ref().map(|q| format!("#{}", q.id)))
                .or_else(|| i.track.as_ref().map(|t| t.name.clone()))
                .unwrap_or_default();
            format!("{} {subject}", i.kind.as_str())
        })
        .collect()
}

#[test]
fn events_are_written_with_their_change() {
    let (dir, mut store) = open();
    store.add_track("core", Priority::Normal, "", 0).unwrap();
    // created ready → task_ready; created blocked → nothing
    store.add_task(role_task("engine", "E", &[]), 1).unwrap(); // E-1
    store
        .add_task(role_task("engine", "E", &["E-1"]), 1)
        .unwrap(); // E-2, blocked
    assert_eq!(kinds(&dir), ["task_ready engine"]);

    // claim writes nothing; completing E-1 makes E-2 ready
    store.claim("E-1", "engine-1", 600, 2).unwrap();
    store
        .complete("E-1", "engine-1", None, false, false, 3)
        .unwrap();
    assert_eq!(kinds(&dir)[1..], ["task_ready engine"]);
    assert_eq!(events(&dir)[1].3.as_deref(), Some("E-2"));

    // release → ready again
    store.claim("E-2", "engine-1", 600, 4).unwrap();
    store.release("E-2", "engine-1", false, 5).unwrap();
    assert_eq!(kinds(&dir)[2..], ["task_ready engine"]);

    // review: in_review for the coordinators, origin engine; reject → ready, rejected
    store.claim("E-2", "engine-1", 600, 6).unwrap();
    store
        .complete("E-2", "engine-1", None, true, false, 7)
        .unwrap();
    assert_eq!(events(&dir)[3].0, "task_in_review");
    assert_eq!(events(&dir)[3].1, "*");
    assert_eq!(events(&dir)[3].2.as_deref(), Some("engine"));
    store.reject("E-2", "again", 8).unwrap();
    assert_eq!(
        kinds(&dir)[4..],
        ["task_ready engine", "task_rejected engine"]
    );

    // edit: priority of a ready task → task_changed; a blocked one → nothing
    let edit = TaskEdit {
        priority: Some(Priority::High),
        ..Default::default()
    };
    store.edit_task("E-2", edit, false, 9).unwrap();
    assert_eq!(kinds(&dir)[6..], ["task_changed engine"]);
    store
        .add_task(role_task("engine", "E", &["E-2"]), 10)
        .unwrap(); // E-3, blocked
    let edit = TaskEdit {
        title: Some("Blocked".into()),
        ..Default::default()
    };
    store.edit_task("E-3", edit, false, 11).unwrap();
    assert_eq!(events(&dir).len(), 7);
    // moving a ready task to another role → task_ready for that role
    let mut spec = role_task("engine", "E", &[]);
    spec.track = Some("core".into());
    store.add_task(spec, 12).unwrap(); // E-4
    let edit = TaskEdit {
        role: Some("spec".into()),
        ..Default::default()
    };
    store.edit_task("E-4", edit, false, 13).unwrap();
    assert_eq!(kinds(&dir)[7..], ["task_ready engine", "task_ready spec"]);

    // track priority → every role with open tasks on it; a description only → nothing
    let edit = TrackEdit {
        priority: Some(Priority::High),
        ..Default::default()
    };
    store.edit_track("core", edit, 14).unwrap();
    assert_eq!(kinds(&dir)[9..], ["track_changed spec"]);
    let edit = TrackEdit {
        description: Some("d".into()),
        ..Default::default()
    };
    store.edit_track("core", edit, 15).unwrap();
    assert_eq!(events(&dir).len(), 10);

    // questions: asked for the coordinators, answered for the asker's role
    store
        .ask("melee.md", "Arc?", "engine-1", Some("engine"), 16)
        .unwrap();
    store.answer(1, "Front", "lead-1", None, 17).unwrap();
    store.answer(1, "Front 90°", "lead-1", None, 18).unwrap();
    assert_eq!(
        kinds(&dir)[10..],
        [
            "question_asked *",
            "question_answered engine",
            "question_answered engine"
        ]
    );

    // approve makes dependents ready
    store.claim("E-3", "engine-1", 600, 19).unwrap_err(); // still blocked by E-2
    store.claim("E-2", "engine-1", 600, 20).unwrap();
    store
        .complete("E-2", "engine-1", None, true, false, 21)
        .unwrap();
    store.approve("E-2", None, 22).unwrap();
    let tail: Vec<_> = events(&dir)[13..]
        .iter()
        .map(|e| (e.0.clone(), e.3.clone()))
        .collect();
    assert_eq!(
        tail,
        [
            ("task_in_review".to_string(), Some("E-2".to_string())),
            ("task_ready".to_string(), Some("E-3".to_string()))
        ]
    );

    // a refused transition writes nothing
    let before = events(&dir).len();
    store.release("E-1", "engine-1", false, 23).unwrap_err();
    store.reject("E-1", "no", 23).unwrap_err();
    assert_eq!(events(&dir).len(), before);
}

#[test]
fn inbox_reads_each_event_once_per_session() {
    let (_dir, mut store) = open();
    // Both sessions start at the current end.
    assert!(
        store
            .inbox("engine-1", "engine", false, false, 0)
            .unwrap()
            .items
            .is_empty()
    );
    assert!(
        store
            .inbox("engine-2", "engine", false, false, 0)
            .unwrap()
            .items
            .is_empty()
    );
    store.add_task(role_task("engine", "E", &[]), 1).unwrap();

    let peeked = store.inbox("engine-1", "engine", false, true, 2).unwrap();
    assert_eq!(inbox_kinds(&peeked), ["task_ready E-1"]);
    let read = store.inbox("engine-1", "engine", false, false, 2).unwrap();
    assert_eq!(read, peeked, "peek did not advance");
    assert!(
        store
            .inbox("engine-1", "engine", false, false, 3)
            .unwrap()
            .items
            .is_empty()
    );
    // The other session of the role gets it too, once.
    let other = store.inbox("engine-2", "engine", false, false, 3).unwrap();
    assert_eq!(inbox_kinds(&other), ["task_ready E-1"]);
    assert!(
        store
            .inbox("engine-2", "engine", false, false, 3)
            .unwrap()
            .items
            .is_empty()
    );
    // A cursor per role: the same session reading for another role starts fresh.
    assert!(
        store
            .inbox("engine-1", "spec", true, false, 3)
            .unwrap()
            .items
            .is_empty()
    );
}

#[test]
fn a_new_session_starts_at_the_current_end() {
    let (_dir, mut store) = open();
    store.add_task(role_task("engine", "E", &[]), 1).unwrap();
    store.ask("t", "q", "engine-1", Some("engine"), 1).unwrap();
    assert!(
        store
            .inbox("engine-9", "engine", false, false, 2)
            .unwrap()
            .items
            .is_empty()
    );
    assert!(
        store
            .inbox("lead-9", "lead", true, false, 2)
            .unwrap()
            .items
            .is_empty()
    );
}

#[test]
fn inbox_drops_what_is_no_longer_current() {
    let (_dir, mut store) = open();
    store.inbox("engine-1", "engine", false, false, 0).unwrap();
    store.inbox("lead-1", "lead", true, false, 0).unwrap();
    store.add_task(role_task("engine", "E", &[]), 1).unwrap(); // E-1
    store.add_task(role_task("engine", "E", &[]), 1).unwrap(); // E-2
    store
        .ask("a.md", "q1", "engine-1", Some("engine"), 1)
        .unwrap(); // #1
    store
        .ask("b.md", "q2", "engine-1", Some("engine"), 1)
        .unwrap(); // #2
    // E-1 claimed by someone else, #1 answered by another coordinator.
    store.claim("E-1", "engine-2", 600, 2).unwrap();
    store.answer(1, "a", "spec-1", None, 2).unwrap();

    let engine = store.inbox("engine-1", "engine", false, false, 3).unwrap();
    assert_eq!(
        inbox_kinds(&engine),
        ["task_ready E-2", "question_answered #1"]
    );
    let lead = store.inbox("lead-1", "lead", true, false, 3).unwrap();
    assert_eq!(
        inbox_kinds(&lead),
        ["question_asked #2"],
        "#1 is no longer open"
    );
}

#[test]
fn inbox_collapses_a_subject_to_its_latest_event() {
    let (_dir, mut store) = open();
    store.inbox("engine-1", "engine", false, false, 0).unwrap();
    store.add_task(role_task("engine", "E", &[]), 1).unwrap();
    let edit = TaskEdit {
        priority: Some(Priority::Critical),
        ..Default::default()
    };
    store.edit_task("E-1", edit, false, 2).unwrap();
    let inbox = store.inbox("engine-1", "engine", false, false, 3).unwrap();
    assert_eq!(inbox_kinds(&inbox), ["task_changed E-1"]);
    let task = inbox.items[0].task.as_ref().unwrap();
    assert_eq!(task.priority, Priority::Critical);
    assert_eq!(task.status, airlock::core::TaskStatus::Ready);
}

#[test]
fn coordinating_roles_see_questions_and_reviews_of_other_roles_only() {
    let (_dir, mut store) = open();
    for (s, r, c) in [
        ("engine-1", "engine", false),
        ("lead-1", "lead", true),
        ("spec-1", "spec", true),
    ] {
        store.inbox(s, r, c, false, 0).unwrap();
    }
    store
        .ask("a.md", "from engine", "engine-1", Some("engine"), 1)
        .unwrap(); // #1
    store
        .ask("b.md", "from spec", "spec-1", Some("spec"), 1)
        .unwrap(); // #2
    store
        .ask("c.md", "unknown asker", "human:peter", None, 1)
        .unwrap(); // #3
    store.add_task(role_task("spec", "S", &[]), 1).unwrap();
    store.claim("S-1", "spec-1", 600, 2).unwrap();
    store
        .complete("S-1", "spec-1", None, true, false, 3)
        .unwrap();

    let lead = store.inbox("lead-1", "lead", true, false, 4).unwrap();
    assert_eq!(
        inbox_kinds(&lead),
        [
            "question_asked #1",
            "question_asked #2",
            "question_asked #3",
            "task_in_review S-1"
        ]
    );
    let spec = store.inbox("spec-1", "spec", true, false, 4).unwrap();
    assert_eq!(
        inbox_kinds(&spec),
        ["question_asked #1", "question_asked #3"],
        "not its own question, not its own review, and S-1 is no longer ready"
    );
    let engine = store.inbox("engine-1", "engine", false, false, 4).unwrap();
    assert!(
        engine.items.is_empty(),
        "clean-room roles never see questions or reviews"
    );
}

#[test]
fn answers_reach_the_asker_through_its_binding_when_the_event_is_gone() {
    let (_dir, mut store) = open();
    store
        .bind(
            "/w/engine",
            &airlock::hub::Binding {
                role: "engine".into(),
                session: "engine@w".into(),
                locked: true,
            },
            false,
            0,
        )
        .unwrap();
    store.inbox("engine@w", "engine", false, false, 0).unwrap();
    // Asked without a role (CLI with --session); answered 31 days later.
    store.ask("a.md", "q", "engine@w", None, 1).unwrap();
    store
        .answer(1, "a", "lead-1", None, 1 + 31 * 86_400)
        .unwrap();
    let inbox = store
        .inbox("engine@w", "engine", false, false, 2 + 31 * 86_400)
        .unwrap();
    assert_eq!(inbox_kinds(&inbox), ["question_answered #1"]);
    let q = inbox.items[0].question.as_ref().unwrap();
    assert_eq!(q.answer.as_deref(), Some("a"));
}

#[test]
fn events_hold_no_text_and_are_pruned_after_30_days() {
    let (dir, mut store) = open();
    store.add_task(role_task("engine", "E", &[]), 0).unwrap();
    store
        .ask(
            "secret-topic.md",
            "SECRET QUESTION",
            "engine-1",
            Some("engine"),
            0,
        )
        .unwrap();
    store.answer(1, "SECRET ANSWER", "lead-1", None, 0).unwrap();
    let conn = rusqlite::Connection::open(dir.path().join("airlock.db")).unwrap();
    let dump: String = conn
        .query_row(
            "SELECT group_concat(kind || role || coalesce(origin,'') || coalesce(task_id,'') || coalesce(question_id,'') || coalesce(track,''), '|') FROM events",
            [],
            |r| r.get(0),
        )
        .unwrap();
    for text in ["SECRET", "secret-topic", "Task"] {
        assert!(!dump.contains(text), "{dump}");
    }
    assert_eq!(events(&dir).len(), 3);
    // Any write after 30 days prunes them.
    store
        .add_track("later", Priority::Normal, "", 31 * 86_400)
        .unwrap();
    assert!(events(&dir).is_empty());
}
