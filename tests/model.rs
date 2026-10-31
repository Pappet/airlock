//! Domain model: derived status, leases, dependency graph, configuration.

mod common;

use airlock::config::ProjectConfig;
use airlock::core::{
    GraphError, TaskState, TaskStatus, check_dependencies, dependents, task_number,
};
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

#[test]
fn task_prefix_defaults_to_the_upper_cased_first_letter_of_the_role() {
    let config = ProjectConfig::default();
    assert_eq!(config.task_prefix("engine").unwrap(), "E");
    assert_eq!(config.task_prefix("lead").unwrap(), "L");
    assert_eq!(config.task_prefix("spec").unwrap(), "S");
    assert!(
        config
            .task_prefix("qa")
            .unwrap_err()
            .to_string()
            .contains("unknown role `qa`")
    );
}

#[test]
fn task_prefix_uses_the_configured_prefix() {
    let mut config = ProjectConfig::default();
    config.roles.get_mut("engine").unwrap().prefix = Some("ENG".to_string());
    assert_eq!(config.task_prefix("engine").unwrap(), "ENG");
    config.check_prefixes().unwrap();
}

#[test]
fn roles_sharing_a_prefix_are_rejected() {
    let mut config = ProjectConfig::default();
    config
        .roles
        .insert("simulation".to_string(), Default::default());
    let shared = "roles `simulation` and `spec` share the task prefix `S`";
    assert!(
        config
            .check_prefixes()
            .unwrap_err()
            .to_string()
            .contains(shared)
    );
    assert!(
        config
            .task_prefix("engine")
            .unwrap_err()
            .to_string()
            .contains(shared),
        "no task gets an id while prefixes collide"
    );

    config.roles.get_mut("simulation").unwrap().prefix = Some("SIM".to_string());
    config.check_prefixes().unwrap();
    assert_eq!(config.task_prefix("simulation").unwrap(), "SIM");
}

#[test]
fn invalid_or_underivable_prefixes_are_rejected() {
    let mut config = ProjectConfig::default();
    config.roles.get_mut("engine").unwrap().prefix = Some("E-".to_string());
    assert!(
        config
            .check_prefixes()
            .unwrap_err()
            .to_string()
            .contains("invalid task prefix `E-`")
    );

    let mut config = ProjectConfig::default();
    config.roles.insert("_x".to_string(), Default::default());
    assert_eq!(
        config.task_prefix("_x").unwrap(),
        "X",
        "first letter or digit"
    );
    config.roles.insert("__".to_string(), Default::default());
    assert!(
        config
            .check_prefixes()
            .unwrap_err()
            .to_string()
            .contains("role `__` needs a task prefix")
    );
}

#[test]
fn task_number_reads_only_prefix_dash_digits() {
    assert_eq!(task_number("E-7", "E"), Some(7));
    assert_eq!(task_number("E-007", "E"), Some(7));
    assert_eq!(task_number("SIM-12", "SIM"), Some(12));
    for (id, prefix) in [
        ("E-6d", "E"),
        ("e-7", "E"),
        ("E", "E"),
        ("E-", "E"),
        ("E--7", "E"),
        ("E-+7", "E"),
        ("EX-7", "E"),
        ("SIM-12", "S"),
        ("E-99999999999999999999999", "E"),
    ] {
        assert_eq!(task_number(id, prefix), None, "{id} under {prefix}");
    }
}
