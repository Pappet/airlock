//! Domain model: derived status, leases, dependency graph, configuration.

mod common;

use airlock::config::ProjectConfig;
use airlock::core::{GraphError, TaskState, TaskStatus, check_dependencies, dependents};
use common::task;

#[test]
fn open_task_is_ready_only_when_all_prerequisites_are_done() {
    let t = task("E-2", TaskState::Open, &["E-1", "L-1"]);
    assert_eq!(t.status(|id| id == "E-1"), TaskStatus::Blocked);
    assert_eq!(t.status(|_| true), TaskStatus::Ready);
    assert_eq!(
        task("E-3", TaskState::Open, &[]).status(|_| false),
        TaskStatus::Ready
    );
}

#[test]
fn non_open_states_map_directly() {
    for (state, status) in [
        (TaskState::InProgress, TaskStatus::InProgress),
        (TaskState::InReview, TaskStatus::InReview),
        (TaskState::Done, TaskStatus::Done),
    ] {
        assert_eq!(task("X", state, &["missing"]).status(|_| false), status);
    }
}

#[test]
fn lease_expires_at_deadline() {
    let mut t = task("E-1", TaskState::InProgress, &[]);
    t.lease_expires_at = Some(100);
    assert!(!t.lease_expired(99));
    assert!(t.lease_expired(100));

    t.state = TaskState::Done;
    assert!(!t.lease_expired(1_000));
}

#[test]
fn dependents_are_the_inverse_of_blocked_by() {
    let tasks = [
        task("A", TaskState::Done, &[]),
        task("B", TaskState::Open, &["A"]),
        task("C", TaskState::Open, &["A", "B"]),
    ];
    assert_eq!(dependents(&tasks, "A"), ["B", "C"]);
    assert_eq!(dependents(&tasks, "C"), Vec::<&str>::new());
}

fn graph(nodes: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
    nodes
        .iter()
        .map(|(id, deps)| (id.to_string(), deps.iter().map(|d| d.to_string()).collect()))
        .collect()
}

fn check(nodes: &[(String, Vec<String>)]) -> Result<(), GraphError> {
    check_dependencies(
        nodes
            .iter()
            .map(|(id, deps)| (id.as_str(), deps.as_slice())),
    )
}

#[test]
fn valid_graph_passes() {
    let g = graph(&[
        ("A", &[]),
        ("B", &["A"]),
        ("C", &["A", "B"]),
        ("D", &["C", "B"]),
    ]);
    assert_eq!(check(&g), Ok(()));
}

#[test]
fn duplicate_and_unknown_ids_are_rejected() {
    let g = graph(&[("A", &[]), ("A", &[])]);
    assert_eq!(check(&g), Err(GraphError::DuplicateId("A".into())));

    let g = graph(&[("A", &["Z"])]);
    assert_eq!(
        check(&g),
        Err(GraphError::UnknownDependency {
            task: "A".into(),
            dependency: "Z".into()
        })
    );
}

#[test]
fn cycles_are_reported_with_their_path() {
    let g = graph(&[("A", &["C"]), ("B", &["A"]), ("C", &["B"])]);
    let Err(GraphError::Cycle(path)) = check(&g) else {
        panic!("cycle not detected");
    };
    assert_eq!(path, ["A", "C", "B", "A"]);

    let g = graph(&[("A", &["A"])]);
    assert_eq!(
        check(&g),
        Err(GraphError::Cycle(vec!["A".into(), "A".into()]))
    );
}

#[test]
fn default_config_roles_are_sorted_and_engine_is_clean_room() {
    let cfg = ProjectConfig::default();
    assert_eq!(
        cfg.roles.keys().collect::<Vec<_>>(),
        ["engine", "lead", "spec"]
    );
    assert!(cfg.roles["engine"].clean_room);
    assert!(!cfg.roles["lead"].clean_room);
    assert_eq!(cfg.export.state_file, "docs/.airlock-state.json");
}

#[test]
fn config_fills_defaults_for_missing_fields() {
    let cfg: ProjectConfig =
        serde_json::from_str(r#"{"schema_version": 1, "name": "demo"}"#).expect("must parse");
    assert_eq!(cfg.lease_minutes, 60);
    assert!(cfg.roles.is_empty());
    assert_eq!(cfg.export, ProjectConfig::default().export);
}

#[test]
fn lease_minutes_left_rounds_up() {
    let mut t = task("E-1", TaskState::InProgress, &[]);
    assert_eq!(t.lease_minutes_left(0), None);
    t.lease_expires_at = Some(600);
    assert_eq!(t.lease_minutes_left(0), Some(10));
    assert_eq!(t.lease_minutes_left(1), Some(10));
    assert_eq!(t.lease_minutes_left(541), Some(1));
    assert_eq!(t.lease_minutes_left(700), Some(0));
}

#[test]
fn project_ids_must_be_canonical_uuids() {
    use airlock::hub::is_project_id;
    assert!(is_project_id("11111111-2222-4333-8444-555555555555"));
    for bad in [
        "../../escaped",
        "",
        "11111111222243338444555555555555",
        "11111111-2222-4333-8444-55555555555A",
        "{11111111-2222-4333-8444-555555555555}",
    ] {
        assert!(!is_project_id(bad), "{bad} accepted");
    }
}
