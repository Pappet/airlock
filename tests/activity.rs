use airlock::activity::{
    ActivityLog, Channel, ERROR_MAX, Entry, Filter, LIVE_SECS, RETENTION_SECS, agent_from_env,
    build_time, record_quietly, truncate_error,
};
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
    let id = log.register("mcp", Some("s1"), Some("backend"), 0).unwrap();

    let live = log.processes(LIVE_SECS).unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].id, id);
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
    let id = log.register("serve", None, None, 0).unwrap();
    log.touch(id, 100).unwrap();
    let live = log.processes(100 + LIVE_SECS).unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].last_seen, 100);
    assert_eq!(live[0].started_at, 0);
    log.unregister(id).unwrap();
    assert!(log.processes(100).unwrap().is_empty());
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
