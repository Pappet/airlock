use airlock::activity::{
    ActivityLog, Channel, ERROR_MAX, Entry, Filter, LIVE_SECS, NAME_MAX, RETENTION_SECS,
    agent_from_env, build_time, corrects, is_write, record_quietly, summarize_cli, summarize_json,
    truncate_error,
};
use airlock::cli::{Command, TaskCommand};
use airlock::core::{AnswerVersion, Question, QuestionStatus};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

fn entry(at: i64, session: &str, command: &str) -> Entry {
    Entry {
        id: 0,
        at,
        session: session.to_string(),
        role: Some("backend".to_string()),
        clean_room: false,
        channel: Channel::Mcp,
        agent: Some("claude-code".to_string()),
        command: command.to_string(),
        target: None,
        changed: Vec::new(),
        write: false,
        ok: true,
        error: None,
        pid: 4242,
        build: "0.1.0+1".to_string(),
    }
}

fn open(dir: &TempDir) -> ActivityLog {
    ActivityLog::open(&dir.path().join("activity.db")).unwrap()
}

#[test]
fn record_and_read_back_newest_first() {
    let dir = TempDir::new().unwrap();
    let mut log = open(&dir);
    let mut first = entry(100, "s1", "task_list");
    first.target = Some("T-1".to_string());
    first.changed = vec!["T-1".to_string(), "T-2".to_string()];
    assert_eq!(log.record(&first).unwrap(), 1);
    assert_eq!(log.record(&entry(200, "s1", "task_claim")).unwrap(), 2);

    let entries = log.entries(&Filter::default()).unwrap();
    let ids: Vec<u64> = entries.iter().map(|e| e.id).collect();
    assert_eq!(ids, vec![2, 1]);
    assert_eq!(entries[1], Entry { id: 1, ..first });
    assert_eq!(log.latest_id().unwrap(), 2);
}

#[test]
fn latest_id_is_zero_when_empty() {
    let dir = TempDir::new().unwrap();
    assert_eq!(open(&dir).latest_id().unwrap(), 0);
}

#[test]
fn filters_by_session_role_channel_writes_failures_and_since() {
    let dir = TempDir::new().unwrap();
    let mut log = open(&dir);

    let a = entry(100, "s1", "task_list");
    let mut b = entry(200, "s2", "task_claim");
    b.role = Some("frontend".to_string());
    b.write = true;
    let mut c = entry(300, "s2", "task_done");
    c.channel = Channel::Cli;
    c.write = true;
    c.ok = false;
    c.error = Some("boom".to_string());
    let mut d = entry(400, "s3", "tasks");
    d.channel = Channel::Rest;
    d.role = None;
    for e in [&a, &b, &c, &d] {
        log.record(e).unwrap();
    }

    let commands = |filter: Filter| -> Vec<String> {
        log.entries(&filter)
            .unwrap()
            .into_iter()
            .map(|e| e.command)
            .collect()
    };
    assert_eq!(
        commands(Filter {
            session: Some("s2".to_string()),
            ..Filter::default()
        }),
        vec!["task_done", "task_claim"]
    );
    assert_eq!(
        commands(Filter {
            role: Some("frontend".to_string()),
            ..Filter::default()
        }),
        vec!["task_claim"]
    );
    assert_eq!(
        commands(Filter {
            channel: Some(Channel::Cli),
            ..Filter::default()
        }),
        vec!["task_done"]
    );
    assert_eq!(
        commands(Filter {
            writes_only: true,
            ..Filter::default()
        }),
        vec!["task_done", "task_claim"]
    );
    assert_eq!(
        commands(Filter {
            failures_only: true,
            ..Filter::default()
        }),
        vec!["task_done"]
    );
    assert_eq!(
        commands(Filter {
            since: Some(300),
            ..Filter::default()
        }),
        vec!["tasks", "task_done"]
    );
    assert_eq!(
        commands(Filter {
            limit: Some(1),
            ..Filter::default()
        }),
        vec!["tasks"]
    );
}

#[test]
fn prunes_entries_older_than_thirty_days() {
    let dir = TempDir::new().unwrap();
    let mut log = open(&dir);
    log.record(&entry(1_000, "s1", "task_list")).unwrap();
    log.record(&entry(1_000 + RETENTION_SECS + 1, "s1", "task_list"))
        .unwrap();
    let entries = log.entries(&Filter::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].at, 1_000 + RETENTION_SECS + 1);
}

#[test]
fn record_truncates_long_errors() {
    let dir = TempDir::new().unwrap();
    let mut log = open(&dir);
    let mut e = entry(1, "s1", "task_done");
    e.ok = false;
    e.error = Some("x".repeat(400));
    log.record(&e).unwrap();
    let stored = log.entries(&Filter::default()).unwrap();
    assert_eq!(stored[0].error.as_ref().unwrap().chars().count(), ERROR_MAX);
}

#[test]
fn last_briefing_is_latest_briefing_command_of_that_session() {
    let dir = TempDir::new().unwrap();
    let mut log = open(&dir);
    log.record(&entry(100, "s1", "session_start")).unwrap();
    log.record(&entry(150, "s1", "task_claim")).unwrap();
    log.record(&entry(200, "s1", "start")).unwrap();
    log.record(&entry(250, "s1", "task_claim")).unwrap();
    log.record(&entry(300, "s2", "prompt start")).unwrap();
    assert_eq!(log.last_briefing("s1").unwrap(), Some(200));
    assert_eq!(log.last_briefing("s2").unwrap(), Some(300));
    assert_eq!(log.last_briefing("nobody").unwrap(), None);
}

#[test]
fn last_briefing_ignores_failed_briefings() {
    let dir = TempDir::new().unwrap();
    let mut log = open(&dir);
    log.record(&entry(100, "s1", "session_start")).unwrap();
    let mut failed = entry(200, "s1", "session_start");
    failed.ok = false;
    failed.error = Some("boom".into());
    log.record(&failed).unwrap();
    assert_eq!(log.last_briefing("s1").unwrap(), Some(100));
    let mut only_failed = entry(300, "s2", "start");
    only_failed.ok = false;
    log.record(&only_failed).unwrap();
    assert_eq!(log.last_briefing("s2").unwrap(), None);
}

#[test]
fn last_briefing_ignores_non_briefing_commands() {
    let dir = TempDir::new().unwrap();
    let mut log = open(&dir);
    log.record(&entry(100, "s1", "task_claim")).unwrap();
    assert_eq!(log.last_briefing("s1").unwrap(), None);
}

#[test]
fn processes_are_live_until_they_stop_touching() {
    let dir = TempDir::new().unwrap();
    let mut log = open(&dir);
    let process = log.register("mcp", Some("s1"), Some("backend"), 0).unwrap();

    let live = log.processes(LIVE_SECS).unwrap();
    assert_eq!(live, [process]);
    assert_eq!(live[0].kind, "mcp");
    assert_eq!(live[0].session.as_deref(), Some("s1"));
    assert_eq!(live[0].role.as_deref(), Some("backend"));
    assert_eq!(live[0].pid, std::process::id());
    assert_eq!(live[0].build, airlock::activity::build());
    assert_eq!(live[0].started_at, 0);

    assert!(log.processes(LIVE_SECS + 1).unwrap().is_empty());
    // The stale row is gone for good, even for an earlier "now".
    assert!(log.processes(0).unwrap().is_empty());
}

#[test]
fn touch_keeps_a_process_live_and_unregister_removes_it() {
    let dir = TempDir::new().unwrap();
    let mut log = open(&dir);
    let process = log.register("serve", None, None, 0).unwrap();
    log.touch(&process, 100).unwrap();
    let live = log.processes(100 + LIVE_SECS).unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].last_seen, 100);
    assert_eq!(live[0].started_at, 0);
    log.unregister(process.id).unwrap();
    assert!(log.processes(100).unwrap().is_empty());
}

#[test]
fn a_live_process_reappears_after_a_heartbeat_gap() {
    let dir = TempDir::new().unwrap();
    let mut log = open(&dir);
    let process = log.register("mcp", Some("s1"), Some("backend"), 0).unwrap();
    let other = log.register("serve", None, None, 0).unwrap();

    // A suspend: no heartbeat for longer than LIVE_SECS, and a reader prunes.
    let resumed = LIVE_SECS + 1;
    assert!(log.processes(resumed).unwrap().is_empty());

    log.touch(&process, resumed).unwrap();
    let live = log.processes(resumed).unwrap();
    assert_eq!(live.len(), 1, "only the touched process is live: {live:?}");
    assert_eq!(live[0].id, process.id);
    assert_eq!(live[0].kind, "mcp");
    assert_eq!(live[0].session.as_deref(), Some("s1"));
    assert_eq!(live[0].role.as_deref(), Some("backend"));
    assert_eq!(live[0].started_at, 0);
    assert_eq!(live[0].last_seen, resumed);

    // A fresh registration never takes a re-inserted id.
    let fresh = log.register("mcp", None, None, resumed).unwrap();
    assert!(fresh.id > other.id && fresh.id != process.id);
}

#[test]
fn parallel_writers_lose_nothing() {
    let dir = Arc::new(TempDir::new().unwrap());
    let path = dir.path().join("activity.db");
    let handles: Vec<_> = (0..8)
        .map(|n| {
            let path = path.clone();
            std::thread::spawn(move || {
                let mut log = ActivityLog::open(&path).unwrap();
                for i in 0..25 {
                    log.record(&entry(1_000 + i, &format!("s{n}"), "task_list"))
                        .unwrap();
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let log = ActivityLog::open(&path).unwrap();
    assert_eq!(log.entries(&Filter::default()).unwrap().len(), 200);
}

#[test]
fn path_for_is_a_sibling_activity_db() {
    let path = ActivityLog::path_for(&PathBuf::from("/hub/projects/p/airlock.db"));
    assert_eq!(path, PathBuf::from("/hub/projects/p/activity.db"));
}

#[test]
fn record_quietly_survives_an_unwritable_path() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("activity.db")).unwrap();
    record_quietly(&dir.path().join("airlock.db"), &entry(1, "s1", "task_list"));
}

#[test]
fn record_quietly_writes_next_to_the_database() {
    let dir = TempDir::new().unwrap();
    record_quietly(&dir.path().join("airlock.db"), &entry(1, "s1", "task_list"));
    let log = open(&dir);
    assert_eq!(log.entries(&Filter::default()).unwrap().len(), 1);
}

#[test]
fn build_time_parses_the_suffix() {
    assert_eq!(build_time("0.1.0+1700000000"), 1_700_000_000);
    assert_eq!(build_time("0.1.0"), 0);
    assert_eq!(build_time("0.1.0+soon"), 0);
    assert!(build_time(airlock::activity::build()) > 0);
}

#[test]
fn agent_from_env_recognises_claude_code() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |name: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        }
    };
    assert_eq!(
        agent_from_env(env(&[("AI_AGENT", "claude-code_2-1-316_agent")])),
        Some("claude-code".to_string())
    );
    assert_eq!(
        agent_from_env(env(&[("CLAUDECODE", "1")])),
        Some("claude-code".to_string())
    );
    assert_eq!(agent_from_env(env(&[])), None);
}

#[test]
fn truncate_error_keeps_300_chars() {
    assert_eq!(truncate_error(&"x".repeat(400)).chars().count(), 300);
    assert_eq!(truncate_error("short"), "short");
    let wide = "ä".repeat(400);
    let cut = truncate_error(&wide);
    assert_eq!(cut.chars().count(), 300);
    assert!(wide.starts_with(&cut));
}

fn summary(args: Value) -> (Option<String>, Vec<String>) {
    summarize_json("cmd", args.as_object().unwrap())
}

#[test]
fn summary_takes_the_target_from_the_first_identifying_argument() {
    let (target, changed) = summary(json!({ "task_id": "E-1", "name": "ui", "title": "T" }));
    assert_eq!(target.as_deref(), Some("E-1"));
    assert_eq!(changed, ["name", "title"]);
    assert_eq!(
        summary(json!({ "question_id": 7, "answer": "x" }))
            .0
            .as_deref(),
        Some("#7")
    );
    assert_eq!(summary(json!({ "id": "E-2" })).0.as_deref(), Some("E-2"));
    assert_eq!(summary(json!({ "name": "ui" })).0.as_deref(), Some("ui"));
    assert_eq!(summary(json!({ "title": "T" })).0, None);
    assert_eq!(
        summary(json!({ "task_id": null })),
        (None, vec!["task_id".to_string()])
    );
}

#[test]
fn summary_attaches_only_enumerated_short_values() {
    let (_, changed) = summary(json!({
        "task_id": "E-1", "priority": "high", "track": "ui", "role": "engine",
        "status": "open", "review": true, "lease_minutes": 30, "clean_room": false,
        "lock": true, "title": "T", "do": "free text", "blocked_by": ["E-2"],
    }));
    assert_eq!(
        changed,
        [
            "blocked_by",
            "clean_room: false",
            "do",
            "lease_minutes: 30",
            "lock: true",
            "priority: high",
            "review: true",
            "role: engine",
            "status: open",
            "title",
            "track: ui",
        ]
    );
}

#[test]
fn summary_drops_enumerated_values_that_are_not_short_plain_words() {
    let (_, changed) = summary(json!({
        "priority": "SECRET words here", "track": "x".repeat(41), "role": "",
        "status": ["a"], "lock": "a/b",
    }));
    assert_eq!(changed, ["lock", "priority", "role", "status", "track"]);
    assert_eq!(summary(json!({ "track": null })).1, ["track: null"]);
}

#[test]
fn summary_records_a_new_name_as_plain_name() {
    let (target, changed) = summary(json!({ "name": "old", "new_name": "SECRET-NEW" }));
    assert_eq!(target.as_deref(), Some("old"));
    assert_eq!(changed, ["name"]);
}

#[test]
fn reads_are_not_writes() {
    for read in [
        "session_start",
        "prompt start",
        "task_next",
        "task_get",
        "question_list",
        "track_list",
        "start",
        "task list",
        "task next",
        "task show",
        "track list",
        "question list",
        "role list",
        "GET /api/tasks",
    ] {
        assert!(!is_write(read), "{read}");
    }
    for write in [
        "task_claim",
        "task add",
        "question answer",
        "POST /api/tasks",
        "PATCH /api/x",
        "sync",
    ] {
        assert!(is_write(write), "{write}");
    }
}

#[test]
fn hostile_targets_and_keys_are_capped() {
    let long_id = "é".repeat(10_000);
    let long_key = "k".repeat(10_000);
    let mut args = serde_json::Map::new();
    args.insert("task_id".into(), Value::String(long_id.clone()));
    args.insert(long_key.clone(), json!(1));
    let (target, changed) = summarize_json("cmd", &args);
    let target = target.unwrap();
    assert_eq!(NAME_MAX, 120);
    assert_eq!(target.chars().count(), NAME_MAX);
    assert!(long_id.starts_with(&target));
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].chars().count(), NAME_MAX);
    assert!(long_key.starts_with(&changed[0]));

    let (target, _) = summarize_cli(&Command::Task(TaskCommand::Show {
        id: long_id.clone(),
    }));
    assert_eq!(target.unwrap().chars().count(), NAME_MAX);
}

#[test]
fn a_correction_is_read_from_the_answered_question() {
    let first = Question {
        id: 1,
        topic: "melee.md".into(),
        asked_by: "engine-1".into(),
        question: "How?".into(),
        status: QuestionStatus::Answered,
        answer: Some("first".into()),
        answered_by: Some("lead".into()),
        spec_file: None,
        created_at: 10,
        answered_at: Some(11),
        corrected_at: None,
        corrected_by: None,
        history: Vec::new(),
    };
    assert!(!corrects(&first), "a first answer");
    let corrected = Question {
        answer: Some("second".into()),
        answered_at: Some(12),
        corrected_at: Some(12),
        corrected_by: Some("lead".into()),
        history: vec![AnswerVersion {
            answer: "first".into(),
            answered_by: Some("lead".into()),
            answered_at: Some(11),
            spec_file: None,
        }],
        ..first
    };
    assert!(corrects(&corrected));
}

#[test]
fn opening_a_current_log_takes_no_write_lock() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("activity.db");
    let mut log = ActivityLog::open(&path).unwrap();
    log.record(&entry(100, "s1", "task_claim")).unwrap();
    drop(log);

    // Another process holds the write lock.
    let writer = rusqlite::Connection::open(&path).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    let started = std::time::Instant::now();
    let log = ActivityLog::open(&path).expect("open while another writer holds the lock");
    assert_eq!(log.entries(&Filter::default()).unwrap().len(), 1);
    assert_eq!(log.last_briefing("s1").unwrap(), None);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "waited {:?} for the write lock",
        started.elapsed()
    );
    writer.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn entries_are_indexed_by_session_and_time() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("activity.db");
    drop(ActivityLog::open(&path).unwrap());
    let conn = rusqlite::Connection::open(&path).unwrap();
    let columns: Vec<String> = conn
        .prepare("SELECT name FROM pragma_index_info('entries_session_at') ORDER BY seqno")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(columns, ["session", "at"]);
}
