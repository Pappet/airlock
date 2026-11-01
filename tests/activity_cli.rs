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
    assert!(env.entries().iter().all(|e| e.ok));

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
#[ignore = "`airlock activity` arrives in Task 7; un-ignore it there"]
fn activity_command_is_not_logged() {
    let env = Env::new();
    env.ok(&["task", "list"]);
    let before = env.entries().len();
    env.ok(&["activity"]);
    assert_eq!(env.entries().len(), before);
}
