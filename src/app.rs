//! Command execution: resolves project and session, runs the operation, prints the result.

use crate::activity::{self, ActivityLog, Channel, Entry, Filter as ActivityFilter};
use crate::cli::{
    Cli, Command, QuestionCommand, RoleCommand, TaskCommand, TaskEditSpec, TaskSpec, TrackCommand,
};
use crate::core::{
    EventKind, Inbox, InboxTask, Priority, Question, Task, TaskState, TaskStatus, Track,
    cmp_task_ids, dependents,
};
use crate::hub::{Binding, Hub, MARKER_FILE, Project, binding_key};
use crate::overlap::{self, Overlap};
use crate::problems::{self, Problem};
use crate::store::{Completion, NewTask, Store, TaskEdit, TrackEdit};
use crate::{briefing, doctor, init, mcp, setup, sync, views, web};
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use serde_json::{Value, json};
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
        Command::Init {
            name,
            template,
            agents,
            yes,
        } => {
            return init::run(
                &hub,
                &cwd,
                init::Options {
                    name,
                    template,
                    agents,
                    yes,
                },
                &out,
            );
        }
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
    let name = command.name();
    let mut summary = activity::summarize_cli(&command);
    match command {
        Command::Use {
            role,
            lock,
            force,
            clear,
        } => {
            // Showing the binding (no role, no --clear) only reads.
            let write = role.is_some() || clear;
            // What the call is logged under if it binds nothing: the existing binding.
            let fallback_session = (cli.session.clone())
                .or(bound_session)
                .unwrap_or_else(default_session);
            let fallback_role = role
                .clone()
                .or_else(|| project.binding.as_ref().map(|b| b.role.clone()));
            let result = use_binding(&project, role, lock, force, clear, cli.session, &out);
            let (session, role) = match &result {
                Ok(Some(binding)) => (binding.session.clone(), Some(binding.role.clone())),
                _ => (fallback_session, fallback_role),
            };
            log_cli_call(
                &project,
                "use",
                write,
                summary,
                session,
                role,
                result.as_ref().err(),
            );
            return result.map(|_| ());
        }
        Command::Setup { harness, role } => return setup::run(&project, harness, role, &out),
        Command::Serve { port } => return web::serve(project, port),
        Command::Activity {
            role,
            channel,
            since,
            problems,
            writes,
            failures,
            limit,
        } => {
            let filter = ActivityFilter {
                session: cli.session,
                role,
                channel,
                since: since.map(|s| s.resolve(now)),
                writes_only: writes,
                failures_only: failures,
                limit: Some(limit),
            };
            return show_activity(&project, &filter, problems, now, &out);
        }
        _ => {}
    }

    let session = cli
        .session
        .or(bound_session)
        .unwrap_or_else(default_session);
    let mut store = Store::open(&project.database)?;
    // Set from the outcome: whether this call corrected an earlier answer.
    let mut correction = false;
    // Set from the outcome: how many inbox items were read.
    let mut inbox_count: Option<usize> = None;

    let caller_role = match &command {
        Command::Start {
            role: Some(role), ..
        }
        | Command::Inbox {
            role: Some(role), ..
        }
        | Command::Task(TaskCommand::Next { role: Some(role) }) => Some(role.clone()),
        _ => project.binding.as_ref().map(|b| b.role.clone()),
    };
    let logged_session = session.clone();
    let result = (|| -> Result<()> {
        match command {
            Command::Init { .. }
            | Command::Doctor
            | Command::Mcp { .. }
            | Command::Use { .. }
            | Command::Setup { .. }
            | Command::Activity { .. }
            | Command::Serve { .. } => unreachable!("handled above"),
            Command::Start { role, hints } => {
                let role = match (role, &project.binding) {
                    (Some(role), Some(b)) if b.locked && role != b.role => bail!(
                        "this worktree is locked to role `{}`; `--role {role}` is refused",
                        b.role
                    ),
                    (Some(role), _) => {
                        ensure_role(&project, &role)?;
                        Some(role)
                    }
                    (None, binding) => binding.as_ref().map(|b| b.role.clone()),
                };
                // Read before this briefing is logged, so it is the one before this.
                let previous = activity::last_briefing_quietly(&project.database, &session);
                let brief =
                    briefing::build(&store, &project, &session, role.as_deref(), previous, now)?;
                let via = match hints {
                    crate::cli::Hints::Cli => briefing::Via::Cli,
                    crate::cli::Hints::Mcp => briefing::Via::Mcp,
                };
                out.print(&brief, || brief.markdown(via))
            }
            Command::Inbox { role, peek } => {
                let role = match (role, &project.binding) {
                    (Some(role), Some(b)) if b.locked && role != b.role => bail!(
                        "this worktree is locked to role `{}`; `--role {role}` is refused",
                        b.role
                    ),
                    (Some(role), _) => {
                        ensure_role(&project, &role)?;
                        role
                    }
                    (None, Some(b)) => b.role.clone(),
                    (None, None) => bail!(
                        "no role: pass --role or bind this worktree with `airlock use <role>`"
                    ),
                };
                let coordinating = !project
                    .config
                    .roles
                    .get(&role)
                    .is_some_and(|r| r.clean_room);
                let inbox = store.inbox(&session, &role, coordinating, peek, now)?;
                inbox_count = Some(inbox.items.len());
                out.print(&inbox, || inbox_text(&role, &inbox))
            }
            Command::Role(sub) => role(&hub, &store, &project, sub, now, &out),
            Command::Task(sub) => task(&mut store, &project, &session, sub, now, &out),
            Command::Track(sub) => track(&mut store, sub, now, &out),
            Command::Question(sub) => {
                let role = project
                    .binding
                    .as_ref()
                    .filter(|b| b.session == session)
                    .map(|b| b.role.clone());
                correction = question(&mut store, &session, role.as_deref(), sub, now, &out)?;
                Ok(())
            }
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
                let (tasks, tracks, questions) = (
                    store.tasks(now)?.len(),
                    store.tracks()?.len(),
                    store.questions()?.len(),
                );
                let prompt = format!(
                    "Replace {tasks} tasks, {tracks} tracks, and {questions} questions of project {} with {} tasks, {} tracks, and {} questions from {}?",
                    project.config.name,
                    loaded.snapshot.tasks.len(),
                    loaded.snapshot.tracks.len(),
                    loaded.snapshot.questions.len(),
                    path.display()
                );
                ensure!(yes || confirm(&prompt)?, "import cancelled");
                sync::import(&mut store, &hub, &project, &loaded, now)?;
                out.print(
                    &json!({
                        "tasks": loaded.snapshot.tasks.len(),
                        "tracks": loaded.snapshot.tracks.len(),
                        "questions": loaded.snapshot.questions.len()
                    }),
                    || {
                        format!(
                            "Imported {} tasks, {} tracks, and {} questions",
                            loaded.snapshot.tasks.len(),
                            loaded.snapshot.tracks.len(),
                            loaded.snapshot.questions.len()
                        )
                    },
                )
            }
        }
    })();
    if correction && result.is_ok() {
        summary.1.push("correction".to_string());
    }
    if let Some(count) = inbox_count {
        summary.0 = Some(count.to_string());
    }
    log_cli_call(
        &project,
        name,
        activity::is_write(name),
        summary,
        logged_session,
        caller_role,
        result.as_ref().err(),
    );
    result
}

/// Records a finished CLI call in the activity log. Logging never changes the
/// call's outcome: a failure only reaches stderr.
fn log_cli_call(
    project: &Project,
    command: &str,
    write: bool,
    (target, changed): (Option<String>, Vec<String>),
    session: String,
    role: Option<String>,
    failure: Option<&anyhow::Error>,
) {
    let clean_room = role
        .as_ref()
        .and_then(|role| project.config.roles.get(role))
        .is_some_and(|role| role.clean_room);
    let error = failure.map(|e| format!("{e:#}"));
    let entry = Entry {
        id: 0,
        at: unix_now(),
        session,
        role,
        clean_room,
        channel: Channel::Cli,
        agent: activity::agent_from_env(|key| std::env::var(key).ok()),
        command: command.to_string(),
        target,
        changed,
        write,
        ok: error.is_none(),
        error,
        pid: std::process::id(),
        build: activity::build().to_string(),
    };
    activity::record_quietly(&project.database, &entry);
}

/// The project's activity log, or `None` while nothing has created it yet.
pub(crate) fn existing_activity_log(database: &Path) -> Result<Option<ActivityLog>> {
    let path = ActivityLog::path_for(database);
    if !path.exists() {
        return Ok(None);
    }
    ActivityLog::open(&path).map(Some)
}

/// The problems of the whole retention window, narrowed to `session` and
/// `role` where given. Other filters do not apply to problems.
pub(crate) fn current_problems(
    log: &mut ActivityLog,
    now: i64,
    session: Option<&str>,
    role: Option<&str>,
) -> Result<Vec<Problem>> {
    let entries = log.entries(&ActivityFilter {
        since: Some(now - activity::RETENTION_SECS),
        ..ActivityFilter::default()
    })?;
    let processes = log.processes(now)?;
    let mut found = problems::problems(&entries, &processes, activity::build());
    found.retain(|p| {
        session.is_none_or(|s| p.session.as_deref() == Some(s))
            && role.is_none_or(|r| p.role.as_deref() == Some(r))
    });
    Ok(found)
}

/// `airlock activity`: the filtered calls, or with `problems` the problems.
/// Reading is not itself logged; a missing log reads as empty.
fn show_activity(
    project: &Project,
    filter: &ActivityFilter,
    problems: bool,
    now: i64,
    out: &Output,
) -> Result<()> {
    let log = existing_activity_log(&project.database)?;
    if problems {
        let found = match log {
            Some(mut log) => current_problems(
                &mut log,
                now,
                filter.session.as_deref(),
                filter.role.as_deref(),
            )?,
            None => Vec::new(),
        };
        return out.print(&found, || problems_text(&found));
    }
    let entries = match log {
        Some(log) => log.entries(filter)?,
        None => Vec::new(),
    };
    out.print(&entries, || entries_text(&entries))
}

/// One line per call, newest first, under a `── YYYY-MM-DD (UTC)` header for
/// each day: `HH:MM  session  CHANNEL  command target changed  ✖ error`.
fn entries_text(entries: &[Entry]) -> String {
    if entries.is_empty() {
        return "No activity.".to_string();
    }
    let mut lines = Vec::new();
    let mut day = String::new();
    for e in entries {
        let stamp = views::utc_minute(e.at);
        let (date, time) = stamp.split_once(' ').unwrap_or((&stamp, ""));
        if date != day {
            lines.push(format!("── {date} (UTC)"));
            day = date.to_string();
        }
        let mut what = vec![e.command.clone()];
        what.extend(e.target.clone());
        if !e.changed.is_empty() {
            what.push(e.changed.join(", "));
        }
        let mut row = format!(
            "{time}  {}  {}  {}",
            e.session,
            format!("{:?}", e.channel).to_uppercase(),
            what.join(" ")
        );
        if !e.ok {
            let error = e.error.as_deref().unwrap_or_default();
            let error = error.split_whitespace().collect::<Vec<_>>().join(" ");
            row.push_str("  ✖");
            if !error.is_empty() {
                row.push(' ');
                row.push_str(&error);
            }
        }
        lines.push(row);
    }
    lines.join("\n")
}

/// One line per problem, newest first:
/// `YYYY-MM-DD HH:MM UTC  kind  session  message (×count)`.
fn problems_text(found: &[Problem]) -> String {
    if found.is_empty() {
        return "No problems.".to_string();
    }
    found
        .iter()
        .map(|p| {
            let kind = serde_json::to_value(p.kind)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            let times = if p.count > 1 {
                format!(" (×{})", p.count)
            } else {
                String::new()
            };
            let message = p.message.split_whitespace().collect::<Vec<_>>().join(" ");
            format!(
                "{} UTC  {kind}  {}  {message}{times}",
                views::utc_minute(p.last_at),
                p.session.as_deref().unwrap_or("-")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn use_binding(
    project: &Project,
    role: Option<String>,
    lock: bool,
    force: bool,
    clear: bool,
    session: Option<String>,
    out: &Output,
) -> Result<Option<Binding>> {
    let root = project_root(project, "use")?;
    let key = binding_key(root)?;
    let mut store = Store::open(&project.database)?;
    let now = unix_now();

    if clear {
        let removed = store.unbind(&key, force, now)?;
        out.print(&json!({ "cleared": key, "removed": removed }), || {
            if removed {
                format!("Removed the binding of {key}")
            } else {
                format!("{key} had no binding")
            }
        })?;
        return Ok(None);
    }

    let Some(role) = role else {
        if session.is_some() {
            eprintln!("note: --session / AIRLOCK_SESSION is ignored when showing a binding");
        }
        let current = store.binding(&key)?;
        out.print(&current, || match &current {
            Some(b) => format!(
                "{key} is bound to role {} as session {}{}",
                b.role,
                b.session,
                if b.locked { " (locked)" } else { "" }
            ),
            None => format!("{key} has no binding; bind it with `airlock use <role>`"),
        })?;
        return Ok(None);
    };

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
    store.bind(&key, &binding, force, now)?;
    out.print(&binding, || {
        format!(
            "Bound {key} to role {} as session {}{}",
            binding.role,
            binding.session,
            if binding.locked { " (locked)" } else { "" }
        )
    })?;
    Ok(Some(binding))
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
                        "{name}{}{}  {}",
                        r.prefix
                            .as_ref()
                            .map(|p| format!(" (prefix {p})"))
                            .unwrap_or_default(),
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
            clear_read,
            prefix,
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
            if clear_read {
                role.read.clear();
            } else if !read.is_empty() {
                role.read = read;
            }
            if let Some(prefix) = prefix {
                role.prefix = Some(prefix).filter(|p| !p.is_empty());
            }
            let role = role.clone();
            config.check_prefixes()?;
            hub.save_config(&project.id, &config)?;
            out.print(&role, || format!("Saved role {name}"))
        }
        RoleCommand::Remove { name } => {
            ensure!(config.roles.contains_key(&name), "no role `{name}`");
            let tasks = store.tasks(now)?.iter().filter(|t| t.role == name).count();
            ensure!(tasks == 0, "role `{name}` is used by {tasks} task(s)");
            let bound: Vec<String> = store
                .bindings()?
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
        TaskCommand::List {
            role,
            status,
            track,
            no_track,
            priority,
        } => {
            if let Some(track) = &track {
                store.ensure_track(track)?;
            }
            let tasks = store.tasks(now)?;
            let done = done_ids(&tasks);
            let views: Vec<TaskView> = tasks
                .iter()
                .map(|t| TaskView::new(t, &done))
                .filter(|v| role.as_deref().is_none_or(|r| v.task.role == r))
                .filter(|v| status.is_none_or(|s| v.status == s))
                .filter(|v| {
                    track
                        .as_deref()
                        .is_none_or(|t| v.task.track.as_deref() == Some(t))
                })
                .filter(|v| !no_track || v.task.track.is_none())
                .filter(|v| priority.is_none_or(|p| v.task.priority >= p))
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
            let tracks = store.tracks()?;
            let next = next_ready(&tasks, &tracks, &role)
                .with_context(|| format!("no ready task for role `{role}`"))?;
            if !next.overlaps.is_empty() {
                eprintln!(
                    "warning: {} touches files of unfinished work: {}; no ready task of role `{role}` is free of overlap. Claiming it records a question for the lead.",
                    next.task.id,
                    overlap::describe(&next.overlaps)
                );
            }
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
            let prefix = project.config.task_prefix(&spec.role)?;
            let task = store.add_task(new_task(spec, prefix), now)?;
            let tasks = store.tasks(now)?;
            let view = TaskView::new(&task, &done_ids(&tasks));
            out.print(&view, || {
                format!("Added {} ({})", task.id, view.status.as_str())
            })
        }
        TaskCommand::Edit(spec) => {
            if let Some(role) = &spec.role {
                ensure_role(project, role)?;
            }
            let id = spec.id.clone();
            let force = spec.force;
            let task = store.edit_task(&id, task_edit(spec), force, now)?;
            let tasks = store.tasks(now)?;
            let view = TaskView::new(&task, &done_ids(&tasks));
            out.print(&view, || {
                format!("Edited {} ({})", task.id, view.status.as_str())
            })
        }
        TaskCommand::Claim { id, lease } => {
            let claimed = claim(store, &id, session, lease_secs(lease)?, now)?;
            if let Some(warning) = claimed.warning() {
                eprintln!("{warning}");
            }
            out.print(&claimed.to_json()?, || {
                lease_text(&claimed.task, "Claimed", now)
            })
        }
        TaskCommand::Heartbeat { id, lease, notes } => {
            let task = store.heartbeat(&id, session, lease_secs(lease)?, notes.as_deref(), now)?;
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

fn track(store: &mut Store, sub: TrackCommand, now: i64, out: &Output) -> Result<()> {
    match sub {
        TrackCommand::List => {
            let views = TrackView::list(&store.tracks()?, &store.tasks(now)?);
            out.print(&views, || track_table(&views))
        }
        TrackCommand::Add {
            name,
            priority,
            description,
        } => {
            let track = store.add_track(&name, priority, &description, now)?;
            out.print(&track, || {
                format!("Added track {} ({})", track.name, track.priority.as_str())
            })
        }
        TrackCommand::Edit {
            name,
            new_name,
            priority,
            description,
        } => {
            let edit = TrackEdit {
                name: new_name,
                priority,
                description,
            };
            let track = store.edit_track(&name, edit, now)?;
            out.print(&track, || {
                format!("Edited track {} ({})", track.name, track.priority.as_str())
            })
        }
        TrackCommand::Remove { name } => {
            store.remove_track(&name, now)?;
            out.print(&json!({ "removed": name }), || {
                format!("Removed track {name}")
            })
        }
    }
}

/// Runs a `question` subcommand; `true` when it corrected an earlier answer.
fn question(
    store: &mut Store,
    session: &str,
    role: Option<&str>,
    sub: QuestionCommand,
    now: i64,
    out: &Output,
) -> Result<bool> {
    match sub {
        QuestionCommand::List { topic, status } => {
            let questions: Vec<Question> = store
                .questions()?
                .into_iter()
                .filter(|q| topic.as_deref().is_none_or(|t| q.topic == t))
                .filter(|q| status.is_none_or(|s| q.status == s))
                .collect();
            out.print(&questions, || question_list(&questions))?;
        }
        QuestionCommand::Ask { question, topic } => {
            let q = store.ask(&topic, &question, session, role, now)?;
            out.print(&json!({ "question_id": q.id }), || {
                format!("Asked question #{} on {}", q.id, q.topic)
            })?;
        }
        QuestionCommand::Answer { id, answer, spec } => {
            let q = store.answer(id, &answer, session, spec.as_deref(), now)?;
            out.print(&q, || format!("Answered question #{}", q.id))?;
            return Ok(activity::corrects(&q));
        }
        QuestionCommand::Resolve { id, spec } => {
            let q = store.resolve(id, spec.as_deref(), now)?;
            out.print(&q, || format!("Resolved question #{}", q.id))?;
        }
    }
    Ok(false)
}

/// A task together with its effective status, as printed by `--json`.
#[derive(Debug, Serialize)]
pub(crate) struct TaskView<'a> {
    #[serde(flatten)]
    pub task: &'a Task,
    pub status: TaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependents: Option<Vec<String>>,
    /// Shared files with occupying tasks, set by `task next` when no clean task is left
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub overlaps: Vec<Overlap>,
}

impl<'a> TaskView<'a> {
    pub fn new(task: &'a Task, done: &HashSet<&str>) -> Self {
        Self {
            task,
            status: task.status(|id| done.contains(id)),
            dependents: None,
            overlaps: Vec::new(),
        }
    }
}

/// How many tasks of a track are in each effective status.
#[derive(Debug, Default, Serialize)]
pub(crate) struct TrackCounts {
    pub blocked: usize,
    pub ready: usize,
    pub in_progress: usize,
    pub in_review: usize,
    pub done: usize,
}

/// A track with the effective status of its tasks, as printed by `track list --json`
/// and served by MCP `track_list`.
#[derive(Debug, Serialize)]
pub(crate) struct TrackView {
    #[serde(flatten)]
    pub track: Track,
    pub counts: TrackCounts,
}

impl TrackView {
    /// One view per track, in the order of `tracks`, counting from `tasks`.
    pub fn list(tracks: &[Track], tasks: &[Task]) -> Vec<Self> {
        let done = done_ids(tasks);
        let mut views: Vec<Self> = tracks
            .iter()
            .map(|track| Self {
                track: track.clone(),
                counts: TrackCounts::default(),
            })
            .collect();
        for task in tasks {
            let Some(view) = views
                .iter_mut()
                .find(|v| task.track.as_deref() == Some(v.track.name.as_str()))
            else {
                continue;
            };
            let counts = &mut view.counts;
            match task.status(|id| done.contains(id)) {
                TaskStatus::Blocked => counts.blocked += 1,
                TaskStatus::Ready => counts.ready += 1,
                TaskStatus::InProgress => counts.in_progress += 1,
                TaskStatus::InReview => counts.in_review += 1,
                TaskStatus::Done => counts.done += 1,
            }
        }
        views
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
        || lease_text(task, verb, now),
    )
}

fn lease_text(task: &Task, verb: &str, now: i64) -> String {
    let minutes = task.lease_minutes_left(now).unwrap_or(0);
    format!(
        "{verb} {} for {} (lease: {minutes} min)",
        task.id,
        task.claimed_by.as_deref().unwrap_or("?")
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

fn new_task(spec: TaskSpec, prefix: String) -> NewTask {
    NewTask {
        prefix,
        title: spec.title,
        role: spec.role,
        priority: spec.priority,
        track: spec.track,
        blocked_by: spec.blocked_by,
        read: spec.read,
        touch: spec.touch,
        r#do: spec.r#do,
        verify: spec.verify,
    }
}

/// A list flag replaces the list when given and empties it with its `--clear-*` flag.
fn replaced(list: Vec<String>, clear: bool) -> Option<Vec<String>> {
    (clear || !list.is_empty()).then_some(list)
}

fn task_edit(spec: TaskEditSpec) -> TaskEdit {
    TaskEdit {
        title: spec.title,
        role: spec.role,
        priority: spec.priority,
        track: match (spec.track, spec.clear_track) {
            (_, true) => Some(None),
            (track, false) => track.map(Some),
        },
        blocked_by: replaced(spec.blocked_by, spec.clear_blocked_by),
        read: replaced(spec.read, spec.clear_read),
        touch: replaced(spec.touch, spec.clear_touch),
        r#do: spec.r#do,
        verify: spec.verify,
    }
}

/// The next ready task of `role`: most important track first (a task without a
/// track, or with an unknown one, counts as `normal`), then task priority, then
/// oldest, then by id, skipping tasks that share files with occupying tasks.
/// When every ready task does, the first is returned with its `overlaps`
/// (Spec 01 §2.1).
pub(crate) fn next_ready<'a>(
    tasks: &'a [Task],
    tracks: &[Track],
    role: &str,
) -> Option<TaskView<'a>> {
    let track_priority = |task: &Task| {
        task.track
            .as_deref()
            .and_then(|name| tracks.iter().find(|t| t.name == name))
            .map_or(Priority::Normal, |t| t.priority)
    };
    let done = done_ids(tasks);
    let mut ready: Vec<TaskView<'a>> = tasks
        .iter()
        .map(|t| TaskView::new(t, &done))
        .filter(|v| v.task.role == role && v.status == TaskStatus::Ready)
        .collect();
    ready.sort_by(|a, b| {
        track_priority(b.task)
            .cmp(&track_priority(a.task))
            .then(b.task.priority.cmp(&a.task.priority))
            .then(a.task.created_at.cmp(&b.task.created_at))
            .then_with(|| cmp_task_ids(&a.task.id, &b.task.id))
    });
    let analysis = overlap::analyze(tasks);
    for view in &mut ready {
        view.overlaps = analysis.conflicts(&view.task.id);
    }
    let clean = ready
        .iter()
        .position(|v| v.overlaps.is_empty())
        .unwrap_or(0);
    ready.into_iter().nth(clean)
}

/// A claimed task and the files it shares with occupying tasks.
pub(crate) struct Claimed {
    pub task: Task,
    pub overlaps: Vec<Overlap>,
    /// The open overlap question for the lead, if there is an overlap
    pub question_id: Option<u64>,
    /// Why the overlap question could not be recorded; the claim stands
    pub question_error: Option<String>,
}

impl Claimed {
    /// The claim result of CLI `--json` and MCP `task_claim`.
    pub fn to_json(&self) -> Result<Value> {
        let mut value = json!({
            "task_id": self.task.id,
            "lease_expires_at": self.task.lease_expires_at,
        });
        if !self.overlaps.is_empty() {
            value["overlaps"] = serde_json::to_value(&self.overlaps)?;
        }
        if let Some(id) = self.question_id {
            value["question_id"] = id.into();
        }
        if let Some(error) = &self.question_error {
            value["question_error"] = error.as_str().into();
        }
        Ok(value)
    }

    /// The stderr warning for an overlapping claim.
    pub fn warning(&self) -> Option<String> {
        if self.overlaps.is_empty() {
            return None;
        }
        let shared = overlap::describe(&self.overlaps);
        let id = &self.task.id;
        Some(match (self.question_id, &self.question_error) {
            (Some(question), _) => format!(
                "warning: {id} touches files of unfinished work: {shared}; recorded as question #{question} for the lead"
            ),
            (None, error) => format!(
                "warning: {id} touches files of unfinished work: {shared}; could not record the question for the lead: {}",
                error.as_deref().unwrap_or("unknown error")
            ),
        })
    }
}

/// Claims a task; an overlap with occupying tasks never blocks the claim but
/// records a question so the lead can check it (Spec 01 §2.1). The claim is
/// committed first, so a failed question is reported, not returned as error.
pub(crate) fn claim(
    store: &mut Store,
    id: &str,
    session: &str,
    lease_secs: i64,
    now: i64,
) -> Result<Claimed> {
    let task = store.claim(id, session, lease_secs, now)?;
    let tasks = store.tasks(now)?;
    let analysis = overlap::analyze(&tasks);
    let overlaps = analysis.conflicts(id);
    if overlaps.is_empty() {
        return Ok(Claimed {
            task,
            overlaps,
            question_id: None,
            question_error: None,
        });
    }
    let held: Vec<String> = overlaps
        .iter()
        .map(|o| {
            let status = analysis
                .tasks
                .iter()
                .find(|t| t.id == o.id)
                .map_or("open", |t| t.status.as_str());
            let holder = tasks
                .iter()
                .find(|t| t.id == o.id)
                .and_then(|t| t.claimed_by.as_deref())
                .map(|s| format!(" by {s}"))
                .unwrap_or_default();
            format!("{} ({}, {status}{holder})", o.paths.join(", "), o.id)
        })
        .collect();
    let question = store.ask_once(
        &format!("overlap:{id}"),
        &format!(
            "{session} claimed {id} although it touches files of unfinished work: {}. Continue, or release {id}?",
            held.join("; ")
        ),
        session,
        Some(&task.role),
        now,
    );
    let (question_id, question_error) = match question {
        Ok(q) => (Some(q.id), None),
        Err(e) => (None, Some(format!("{e:#}"))),
    };
    Ok(Claimed {
        task,
        overlaps,
        question_id,
        question_error,
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
    let rows: Vec<[String; 7]> = views
        .iter()
        .map(|v| {
            [
                v.task.id.clone(),
                v.status.as_str().to_string(),
                v.task.role.clone(),
                v.task.priority.as_str().to_string(),
                v.task.track.clone().unwrap_or_default(),
                v.task.claimed_by.clone().unwrap_or_default(),
                v.task.title.clone(),
            ]
        })
        .collect();
    let header = [
        "ID",
        "STATUS",
        "ROLE",
        "PRIO",
        "TRACK",
        "CLAIMED BY",
        "TITLE",
    ]
    .map(str::to_string);
    let mut widths = [0; 7];
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

fn track_table(views: &[TrackView]) -> String {
    if views.is_empty() {
        return "No tracks. Add one with `airlock track add <name>`.".to_string();
    }
    let name_width = views
        .iter()
        .map(|v| v.track.name.chars().count())
        .max()
        .unwrap_or(0);
    views
        .iter()
        .map(|v| {
            let c = &v.counts;
            let counts: Vec<String> = [
                (c.blocked, "blocked"),
                (c.ready, "ready"),
                (c.in_progress, "running"),
                (c.in_review, "in review"),
                (c.done, "done"),
            ]
            .into_iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, label)| format!("{n} {label}"))
            .collect();
            let counts = if counts.is_empty() {
                "(no tasks)".to_string()
            } else {
                format!("({})", counts.join(", "))
            };
            let description = if v.track.description.is_empty() {
                String::new()
            } else {
                format!("{}  ", v.track.description)
            };
            format!(
                "{:<8}  {:<name_width$}  {description}{counts}",
                v.track.priority.as_str(),
                v.track.name
            )
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
    let mut header = format!(
        "role: {} · status: {} · priority: {}",
        t.role,
        view.status.as_str(),
        t.priority.as_str()
    );
    if let Some(track) = &t.track {
        header.push_str(&format!(" · track: {track}"));
    }
    let mut lines = vec![format!("## {} — {}", t.id, t.title), header];
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
    if let Some(progress) = &t.progress {
        lines.push(format!("**Progress:** {progress}"));
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
pub(crate) fn confirm(prompt: &str) -> Result<bool> {
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

/// The inbox as text: one line per item, oldest first.
pub(crate) fn inbox_text(role: &str, inbox: &Inbox) -> String {
    if inbox.items.is_empty() {
        return format!("Nothing new for {role}.");
    }
    let task_line = |t: &InboxTask, what: &str| {
        let track = t
            .track
            .as_deref()
            .map(|t| format!(", {t}"))
            .unwrap_or_default();
        format!(
            "{} {what} — {} ({}{track})",
            t.id,
            t.title,
            t.priority.as_str()
        )
    };
    inbox
        .items
        .iter()
        .filter_map(|item| {
            Some(match item.kind {
                EventKind::TaskReady => task_line(item.task.as_ref()?, "ready"),
                EventKind::TaskChanged => task_line(item.task.as_ref()?, "changed"),
                EventKind::TaskRejected => task_line(item.task.as_ref()?, "rejected in review"),
                EventKind::TaskInReview => task_line(item.task.as_ref()?, "waits for review"),
                EventKind::QuestionAsked => {
                    let q = item.question.as_ref()?;
                    format!(
                        "#{} asked on {}: {}",
                        q.id,
                        q.topic,
                        q.question.as_deref().unwrap_or("")
                    )
                }
                EventKind::QuestionAnswered => {
                    let q = item.question.as_ref()?;
                    let what = if q.corrected {
                        "answer corrected"
                    } else {
                        "answered"
                    };
                    let spec = q
                        .spec_file
                        .as_deref()
                        .map(|f| format!(" (codified in {f})"))
                        .unwrap_or_default();
                    format!(
                        "#{} {what} ({}): {}{spec}",
                        q.id,
                        q.topic,
                        q.answer.as_deref().unwrap_or("")
                    )
                }
                EventKind::TrackChanged => {
                    let t = item.track.as_ref()?;
                    format!("track {} is now {}", t.name, t.priority.as_str())
                }
            })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, priority: Priority, track: Option<&str>, created_at: i64) -> Task {
        Task {
            id: id.to_string(),
            title: format!("Task {id}"),
            role: "engine".to_string(),
            state: TaskState::Open,
            priority,
            track: track.map(str::to_string),
            blocked_by: Vec::new(),
            read: Vec::new(),
            touch: Vec::new(),
            r#do: String::new(),
            verify: String::new(),
            notes: None,
            claimed_by: None,
            lease_expires_at: None,
            progress: None,
            created_at,
            updated_at: created_at,
        }
    }

    fn track(name: &str, priority: Priority) -> Track {
        Track {
            name: name.to_string(),
            priority,
            description: String::new(),
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn next_ready_prefers_the_most_important_track() {
        let tracks = [
            track("core", Priority::Critical),
            track("comfort", Priority::Low),
        ];
        let mut tasks = vec![
            task("E-1", Priority::High, Some("comfort"), 10),
            task("E-2", Priority::Low, Some("core"), 20),
            task("E-3", Priority::Critical, None, 30),
            // Unknown track ranks as normal; created earlier than E-3, which
            // still wins on task priority.
            task("E-4", Priority::Normal, Some("gone"), 5),
        ];
        let mut order = Vec::new();
        while let Some(next) = next_ready(&tasks, &tracks, "engine") {
            let id = next.task.id.clone();
            tasks.retain(|t| t.id != id);
            order.push(id);
        }
        assert_eq!(order, ["E-2", "E-3", "E-4", "E-1"]);
    }
}
