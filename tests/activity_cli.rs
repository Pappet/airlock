//! CLI calls land in the activity log, driven through the real binary.

use airlock::activity::{ActivityLog, Channel, Entry, Filter};
use std::path::PathBuf;
use std::process::{Command, Output};
use tempfile::TempDir;

struct Env {
    _tmp: TempDir,
    hub: PathBuf,
    project: PathBuf,
}

impl Env {
    /// A project whose `engine` role is clean-room, with worktree `wt` bound to it.
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let hub = tmp.path().join("hub");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let env = Self {
            _tmp: tmp,
            hub,
            project,
        };
        env.ok(&["init", "--name", "demo"]);
        env.ok(&["role", "set", "engine", "--clean-room", "true"]);
        env
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_airlock"));
        cmd.args(args)
            .current_dir(&self.project)
            .env("AIRLOCK_HOME", &self.hub)
            .env_remove("AIRLOCK_PROJECT")
            .env_remove("AIRLOCK_SESSION")
            .env_remove("AIRLOCK_ROLE")
            .env_remove("AI_AGENT")
            .env_remove("CLAUDECODE");
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn project_dir(&self) -> PathBuf {
        std::fs::read_dir(self.hub.join("projects"))
            .unwrap()
            .next()
            .expect("a project in the hub")
            .unwrap()
            .path()
    }

    /// Entries oldest first.
    fn entries(&self) -> Vec<Entry> {
        let log = ActivityLog::open(&self.project_dir().join("activity.db")).unwrap();
        let mut entries = log.entries(&Filter::default()).unwrap();
        entries.reverse();
        entries
    }

    /// The entries recorded after the `init`/`role set` setup calls.
    fn entries_after(&self, skip: usize) -> Vec<Entry> {
        self.entries().into_iter().skip(skip).collect()
    }
}

#[test]
fn cli_commands_are_logged_with_binding_session_and_role() {
    let env = Env::new();
    env.ok(&["use", "engine", "--session", "engine-1"]);
    let before = env.entries().len();
    env.ok(&["task", "list"]);
    env.ok(&[
        "task",
        "add",
        "Engine",
        "--role",
        "engine",
        "--do",
        "x",
        "--verify",
        "y",
        "--priority",
        "high",
    ]);
    let entries = env.entries_after(before);
    assert_eq!(entries.len(), 2, "{entries:?}");

    let list = &entries[0];
    assert_eq!(list.command, "task list");
    assert!(!list.write);
    assert_eq!(list.session, "engine-1");
    assert_eq!(list.role.as_deref(), Some("engine"));
    assert!(list.clean_room);
    assert_eq!(list.channel, Channel::Cli);
    assert!(list.ok);
    assert_ne!(list.pid, 0);
    assert_eq!(list.build, airlock::activity::build());

    let add = &entries[1];
    assert_eq!(add.command, "task add");
    assert!(add.write);
    assert_eq!(add.session, "engine-1");
    assert_eq!(add.role.as_deref(), Some("engine"));
    assert!(add.clean_room);
    assert_eq!(add.target, None);
    assert_eq!(
        add.changed,
        ["title", "role: engine", "do", "verify", "priority: high"]
    );
}

#[test]
fn use_is_logged_with_the_session_it_binds() {
    let env = Env::new();
    let before = env.entries().len();
    env.ok(&["use", "engine", "--session", "engine-7", "--lock"]);
    env.ok(&["use"]);
    env.ok(&["use", "--clear", "--force"]);
    assert!(!env.run(&["use", "ghost"]).status.success());
    let entries = env.entries_after(before);
    assert_eq!(entries.len(), 4, "{entries:?}");
    assert!(!entries[3].ok, "{:?}", entries[3]);
    assert!(entries[3].error.as_deref().unwrap().contains("ghost"));

    let bind = &entries[0];
    assert_eq!(bind.command, "use");
    assert!(bind.write);
    // Showing the binding is a read; clearing it is a write.
    assert!(!entries[1].write, "{:?}", entries[1]);
    assert!(entries[2].write, "{:?}", entries[2]);
    assert_eq!(bind.session, "engine-7");
    assert_eq!(bind.role.as_deref(), Some("engine"));
    assert!(bind.clean_room);
    assert_eq!(bind.target.as_deref(), Some("engine"));
    assert_eq!(bind.changed, ["lock: true"]);

    // Showing and clearing fall back to the binding that exists.
    assert_eq!(entries[1].session, "engine-7");
    assert_eq!(entries[1].role.as_deref(), Some("engine"));
    assert_eq!(entries[2].session, "engine-7");
    assert_eq!(entries[2].changed, ["force", "clear"]);
}

#[test]
fn start_and_task_next_log_their_role_flag() {
    let env = Env::new();
    let before = env.entries().len();
    env.ok(&["start", "--role", "engine", "--session", "s1"]);
    // No ready task for `lead`: the call fails, and is still logged under that role.
    env.run(&["task", "next", "--role", "lead", "--session", "s1"]);
    let entries = env.entries_after(before);
    assert_eq!(entries[0].command, "start");
    assert_eq!(entries[0].role.as_deref(), Some("engine"));
    assert!(entries[0].clean_room);
    assert_eq!(entries[0].changed, ["role: engine"]);
    assert_eq!(entries[1].role.as_deref(), Some("lead"));
    assert!(!entries[1].clean_room);
}

#[test]
fn cli_failure_is_logged_with_exact_error() {
    let env = Env::new();
    let before = env.entries().len();
    let out = env.run(&["task", "claim", "E-99"]);
    assert!(!out.status.success());
    let entries = env.entries_after(before);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].command, "task claim");
    assert!(!entries[0].ok);
    assert_eq!(entries[0].error.as_deref(), Some("no task `E-99`"));
    assert_eq!(entries[0].target.as_deref(), Some("E-99"));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no task `E-99`"),
        "the command's own error still reaches the user"
    );
}

#[test]
fn flags_are_logged_by_name_with_enumerated_values() {
    let env = Env::new();
    let before = env.entries().len();
    env.ok(&[
        "track",
        "add",
        "core",
        "--priority",
        "high",
        "--description",
        "Words",
    ]);
    env.ok(&["track", "edit", "core", "--name", "kernel"]);
    env.ok(&["question", "ask", "Why?", "--topic", "a topic"]);
    env.ok(&["question", "answer", "1", "Because", "--spec", "docs/x.md"]);
    env.ok(&[
        "role",
        "set",
        "qa",
        "--clean-room",
        "false",
        "--prefix",
        "Q",
    ]);
    env.ok(&["task", "list", "--no-track", "--status", "in_progress"]);
    let entries = env.entries_after(before);
    let summary: Vec<_> = entries
        .iter()
        .map(|e| {
            let changed: Vec<&str> = e.changed.iter().map(String::as_str).collect();
            (e.command.as_str(), e.target.as_deref(), changed)
        })
        .collect();
    assert_eq!(
        summary,
        [
            (
                "track add",
                Some("core"),
                vec!["priority: high", "description"]
            ),
            ("track edit", Some("core"), vec!["name"]),
            ("question ask", None, vec!["question", "topic"]),
            ("question answer", Some("#1"), vec!["answer", "spec_file"]),
            ("role set", Some("qa"), vec!["clean_room: false", "prefix"]),
            ("task list", None, vec!["status: in_progress", "no_track"]),
        ]
    );
}

#[test]
fn agent_shell_is_recorded() {
    let env = Env::new();
    let before = env.entries().len();
    let mut with = env.command(&["task", "list"]);
    with.env("AI_AGENT", "claude-code_x");
    assert!(with.output().unwrap().status.success());
    env.ok(&["task", "list"]);
    let entries = env.entries_after(before);
    assert_eq!(entries[0].agent.as_deref(), Some("claude-code"));
    assert_eq!(entries[1].agent, None);
}

#[test]
fn cli_log_holds_no_free_text() {
    let env = Env::new();
    env.ok(&[
        "task",
        "add",
        "SECRET-TITLE",
        "--role",
        "engine",
        "--do",
        "SECRET-DO-TEXT",
        "--verify",
        "SECRET-VERIFY",
    ]);
    env.ok(&[
        "question",
        "ask",
        "SECRET-QUESTION",
        "--topic",
        "SECRET-TOPIC",
    ]);
    env.ok(&["question", "answer", "1", "SECRET-ANSWER"]);
    env.ok(&[
        "role",
        "set",
        "qa",
        "--description",
        "SECRET-DESC",
        "--instructions",
        "SECRET-RULES",
    ]);
    env.ok(&["track", "add", "t", "--description", "SECRET-TRACK"]);
    env.ok(&["task", "claim", "E-1"]);
    env.ok(&["task", "heartbeat", "E-1", "--notes", "SECRET-PROGRESS"]);
    assert!(env.entries().iter().all(|e| e.ok));
    let heartbeat = env.entries().pop().unwrap();
    assert_eq!(heartbeat.command, "task heartbeat");
    assert_eq!(heartbeat.changed, ["notes"]);

    let mut scanned = Vec::new();
    for file in std::fs::read_dir(env.project_dir()).unwrap() {
        let file = file.unwrap().path();
        let name = file.file_name().unwrap().to_string_lossy().to_string();
        if !name.starts_with("activity.db") {
            continue;
        }
        let bytes = std::fs::read(&file).unwrap();
        assert!(
            !bytes.windows(6).any(|w| w == b"SECRET"),
            "{name} holds free text"
        );
        scanned.push(name);
    }
    assert!(scanned.contains(&"activity.db".to_string()), "{scanned:?}");
}

#[test]
fn unwritable_activity_log_never_fails_a_command() {
    let env = Env::new();
    let db = env.project_dir().join("activity.db");
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", db.display()));
    }
    std::fs::create_dir(&db).unwrap();
    let out = env.run(&["task", "list"]);
    assert!(out.status.success(), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("airlock: activity log:"), "{stderr}");
    assert_eq!(stderr.matches("airlock: activity log:").count(), 1);
}

#[test]
fn unlogged_commands_add_no_entry() {
    let env = Env::new();
    let before = env.entries().len();
    env.ok(&["doctor"]);
    env.ok(&["setup", "claude", "--role", "engine"]);
    assert_eq!(env.entries().len(), before);
}

#[test]
fn activity_command_is_not_logged() {
    let env = Env::new();
    env.ok(&["task", "list"]);
    let before = env.entries().len();
    env.ok(&["activity"]);
    assert_eq!(env.entries().len(), before);
}

#[test]
fn answering_an_answered_question_is_logged_as_a_correction() {
    let env = Env::new();
    env.ok(&["question", "ask", "Why?", "--topic", "plan.md"]);
    env.ok(&["question", "answer", "1", "first"]);
    env.ok(&["question", "resolve", "1"]);
    env.ok(&["question", "answer", "1", "second"]);
    let answers: Vec<Entry> = env
        .entries()
        .into_iter()
        .filter(|e| e.command == "question answer")
        .collect();
    assert_eq!(answers.len(), 2, "{answers:?}");
    assert_eq!(answers[0].changed, ["answer"]);
    assert_eq!(answers[1].changed, ["answer", "correction"]);
    assert!(answers[1].ok);
}

#[test]
fn activity_json_lists_entries_and_problems() {
    let env = Env::new();
    env.ok(&["task", "list"]);
    assert!(!env.run(&["task", "claim", "E-99"]).status.success());

    let failures: Vec<Entry> =
        serde_json::from_str(&env.ok(&["--json", "activity", "--failures"])).unwrap();
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(failures[0].command, "task claim");
    assert_eq!(failures[0].error.as_deref(), Some("no task `E-99`"));

    let all: Vec<Entry> = serde_json::from_str(&env.ok(&["--json", "activity"])).unwrap();
    assert!(all.len() >= 3, "{all:?}");
    assert!(all[0].id > all[1].id, "newest first");
    let limited: Vec<Entry> =
        serde_json::from_str(&env.ok(&["--json", "activity", "--limit", "1"])).unwrap();
    assert_eq!(limited, &all[..1]);
    let writes: Vec<Entry> =
        serde_json::from_str(&env.ok(&["--json", "activity", "--writes"])).unwrap();
    assert!(writes.iter().all(|e| e.write) && writes.len() < all.len());
    let channel: Vec<Entry> =
        serde_json::from_str(&env.ok(&["--json", "activity", "--channel", "rest"])).unwrap();
    assert!(channel.is_empty());

    let problems: Vec<serde_json::Value> =
        serde_json::from_str(&env.ok(&["--json", "activity", "--problems"])).unwrap();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0]["kind"], "failure");
    assert_eq!(problems[0]["command"], "task claim");
    // --session and --role narrow the problems afterwards.
    let session = failures[0].session.clone();
    let mine: Vec<serde_json::Value> =
        serde_json::from_str(&env.ok(&["--json", "activity", "--problems", "--session", &session]))
            .unwrap();
    assert_eq!(mine.len(), 1);
    let other: Vec<serde_json::Value> =
        serde_json::from_str(&env.ok(&["--json", "activity", "--problems", "--session", "nobody"]))
            .unwrap();
    assert!(other.is_empty());
    let by_role: Vec<serde_json::Value> =
        serde_json::from_str(&env.ok(&["--json", "activity", "--problems", "--role", "engine"]))
            .unwrap();
    assert!(by_role.is_empty());
}

#[test]
fn activity_text_has_day_headers_rows_and_problem_lines() {
    let env = Env::new();
    // Bound from an agent shell, so the clean-room rule applies to `use`.
    let bound = env
        .command(&["use", "engine", "--session", "engine-1"])
        .env("CLAUDECODE", "1")
        .output()
        .unwrap();
    assert!(bound.status.success());
    env.ok(&["task", "list"]);
    assert!(!env.run(&["task", "claim", "E-99"]).status.success());

    let text = env.ok(&["activity"]);
    let day = text.lines().next().unwrap();
    let date = day
        .strip_prefix("── ")
        .and_then(|d| d.strip_suffix(" (UTC)"))
        .unwrap_or_else(|| panic!("{day:?}"));
    assert_eq!(date.len(), "YYYY-MM-DD".len(), "{day:?}");
    assert!(date.starts_with("20"), "{day:?}");
    let row = text
        .lines()
        .find(|l| l.contains("task claim"))
        .unwrap_or_else(|| panic!("{text}"));
    let columns: Vec<&str> = row.split("  ").collect();
    assert_eq!(columns[0].len(), 5, "HH:MM in {row:?}");
    assert_eq!(&columns[0][2..3], ":");
    assert_eq!(columns[1], "engine-1");
    assert_eq!(columns[2], "CLI");
    assert_eq!(columns[3], "task claim E-99");
    assert_eq!(columns[4], "✖ no task `E-99`");
    let list = text.lines().find(|l| l.contains("task list")).unwrap();
    assert!(!list.contains('✖'), "{list}");
    assert_eq!(
        text.matches("── ").count(),
        1,
        "one day header for one day: {text}"
    );

    let problems = env.ok(&["activity", "--problems"]);
    // The failure, and the clean-room session that bound itself through the CLI.
    assert_eq!(problems.lines().count(), 2, "{problems}");
    let failure = problems
        .lines()
        .find(|l| l.contains("  failure  "))
        .unwrap_or_else(|| panic!("{problems}"));
    let columns: Vec<&str> = failure.split("  ").collect();
    assert_eq!(
        columns[0].len(),
        "YYYY-MM-DD HH:MM UTC".len(),
        "{failure:?}"
    );
    assert!(columns[0].ends_with(" UTC"), "{failure:?}");
    assert_eq!(columns[2], "engine-1");
    assert_eq!(columns[3], "no task `E-99`");
    assert!(problems.contains("  clean_room_cli  "), "{problems}");
}

#[test]
fn activity_without_a_log_is_empty() {
    let env = Env::new();
    let db = env.project_dir().join("activity.db");
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", db.display()));
    }
    assert_eq!(env.ok(&["--json", "activity"]).trim(), "[]");
    assert_eq!(env.ok(&["--json", "activity", "--problems"]).trim(), "[]");
    assert!(env.ok(&["activity"]).contains("No activity"));
    assert!(env.ok(&["activity", "--problems"]).contains("No problems"));
    assert!(!db.exists(), "reading must not create the log");
}

#[test]
fn activity_rejects_a_bad_since() {
    let env = Env::new();
    let out = env.run(&["activity", "--since", "yesterday"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("30m") && stderr.contains("2h"), "{stderr}");
}
