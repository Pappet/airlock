//! Command execution: resolves project and session, runs the operation, prints the result.

use crate::cli::{Cli, Command, QuestionCommand, TaskCommand, TaskSpec};
use crate::config::ProjectConfig;
use crate::core::{Question, Task, TaskState, TaskStatus, dependents};
use crate::hub::{Hub, MARKER_FILE, Marker, Project};
use crate::store::{Completion, NewTask, Store};
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use serde_json::json;
use std::collections::HashSet;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Runs one parsed invocation against the hub at `$AIRLOCK_HOME` or the platform default.
pub fn run(cli: Cli) -> Result<()> {
    let hub = Hub::locate()?;
    let cwd = std::env::current_dir().context("reading the current directory")?;
    let out = Output { json: cli.json };
    let now = unix_now();

    let command = match cli.command {
        Command::Init { name, template } => return init(&hub, &cwd, name, &template, &out),
        command => command,
    };

    let project = hub.resolve(cli.project.as_deref(), &cwd)?;
    let session = cli.session.unwrap_or_else(default_session);
    let mut store = Store::open(&project.database)?;

    match command {
        Command::Init { .. } => unreachable!("handled above"),
        Command::Task(sub) => task(&mut store, &project, &session, sub, now, &out),
        Command::Question(sub) => question(&mut store, &session, sub, now, &out),
        other => bail!(
            "`airlock {}` is not implemented yet; see docs/plans/PHASE_PLAN.md",
            other.name()
        ),
    }
}

fn init(hub: &Hub, cwd: &Path, name: Option<String>, template: &str, out: &Output) -> Result<()> {
    ensure!(
        template == "default",
        "unknown template `{template}`; available: default"
    );

    let existing = Marker::read(cwd)?;
    if existing.is_none() {
        if let Some((dir, _)) = Marker::find(cwd)? {
            bail!(
                "{} is already inside the Airlock project at {}",
                cwd.display(),
                dir.display()
            );
        }
    }

    if let Some(marker) = &existing {
        if hub.contains(&marker.project_id) {
            let config = hub.load_config(&marker.project_id)?;
            return out.print(
                &json!({ "project_id": marker.project_id, "name": config.name, "created": false }),
                || {
                    format!(
                        "Already initialized: project {} ({})",
                        config.name, marker.project_id
                    )
                },
            );
        }
    }

    let name = match name {
        Some(name) => name,
        None => cwd
            .file_name()
            .and_then(|n| n.to_str())
            .context("cannot derive a project name from the directory; pass --name")?
            .to_string(),
    };
    ensure!(
        hub.projects()?
            .iter()
            .all(|(_, config)| config.name != name),
        "a project named `{name}` already exists in the hub; choose another with --name"
    );

    let adopted = existing.is_some();
    let id = existing.map_or_else(|| uuid::Uuid::new_v4().to_string(), |m| m.project_id);
    let config = ProjectConfig {
        name: name.clone(),
        ..ProjectConfig::default()
    };
    hub.save_config(&id, &config)?;
    Store::open(&hub.database_path(&id)?)?;
    if !adopted {
        Marker {
            project_id: id.clone(),
        }
        .write(cwd)?;
    }

    let snapshot = cwd.join(&config.export.state_file);
    out.print(
        &json!({ "project_id": id, "name": name, "created": true, "adopted": adopted }),
        || {
            let mut text = if adopted {
                format!(
                    "Registered existing project {name} ({id}) in {}",
                    hub.root().display()
                )
            } else {
                format!("Initialized project {name} ({id}); commit {MARKER_FILE} to the repository")
            };
            if adopted && snapshot.is_file() {
                text.push_str(&format!(
                    "\nA snapshot exists at {}; run `airlock import` to load it",
                    snapshot.display()
                ));
            }
            text
        },
    )
}

fn task(
    store: &mut Store,
    project: &Project,
    session: &str,
    sub: TaskCommand,
    now: i64,
    out: &Output,
) -> Result<()> {
    let lease_secs = |minutes: Option<u32>| -> Result<i64> {
        let minutes = minutes.unwrap_or(project.config.lease_minutes);
        ensure!(minutes > 0, "lease must be at least one minute");
        Ok(i64::from(minutes) * 60)
    };

    match sub {
        TaskCommand::List { role, status } => {
            let tasks = store.tasks(now)?;
            let done = done_ids(&tasks);
            let views: Vec<TaskView> = tasks
                .iter()
                .map(|t| TaskView::new(t, &done))
                .filter(|v| role.as_deref().is_none_or(|r| v.task.role == r))
                .filter(|v| status.is_none_or(|s| v.status == s))
                .collect();
            out.print(&views, || task_table(&views))
        }
        TaskCommand::Next { role } => {
            ensure_role(project, &role)?;
            let tasks = store.tasks(now)?;
            let done = done_ids(&tasks);
            let next = tasks
                .iter()
                .map(|t| TaskView::new(t, &done))
                .filter(|v| v.task.role == role && v.status == TaskStatus::Ready)
                .min_by(|a, b| {
                    // Highest priority first, then oldest, then by id.
                    b.task
                        .priority
                        .cmp(&a.task.priority)
                        .then(a.task.created_at.cmp(&b.task.created_at))
                        .then(a.task.id.cmp(&b.task.id))
                })
                .with_context(|| format!("no ready task for role `{role}`"))?;
            out.print(&next, || work_package(&next))
        }
        TaskCommand::Show { id } => {
            let tasks = store.tasks(now)?;
            let task = tasks
                .iter()
                .find(|t| t.id == id)
                .with_context(|| format!("no task `{id}`"))?;
            let mut view = TaskView::new(task, &done_ids(&tasks));
            view.dependents = Some(
                dependents(&tasks, &id)
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            );
            out.print(&view, || work_package(&view))
        }
        TaskCommand::Add(spec) => {
            ensure_role(project, &spec.role)?;
            let task = store.add_task(new_task(spec), now)?;
            let tasks = store.tasks(now)?;
            let view = TaskView::new(&task, &done_ids(&tasks));
            out.print(&view, || {
                format!("Added {} ({})", task.id, view.status.as_str())
            })
        }
        TaskCommand::Claim { id, lease } => {
            let task = store.claim(&id, session, lease_secs(lease)?, now)?;
            print_lease(out, &task, "Claimed", now)
        }
        TaskCommand::Heartbeat { id, lease } => {
            let task = store.heartbeat(&id, session, lease_secs(lease)?, now)?;
            print_lease(out, &task, "Renewed", now)
        }
        TaskCommand::Release { id, force } => {
            let task = store.release(&id, session, force, now)?;
            out.print(&json!({ "task_id": task.id }), || {
                format!("Released {}", task.id)
            })
        }
        TaskCommand::Done {
            id,
            notes,
            review,
            force,
        } => {
            let completion = store.complete(&id, session, notes.as_deref(), review, force, now)?;
            print_completion(out, &completion)
        }
        TaskCommand::Approve { id, notes } => {
            let completion = store.approve(&id, notes.as_deref(), now)?;
            print_completion(out, &completion)
        }
        TaskCommand::Reject { id, notes } => {
            let task = store.reject(&id, &notes, now)?;
            out.print(&json!({ "task_id": task.id, "state": task.state }), || {
                format!("Rejected {}; it is open again", task.id)
            })
        }
    }
}

fn question(
    store: &mut Store,
    session: &str,
    sub: QuestionCommand,
    now: i64,
    out: &Output,
) -> Result<()> {
    match sub {
        QuestionCommand::List { topic, status } => {
            let questions: Vec<Question> = store
                .questions()?
                .into_iter()
                .filter(|q| topic.as_deref().is_none_or(|t| q.topic == t))
                .filter(|q| status.is_none_or(|s| q.status == s))
                .collect();
            out.print(&questions, || question_list(&questions))
        }
        QuestionCommand::Ask { question, topic } => {
            let q = store.ask(&topic, &question, session, now)?;
            out.print(&json!({ "question_id": q.id }), || {
                format!("Asked question #{} on {}", q.id, q.topic)
            })
        }
        QuestionCommand::Answer { id, answer, spec } => {
            let q = store.answer(id, &answer, session, spec.as_deref(), now)?;
            out.print(&q, || format!("Answered question #{}", q.id))
        }
        QuestionCommand::Resolve { id, spec } => {
            let q = store.resolve(id, spec.as_deref(), now)?;
            out.print(&q, || format!("Resolved question #{}", q.id))
        }
    }
}

/// A task together with its effective status, as printed by `--json`.
#[derive(Debug, Serialize)]
struct TaskView<'a> {
    #[serde(flatten)]
    task: &'a Task,
    status: TaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    dependents: Option<Vec<String>>,
}

impl<'a> TaskView<'a> {
    fn new(task: &'a Task, done: &HashSet<&str>) -> Self {
        Self {
            task,
            status: task.status(|id| done.contains(id)),
            dependents: None,
        }
    }
}

/// Text or JSON output for one command.
struct Output {
    json: bool,
}

impl Output {
    fn print(&self, value: &impl Serialize, text: impl FnOnce() -> String) -> Result<()> {
        if self.json {
            println!("{}", serde_json::to_string_pretty(value)?);
        } else {
            println!("{}", text());
        }
        Ok(())
    }
}

fn print_lease(out: &Output, task: &Task, verb: &str, now: i64) -> Result<()> {
    let until = task
        .lease_expires_at
        .context("claimed task without lease")?;
    out.print(
        &json!({ "task_id": task.id, "lease_expires_at": until }),
        || {
            let minutes = task.lease_minutes_left(now).unwrap_or(0);
            format!(
                "{verb} {} for {} (lease: {minutes} min)",
                task.id,
                task.claimed_by.as_deref().unwrap_or("?")
            )
        },
    )
}

fn print_completion(out: &Output, completion: &Completion) -> Result<()> {
    let task = &completion.task;
    out.print(
        &json!({ "task_id": task.id, "state": task.state, "now_ready": completion.now_ready }),
        || {
            let mut text = match task.state {
                TaskState::InReview => format!("Submitted {} for review", task.id),
                _ => format!("Completed {}", task.id),
            };
            if !completion.now_ready.is_empty() {
                text.push_str(&format!("\nNow ready: {}", completion.now_ready.join(", ")));
            }
            text
        },
    )
}

fn new_task(spec: TaskSpec) -> NewTask {
    NewTask {
        id: spec.id,
        title: spec.title,
        role: spec.role,
        priority: spec.priority,
        blocked_by: spec.blocked_by,
        read: spec.read,
        touch: spec.touch,
        r#do: spec.r#do,
        verify: spec.verify,
    }
}

fn ensure_role(project: &Project, role: &str) -> Result<()> {
    ensure!(
        project.config.roles.contains_key(role),
        "unknown role `{role}`; defined roles: {}",
        project
            .config
            .roles
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(())
}

fn done_ids(tasks: &[Task]) -> HashSet<&str> {
    tasks
        .iter()
        .filter(|t| t.state == TaskState::Done)
        .map(|t| t.id.as_str())
        .collect()
}

fn task_table(views: &[TaskView]) -> String {
    if views.is_empty() {
        return "No tasks.".to_string();
    }
    let rows: Vec<[String; 6]> = views
        .iter()
        .map(|v| {
            [
                v.task.id.clone(),
                v.status.as_str().to_string(),
                v.task.role.clone(),
                v.task.priority.to_string(),
                v.task.claimed_by.clone().unwrap_or_default(),
                v.task.title.clone(),
            ]
        })
        .collect();
    let header = ["ID", "STATUS", "ROLE", "PRIO", "CLAIMED BY", "TITLE"].map(str::to_string);
    let mut widths = [0; 6];
    for row in std::iter::once(&header).chain(&rows) {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.chars().count());
        }
    }
    std::iter::once(&header)
        .chain(&rows)
        .map(|row| {
            let line: Vec<String> = row
                .iter()
                .zip(widths)
                .map(|(cell, w)| format!("{cell:<w$}"))
                .collect();
            line.join("  ").trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn work_package(view: &TaskView) -> String {
    let t = view.task;
    let list = |items: &[String]| {
        if items.is_empty() {
            "—".to_string()
        } else {
            items.join(", ")
        }
    };
    let mut lines = vec![
        format!("## {} — {}", t.id, t.title),
        format!(
            "role: {} · status: {} · priority: {}",
            t.role,
            view.status.as_str(),
            t.priority
        ),
    ];
    if let Some(session) = &t.claimed_by {
        lines.push(format!("claimed by: {session}"));
    }
    lines.push(String::new());
    lines.push(format!("**Blocked by:** {}", list(&t.blocked_by)));
    if let Some(dependents) = &view.dependents {
        lines.push(format!("**Unblocks:** {}", list(dependents)));
    }
    lines.push(format!("**Read:** {}", list(&t.read)));
    lines.push(format!("**Touch:** {}", list(&t.touch)));
    lines.push(format!("**Do:** {}", t.r#do));
    lines.push(format!("**Verify:** {}", t.verify));
    if let Some(notes) = &t.notes {
        lines.push(format!("**Notes:** {notes}"));
    }
    lines.join("\n")
}

fn question_list(questions: &[Question]) -> String {
    if questions.is_empty() {
        return "No questions.".to_string();
    }
    questions
        .iter()
        .map(|q| {
            let mut text = format!(
                "#{} [{}] {} — asked by {}\n  Q: {}",
                q.id,
                q.status.as_str(),
                q.topic,
                q.asked_by,
                q.question
            );
            if let Some(answer) = &q.answer {
                text.push_str(&format!(
                    "\n  A: {answer} ({})",
                    q.answered_by.as_deref().unwrap_or("?")
                ));
            }
            if let Some(spec) = &q.spec_file {
                text.push_str(&format!("\n  Spec: {spec}"));
            }
            text
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn default_session() -> String {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".into());
    format!("human:{user}")
}

fn unix_now() -> i64 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    i64::try_from(secs).unwrap_or(i64::MAX)
}
