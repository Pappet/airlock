//! CLI surface (Spec 02): documented invocations must parse.

use airlock::cli::{Cli, Command, QuestionCommand, TaskCommand, TrackCommand};
use airlock::core::{Priority, TaskStatus};
use clap::{CommandFactory, Parser};

fn parse(args: &[&str]) -> Cli {
    Cli::try_parse_from(std::iter::once("airlock").chain(args.iter().copied()))
        .unwrap_or_else(|e| panic!("{args:?} failed to parse:\n{e}"))
}

#[test]
fn definition_is_consistent() {
    Cli::command().debug_assert();
}

#[test]
fn task_add_accepts_repeated_and_comma_separated_lists() {
    let cli = parse(&[
        "task",
        "add",
        "Missiles implementation",
        "--role",
        "engine",
        "--read",
        "docs/spec/missiles.md",
        "--read",
        "docs/spec/combat.md",
        "--touch",
        "src/missiles.rs",
        "--do",
        "Implement shot clock",
        "--verify",
        "cargo test missiles",
        "--blocked-by",
        "L-1,L-2",
        "--priority",
        "low",
    ]);
    let Command::Task(TaskCommand::Add(spec)) = cli.command else {
        panic!("wrong command");
    };
    assert_eq!(spec.title, "Missiles implementation");
    assert_eq!(spec.read, ["docs/spec/missiles.md", "docs/spec/combat.md"]);
    assert_eq!(spec.blocked_by, ["L-1", "L-2"]);
    assert_eq!(spec.r#do, "Implement shot clock");
    assert_eq!(spec.priority, Priority::Low);
}

#[test]
fn task_add_priority_takes_a_level_not_a_number() {
    let cli = parse(&[
        "task",
        "add",
        "T",
        "--role",
        "engine",
        "--do",
        "x",
        "--verify",
        "y",
        "--priority",
        "high",
    ]);
    let Command::Task(TaskCommand::Add(spec)) = cli.command else {
        panic!("wrong command");
    };
    assert_eq!(spec.priority, Priority::High);
    let numeric = Cli::try_parse_from([
        "airlock",
        "task",
        "add",
        "T",
        "--role",
        "engine",
        "--do",
        "x",
        "--verify",
        "y",
        "--priority",
        "5",
    ]);
    assert!(numeric.is_err(), "numeric priorities are rejected");
}

#[test]
fn task_add_takes_no_id() {
    let own_id = Cli::try_parse_from([
        "airlock", "task", "add", "E-6d", "Missiles", "--role", "engine", "--do", "x", "--verify",
        "y",
    ]);
    assert!(own_id.is_err(), "ids are assigned by airlock");
}

#[test]
fn role_set_takes_a_prefix() {
    let cli = parse(&["role", "set", "simulation", "--prefix", "SIM"]);
    let Command::Role(airlock::cli::RoleCommand::Set { prefix, .. }) = cli.command else {
        panic!("wrong command");
    };
    assert_eq!(prefix.as_deref(), Some("SIM"));
}

#[test]
fn task_edit_takes_optional_fields_and_clear_flags() {
    let cli = parse(&[
        "task",
        "edit",
        "E-6d",
        "--title",
        "Missiles",
        "--touch",
        "src/a.rs",
        "--touch",
        "src/b.rs",
        "--blocked-by",
        "L-1,L-2",
        "--clear-read",
        "--priority",
        "critical",
        "--force",
    ]);
    let Command::Task(TaskCommand::Edit(edit)) = cli.command else {
        panic!("wrong command");
    };
    assert_eq!(edit.id, "E-6d");
    assert_eq!(edit.title.as_deref(), Some("Missiles"));
    assert_eq!(edit.touch, ["src/a.rs", "src/b.rs"]);
    assert_eq!(edit.blocked_by, ["L-1", "L-2"]);
    assert!(edit.clear_read && !edit.clear_touch && !edit.clear_blocked_by);
    assert_eq!(edit.priority, Some(Priority::Critical));
    assert_eq!(edit.role, None);
    assert!(edit.force);

    let conflict = Cli::try_parse_from([
        "airlock",
        "task",
        "edit",
        "E-6d",
        "--read",
        "a",
        "--clear-read",
    ]);
    assert!(
        conflict.is_err(),
        "a list cannot be replaced and cleared at once"
    );
}

#[test]
fn global_flags_work_after_the_subcommand() {
    let cli = parse(&[
        "task",
        "claim",
        "E-6d",
        "--session",
        "engine-1",
        "--lease",
        "30",
        "--json",
    ]);
    assert_eq!(cli.session.as_deref(), Some("engine-1"));
    assert!(cli.json);
    assert!(matches!(
        cli.command,
        Command::Task(TaskCommand::Claim {
            lease: Some(30),
            ..
        })
    ));
}

#[test]
fn status_filters_use_snake_case() {
    let cli = parse(&["task", "list", "--status", "in_progress"]);
    assert!(matches!(
        cli.command,
        Command::Task(TaskCommand::List {
            status: Some(TaskStatus::InProgress),
            ..
        })
    ));
}

#[test]
fn documented_commands_parse() {
    for args in [
        &["init"][..],
        &["task", "next", "--role", "engine"],
        &[
            "task",
            "done",
            "E-6d",
            "--notes",
            "All tests pass",
            "--review",
        ],
        &["task", "release", "E-6d", "--force"],
        &["task", "reject", "E-6d", "--notes", "Missing edge case"],
        &[
            "question",
            "ask",
            "How does blast radius degrade?",
            "--topic",
            "missiles.md",
        ],
        &[
            "question",
            "answer",
            "1",
            "Linearly.",
            "--spec",
            "docs/spec/missiles.md",
        ],
        &["question", "list", "--status", "open"],
        &["mcp", "--role", "engine"],
        &["sync", "--force"],
        &["import", "--yes"],
    ] {
        parse(args);
    }
}

#[test]
fn command_names_match_invocation() {
    let cli = parse(&["question", "resolve", "3"]);
    assert!(matches!(
        cli.command,
        Command::Question(QuestionCommand::Resolve { id: 3, .. })
    ));
    assert_eq!(cli.command.name(), "question resolve");
}

#[test]
fn track_commands_parse() {
    let cli = parse(&[
        "track",
        "add",
        "comfort",
        "--priority",
        "low",
        "--description",
        "Could build",
    ]);
    let Command::Track(TrackCommand::Add {
        name,
        priority,
        description,
    }) = cli.command
    else {
        panic!("wrong command");
    };
    assert_eq!(
        (name.as_str(), priority, description.as_str()),
        ("comfort", Priority::Low, "Could build")
    );

    let cli = parse(&["track", "add", "core"]);
    let Command::Track(TrackCommand::Add {
        priority,
        description,
        ..
    }) = cli.command
    else {
        panic!("wrong command");
    };
    assert_eq!((priority, description.as_str()), (Priority::Normal, ""));

    let cli = parse(&[
        "track",
        "edit",
        "comfort",
        "--name",
        "polish",
        "--priority",
        "high",
    ]);
    let Command::Track(TrackCommand::Edit {
        name,
        new_name,
        priority,
        description,
    }) = cli.command
    else {
        panic!("wrong command");
    };
    assert_eq!(name, "comfort");
    assert_eq!(new_name.as_deref(), Some("polish"));
    assert_eq!(priority, Some(Priority::High));
    assert_eq!(description, None);

    let cli = parse(&["track", "remove", "comfort"]);
    assert!(matches!(
        cli.command,
        Command::Track(TrackCommand::Remove { .. })
    ));
    assert_eq!(cli.command.name(), "track remove");
    assert_eq!(parse(&["track", "list"]).command.name(), "track list");
    assert_eq!(parse(&["track", "add", "x"]).command.name(), "track add");
    assert_eq!(parse(&["track", "edit", "x"]).command.name(), "track edit");
}

#[test]
fn task_list_filters_by_track_and_priority() {
    let cli = parse(&["task", "list", "--track", "core", "--priority", "high"]);
    let Command::Task(TaskCommand::List {
        track,
        no_track,
        priority,
        ..
    }) = cli.command
    else {
        panic!("wrong command");
    };
    assert_eq!(track.as_deref(), Some("core"));
    assert!(!no_track);
    assert_eq!(priority, Some(Priority::High));

    let cli = parse(&["task", "list", "--no-track"]);
    let Command::Task(TaskCommand::List { no_track, .. }) = cli.command else {
        panic!("wrong command");
    };
    assert!(no_track);
    assert!(
        Cli::try_parse_from(["airlock", "task", "list", "--track", "x", "--no-track"]).is_err(),
        "a track and no track conflict"
    );
}

#[test]
fn tasks_take_and_lose_tracks() {
    let cli = parse(&[
        "task", "add", "T", "--role", "engine", "--do", "x", "--verify", "y", "--track", "core",
    ]);
    let Command::Task(TaskCommand::Add(spec)) = cli.command else {
        panic!("wrong command");
    };
    assert_eq!(spec.track.as_deref(), Some("core"));

    let cli = parse(&["task", "edit", "E-1", "--clear-track"]);
    let Command::Task(TaskCommand::Edit(edit)) = cli.command else {
        panic!("wrong command");
    };
    assert!(edit.clear_track && edit.track.is_none());

    let cli = parse(&["task", "edit", "E-1", "--track", "core"]);
    let Command::Task(TaskCommand::Edit(edit)) = cli.command else {
        panic!("wrong command");
    };
    assert_eq!(edit.track.as_deref(), Some("core"));

    assert!(
        Cli::try_parse_from([
            "airlock",
            "task",
            "edit",
            "E-1",
            "--track",
            "x",
            "--clear-track"
        ])
        .is_err(),
        "a track cannot be set and cleared at once"
    );
}
