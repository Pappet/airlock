//! Problems derived from activity entries built in code.

use airlock::activity::{Channel, Entry, Process};
use airlock::problems::{BURST_WINDOW_SECS, Problem, ProblemKind, problems};

fn entry(id: u64, at: i64, session: &str, channel: Channel, command: &str) -> Entry {
    Entry {
        id,
        at,
        session: session.into(),
        role: Some("engine".into()),
        clean_room: false,
        channel,
        agent: None,
        command: command.into(),
        target: None,
        changed: Vec::new(),
        write: true,
        ok: true,
        error: None,
        pid: 1,
        build: "0.1.0+100".into(),
    }
}

fn failed(id: u64, at: i64, session: &str, command: &str, error: &str) -> Entry {
    Entry {
        ok: false,
        error: Some(error.into()),
        ..entry(id, at, session, Channel::Mcp, command)
    }
}

fn process(id: u64, build: &str) -> Process {
    Process {
        id,
        pid: 4000 + id as u32,
        build: build.into(),
        kind: "mcp".into(),
        session: Some("engine-1".into()),
        role: Some("engine".into()),
        started_at: 1_700_000_000,
        last_seen: 1_700_000_060,
    }
}

fn of_kind(all: &[Problem], kind: ProblemKind) -> Vec<&Problem> {
    all.iter().filter(|p| p.kind == kind).collect()
}

#[test]
fn failures_collapse_per_session_and_command() {
    let entries = [
        failed(1, 100, "engine-1", "task_claim", "no task `E-9`"),
        failed(2, 200, "engine-1", "task_claim", "already claimed"),
        failed(3, 300, "engine-1", "task_claim", "lease held by lead"),
        failed(4, 250, "engine-1", "task_complete", "not claimed"),
        failed(5, 260, "engine-2", "task_claim", "no task `E-9`"),
    ];
    let all = problems(&entries, &[], "0.1.0+100");
    let found = of_kind(&all, ProblemKind::Failure);
    assert_eq!(found.len(), 3);
    let claims = found
        .iter()
        .find(|p| {
            p.session.as_deref() == Some("engine-1") && p.command.as_deref() == Some("task_claim")
        })
        .unwrap();
    assert_eq!(claims.count, 3);
    assert_eq!(claims.message, "lease held by lead");
    assert_eq!((claims.first_at, claims.last_at), (100, 300));
    assert_eq!(claims.entry_ids, [1, 2, 3]);
    assert_eq!(claims.role.as_deref(), Some("engine"));
    // Newest first.
    let times: Vec<i64> = all.iter().map(|p| p.last_at).collect();
    assert!(times.windows(2).all(|w| w[0] >= w[1]), "{times:?}");
}

#[test]
fn clean_room_cli_write_is_flagged_with_or_without_agent() {
    let cli = |id, command: &str, clean_room, agent: Option<&str>| Entry {
        clean_room,
        agent: agent.map(str::to_string),
        ..entry(id, 100 + id as i64, "engine-1", Channel::Cli, command)
    };
    let entries = [
        cli(1, "task add", true, None),
        cli(2, "task add", true, Some("claude-code")),
        cli(3, "question answer", true, None),
    ];
    let all = problems(&entries, &[], "0.1.0+100");
    let found = of_kind(&all, ProblemKind::CleanRoomCli);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].count, 3);
    assert_eq!(
        found[0].message,
        "engine-1 (clean-room) writes through the CLI: task add, question answer"
    );
    assert_eq!(found[0].entry_ids, [1, 2, 3]);

    let none = [
        cli(1, "task add", false, None),
        cli(2, "task claim", true, None),
        Entry {
            clean_room: true,
            ..entry(3, 100, "engine-1", Channel::Mcp, "task_create")
        },
    ];
    assert!(problems(&none, &[], "0.1.0+100").is_empty());
}

#[test]
fn five_failures_in_ten_minutes_are_a_burst() {
    let run = |step: i64| -> Vec<Entry> {
        (0..5)
            .map(|i| {
                failed(
                    i + 1,
                    1_000 + step * i as i64,
                    "engine-1",
                    "task_claim",
                    "no",
                )
            })
            .collect()
    };
    let burst = problems(&run(BURST_WINDOW_SECS / 4), &[], "0.1.0+100");
    let bursts = of_kind(&burst, ProblemKind::ErrorBurst);
    assert_eq!(bursts.len(), 1);
    assert_eq!(bursts[0].count, 5);
    assert_eq!(bursts[0].entry_ids, [1, 2, 3, 4, 5]);
    assert_eq!(bursts[0].session.as_deref(), Some("engine-1"));

    // 700 seconds from first to last: no window of five fits.
    let spread = problems(&run(175), &[], "0.1.0+100");
    assert!(of_kind(&spread, ProblemKind::ErrorBurst).is_empty());
    // The failures still show as a plain failure.
    assert_eq!(of_kind(&spread, ProblemKind::Failure).len(), 1);

    // Failures of different sessions do not add up.
    let mixed: Vec<Entry> = (0..5)
        .map(|i| {
            failed(
                i + 1,
                1_000 + i as i64,
                &format!("engine-{i}"),
                "task_claim",
                "no",
            )
        })
        .collect();
    assert!(of_kind(&problems(&mixed, &[], "0.1.0+100"), ProblemKind::ErrorBurst).is_empty());
}

#[test]
fn successful_correction_is_a_problem_failed_one_is_not() {
    let correction = |id, ok: bool| Entry {
        target: Some("#3".into()),
        changed: vec!["answer".into(), "correction".into()],
        ok,
        error: (!ok).then(|| "no such question".to_string()),
        ..entry(
            id,
            100 + id as i64,
            "lead-1",
            Channel::Cli,
            "question answer",
        )
    };
    let all = problems(
        &[correction(1, true), correction(2, false)],
        &[],
        "0.1.0+100",
    );
    let found = of_kind(&all, ProblemKind::AnswerCorrected);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].message, "answer to question #3 corrected");
    assert_eq!(found[0].entry_ids, [1]);
    assert_eq!(found[0].target.as_deref(), Some("#3"));

    let plain = Entry {
        changed: vec!["answer".into()],
        ..correction(3, true)
    };
    assert!(
        of_kind(
            &problems(&[plain], &[], "0.1.0+100"),
            ProblemKind::AnswerCorrected
        )
        .is_empty()
    );
}

#[test]
fn older_live_process_is_an_old_process() {
    let processes = [process(1, "0.1.0+100"), process(2, "0.1.0+200")];
    let all = problems(&[], &processes, "0.1.0+200");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].kind, ProblemKind::OldProcess);
    assert_eq!(
        all[0].message,
        "old airlock process: PID 4001, engine-1, started 2023-11-14 22:13, build 0.1.0+100"
    );
    assert_eq!(all[0].session.as_deref(), Some("engine-1"));

    assert_eq!(problems(&[], &processes, "0.1.0+300").len(), 2);
    // Alone and as new as the asking program: nothing to report.
    assert!(problems(&[], &processes[..1], "0.1.0+100").is_empty());
}

#[test]
fn problems_serialize_with_snake_case_kinds() {
    let all = problems(&[], &[process(1, "0.1.0+100")], "0.1.0+300");
    assert_eq!(
        serde_json::to_value(&all[0]).unwrap()["kind"],
        "old_process"
    );
    assert_eq!(
        serde_json::to_value(ProblemKind::CleanRoomCli).unwrap(),
        "clean_room_cli"
    );
}
