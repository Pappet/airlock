//! Command execution: resolves project and session, runs the operation, prints the result.

use crate::cli::{Cli, Command, QuestionCommand, RoleCommand, TaskCommand, TaskSpec};
use crate::config::ProjectConfig;
use crate::core::{Question, Task, TaskState, TaskStatus, dependents};
use crate::hub::{Binding, Hub, MARKER_FILE, Marker, Project, binding_key};
use crate::store::{Completion, NewTask, Store};
use crate::{briefing, doctor, mcp, setup, sync};
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use serde_json::json;
use std::collections::HashSet;
use std::io::IsTerminal;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Runs one parsed invocation against the hub at `$AIRLOCK_HOME` or the platform default.
pub fn run(cli: Cli) -> Result<()> {
    let hub = Hub::locate()?;
    let current = std::env::current_dir().context("reading the current directory")?;
    // `-C <dir>` behaves like starting in <dir> (Spec 06 §2.4).
    let cwd = match cli.directory {
        Some(dir) => {
            let dir = current.join(dir);
            ensure!(dir.is_dir(), "-C {}: not a directory", dir.display());
            dir
        }
        None => current,
    };
    let out = Output { json: cli.json };
    let now = unix_now();

    let command = match cli.command {
        Command::Init { name, template } => return init(&hub, &cwd, name, &template, &out),
        Command::Doctor => return doctor::run(&hub, &cwd, cli.project.as_deref(), &out),
        command => command,
    };

    let project = hub.resolve(cli.project.as_deref(), &cwd)?;
    let bound_session = project.binding.as_ref().map(|b| b.session.clone());
    if let Command::Mcp { role } = command {
        let role = match (role, &project.binding) {
            (Some(role), Some(b)) if b.locked && role != b.role => bail!(
                "this worktree is locked to role `{}`; `--role {role}` is refused",
                b.role
            ),
            (Some(role), _) => Some(role),
            (None, binding) => binding.as_ref().map(|b| b.role.clone()),
        };
        // Without a binding, give each server its own session id.
        let session = cli
            .session
            .or(bound_session)
            .unwrap_or_else(|| format!("mcp-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
        eprintln!(
            "airlock mcp: project {} ({}), session {session}{}",
            project.config.name,
            project.id,
            role.as_deref()
                .map(|r| format!(", role {r}"))
                .unwrap_or_default()
        );
        let mut server = mcp::Server::new(hub, project, session, role)?;
        return server.run(std::io::stdin().lock(), std::io::stdout().lock());
    }
    match command {
        Command::Use {
            role,
            lock,
            force,
            clear,
        } => {
            return use_binding(&hub, &project, role, lock, force, clear, cli.session, &out);
        }
        Command::Setup { harness, role } => return setup::run(&project, harness, role, &out),
        _ => {}
    }

    let session = cli
        .session
        .or(bound_session)
        .unwrap_or_else(default_session);
    let mut store = Store::open(&project.database)?;

    match command {
        Command::Init { .. }
        | Command::Doctor
        | Command::Mcp { .. }
        | Command::Use { .. }
        | Command::Setup { .. } => unreachable!("handled above"),
        Command::Start => {
            let role = project.binding.as_ref().map(|b| b.role.as_str());
            let brief = briefing::build(&store, &project, &session, role, now)?;
            out.print(&brief, || brief.markdown(briefing::Via::Cli))
        }
        Command::Role(sub) => role(&hub, &store, &project, sub, now, &out),
        Command::Task(sub) => task(&mut store, &project, &session, sub, now, &out),
        Command::Question(sub) => question(&mut store, &session, sub, now, &out),
        Command::Sync { force } => {
            let root = project_root(&project, "sync")?;
            let report = sync::sync(&mut store, &project, root, force, now)?;
            out.print(&report, || {
                let mut lines: Vec<String> = report
                    .written
                    .iter()
                    .map(|p| format!("wrote     {p}"))
                    .collect();
                lines.extend(report.unchanged.iter().map(|p| format!("unchanged {p}")));
                lines.join("\n")
            })
        }
        Command::Import { from, yes } => {
            let path = match from {
                Some(path) => cwd.join(path),
                None => {
                    sync::state_path(project_root(&project, "import")?, &project.config.export)?
                }
            };
            let loaded = sync::load_snapshot(&path)?;
            sync::check_import(&loaded.snapshot, &project.id)?;
            let (tasks, questions) = (store.tasks(now)?.len(), store.questions()?.len());
            let prompt = format!(
                "Replace {tasks} tasks and {questions} questions of project {} with {} tasks and {} questions from {}?",
                project.config.name,
                loaded.snapshot.tasks.len(),
                loaded.snapshot.questions.len(),
                path.display()
            );
            ensure!(yes || confirm(&prompt)?, "import cancelled");
            sync::import(&mut store, &hub, &project, &loaded, now)?;
            out.print(
                &json!({ "tasks": loaded.snapshot.tasks.len(), "questions": loaded.snapshot.questions.len() }),
                || format!("Imported {} tasks and {} questions", loaded.snapshot.tasks.len(), loaded.snapshot.questions.len()),
            )
        }
        other => bail!(
            "`airlock {}` is not implemented yet; see docs/plans/PHASE_PLAN.md",
            other.name()
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn use_binding(
    hub: &Hub,
    project: &Project,
    role: Option<String>,
    lock: bool,
    force: bool,
    clear: bool,
    session: Option<String>,
    out: &Output,
) -> Result<()> {
    let root = project_root(project, "use")?;
    let key = binding_key(root)?;
    let mut bindings = hub.bindings(&project.id)?;
    let current = bindings.get(&key).cloned();

    if role.is_none() && !clear {
        return out.print(&current, || match &current {
            Some(b) => format!(
                "{} is bound to role {} as session {}{}",
                key,
                b.role,
                b.session,
                if b.locked { " (locked)" } else { "" }
            ),
            None => format!("{} has no binding; bind it with `airlock use <role>`", key),
        });
    }
    if let Some(b) = current.as_ref().filter(|b| b.locked && !force) {
        bail!(
            "this worktree is locked to role `{}`; pass --force to change it",
            b.role
        );
    }

    if clear {
        bindings.remove(&key);
        hub.save_bindings(&project.id, &bindings)?;
        return out.print(&json!({ "cleared": key }), || {
            format!("Removed the binding of {}", key)
        });
    }

    let role = role.expect("checked above");
    ensure_role(project, &role)?;
    let session = session.unwrap_or_else(|| {
        let dir = root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("worktree");
        format!("{role}@{dir}")
    });
    let binding = Binding {
        role,
        session,
        locked: lock,
    };
    bindings.insert(key.clone(), binding.clone());
    hub.save_bindings(&project.id, &bindings)?;
    out.print(&binding, || {
        format!(
            "Bound {} to role {} as session {}{}",
            key,
            binding.role,
            binding.session,
            if binding.locked { " (locked)" } else { "" }
        )
    })
}

fn role(
    hub: &Hub,
    store: &Store,
    project: &Project,
    sub: RoleCommand,
    now: i64,
    out: &Output,
) -> Result<()> {
    let mut config = project.config.clone();
    match sub {
        RoleCommand::List => out.print(&config.roles, || {
            config
                .roles
                .iter()
                .map(|(name, r)| {
                    let mut line = format!(
                        "{name}{}  {}",
                        if r.clean_room { " (clean-room)" } else { "" },
                        r.description
                    );
                    if !r.read.is_empty() {
                        line.push_str(&format!("\n  read: {}", r.read.join(", ")));
                    }
                    if let Some(instructions) = &r.instructions {
                        line.push_str(&format!(
                            "\n  instructions: {}",
                            instructions.replace('\n', "\n    ")
                        ));
                    }
                    line
                })
                .collect::<Vec<_>>()
                .join("\n")
        }),
        RoleCommand::Set {
            name,
            description,
            clean_room,
            instructions,
            read,
        } => {
            ensure!(
                !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')),
                "invalid role name `{name}`: use letters, digits, `-`, or `_`"
            );
            let role = config.roles.entry(name.clone()).or_default();
            if let Some(description) = description {
                role.description = description;
            }
            if let Some(clean_room) = clean_room {
                role.clean_room = clean_room;
            }
            if let Some(instructions) = instructions {
                role.instructions = Some(instructions).filter(|i| !i.trim().is_empty());
            }
            if !read.is_empty() {
                role.read = read;
            }
            let role = role.clone();
            hub.save_config(&project.id, &config)?;
            out.print(&role, || format!("Saved role {name}"))
        }
        RoleCommand::Remove { name } => {
            ensure!(config.roles.contains_key(&name), "no role `{name}`");
            let tasks = store.tasks(now)?.iter().filter(|t| t.role == name).count();
            ensure!(tasks == 0, "role `{name}` is used by {tasks} task(s)");
            let bound: Vec<String> = hub
                .bindings(&project.id)?
                .into_iter()
                .filter(|(_, b)| b.role == name)
                .map(|(path, _)| path)
                .collect();
            ensure!(
                bound.is_empty(),
                "role `{name}` is bound to {}",
                bound.join(", ")
            );
            config.roles.remove(&name);
            hub.save_config(&project.id, &config)?;
            out.print(&json!({ "removed": name }), || {
                format!("Removed role {name}")
            })
        }
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
            let role = match role.or_else(|| project.binding.as_ref().map(|b| b.role.clone())) {
                Some(role) => role,
                None => {
                    bail!("no role: pass --role or bind this worktree with `airlock use <role>`")
                }
            };
            ensure_role(project, &role)?;
            let tasks = store.tasks(now)?;
            let next = next_ready(&tasks, &role)
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
pub(crate) struct TaskView<'a> {
    #[serde(flatten)]
    pub task: &'a Task,
    pub status: TaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependents: Option<Vec<String>>,
}

impl<'a> TaskView<'a> {
    pub fn new(task: &'a Task, done: &HashSet<&str>) -> Self {
        Self {
            task,
            status: task.status(|id| done.contains(id)),
            dependents: None,
        }
    }
}

/// Text or JSON output for one command.
pub(crate) struct Output {
    pub json: bool,
}

impl Output {
    pub fn print(&self, value: &impl Serialize, text: impl FnOnce() -> String) -> Result<()> {
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

/// The next ready task of `role`: highest priority first, then oldest, then by id (Spec 01 §2.1).
pub(crate) fn next_ready<'a>(tasks: &'a [Task], role: &str) -> Option<TaskView<'a>> {
    let done = done_ids(tasks);
    tasks
        .iter()
        .map(|t| TaskView::new(t, &done))
        .filter(|v| v.task.role == role && v.status == TaskStatus::Ready)
        .min_by(|a, b| {
            b.task
                .priority
                .cmp(&a.task.priority)
                .then(a.task.created_at.cmp(&b.task.created_at))
                .then(a.task.id.cmp(&b.task.id))
        })
}

pub(crate) fn ensure_role(project: &Project, role: &str) -> Result<()> {
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

pub(crate) fn done_ids(tasks: &[Task]) -> HashSet<&str> {
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

/// The directory of the marker that selected the project; required for file exports.
pub(crate) fn project_root<'a>(project: &'a Project, command: &str) -> Result<&'a Path> {
    project.root.as_deref().with_context(|| {
        format!("`airlock {command}` needs a project root: run it inside a directory with the project's {MARKER_FILE} marker")
    })
}

/// Asks a yes/no question on the terminal; refuses without one.
fn confirm(prompt: &str) -> Result<bool> {
    ensure!(
        std::io::stdin().is_terminal(),
        "{prompt}\nNo terminal to confirm on; pass --yes"
    );
    eprint!("{prompt} [y/N] ");
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
}

fn default_session() -> String {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".into());
    format!("human:{user}")
}

pub(crate) fn unix_now() -> i64 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    i64::try_from(secs).unwrap_or(i64::MAX)
}
