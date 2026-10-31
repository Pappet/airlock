//! CLI surface (Spec 02): documented invocations must parse.

use airlock::cli::{Cli, Command, QuestionCommand, TaskCommand};
use airlock::core::TaskStatus;
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
        "E-6d",
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
        "-1",
    ]);
    let Command::Task(TaskCommand::Add(spec)) = cli.command else {
        panic!("wrong command");
    };
    assert_eq!(spec.read, ["docs/spec/missiles.md", "docs/spec/combat.md"]);
    assert_eq!(spec.blocked_by, ["L-1", "L-2"]);
    assert_eq!(spec.r#do, "Implement shot clock");
    assert_eq!(spec.priority, -1);
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
        "-2",
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
    assert_eq!(edit.priority, Some(-2));
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
