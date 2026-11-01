//! `airlock hook stop`: the Claude Code Stop hook that sends a session back to
//! work when its inbox has news (Spec 06 §6). It never fails: any problem ends
//! in silence and exit 0, so a broken hub never traps a session.

use crate::activity::{self, Channel, Entry, Filter};
use crate::app::{existing_activity_log, unix_now};
use crate::core::{EventKind, Inbox, InboxItem};
use crate::hub::{Hub, Project};
use crate::store::Store;
use anyhow::Result;
use serde_json::json;
use std::io::Read;
use std::path::Path;

/// One hour: the window of the nudge limit.
const LIMIT_WINDOW_SECS: i64 = 60 * 60;

/// Runs the Stop hook for the worktree at `cwd`; prints the block decision
/// when the session should continue, nothing otherwise.
pub fn stop(hub: &Hub, cwd: &Path, project: Option<&str>) {
    // The hook JSON (`session_id`, `stop_hook_active`, …) is read and dropped:
    // Claude Code's session id is not Airlock's, and the nudge state below
    // already keeps a continued session from looping.
    let mut input = String::new();
    let _ = std::io::stdin().take(1 << 20).read_to_string(&mut input);
    if let Ok(Some(reason)) = decide(hub, cwd, project) {
        println!("{}", json!({ "decision": "block", "reason": reason }));
    }
}

/// The reason to continue, if the binding's session has news it was not yet
/// nudged about and the hourly limit allows another nudge.
fn decide(hub: &Hub, cwd: &Path, project: Option<&str>) -> Result<Option<String>> {
    let project = hub.resolve(project, cwd)?;
    let Some(binding) = project.binding.clone() else {
        return Ok(None);
    };
    let limit = project.config.auto_continue_limit;
    if limit == 0 {
        return Ok(None);
    }
    let (session, role) = (binding.session, binding.role);
    let coordinating = !project
        .config
        .roles
        .get(&role)
        .is_some_and(|r| r.clean_room);
    let now = unix_now();
    let mut store = Store::open(&project.database)?;
    let inbox = store.inbox(&session, &role, coordinating, true, now)?;
    if inbox.items.is_empty() || inbox.newest <= store.nudged_seq(&session, &role)? {
        return Ok(None);
    }
    if nudges_in_last_hour(&project, &session, now)? >= limit {
        log(&project, &session, &role, inbox.items.len(), "limit");
        return Ok(None);
    }
    store.record_nudge(&session, &role, inbox.newest, now)?;
    log(&project, &session, &role, inbox.items.len(), "nudge");
    Ok(Some(reason(&role, &inbox)))
}

fn nudges_in_last_hour(project: &Project, session: &str, now: i64) -> Result<u32> {
    // No log yet means no nudge yet; an unreadable one fails the hook (no nudge).
    let Some(log) = existing_activity_log(&project.database)? else {
        return Ok(0);
    };
    let filter = Filter {
        session: Some(session.to_string()),
        since: Some(now - LIMIT_WINDOW_SECS),
        ..Filter::default()
    };
    let count = log
        .entries(&filter)?
        .iter()
        .filter(|e| e.command == "hook stop" && e.changed.iter().any(|c| c == "nudge"))
        .count();
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

/// References only: ids, priorities, topics stay out; the agent reads the
/// content through its own `inbox` call.
fn reason(role: &str, inbox: &Inbox) -> String {
    let refs: Vec<String> = inbox.items.iter().filter_map(reference).collect();
    format!(
        "Airlock: {} new for {role}: {}. Call inbox (`airlock inbox`), then continue.",
        inbox.items.len(),
        refs.join("; ")
    )
}

fn reference(item: &InboxItem) -> Option<String> {
    let task = || {
        item.task
            .as_ref()
            .map(|t| (t.id.as_str(), t.priority.as_str()))
    };
    Some(match item.kind {
        EventKind::TaskReady => {
            let (id, priority) = task()?;
            format!("{id} ready ({priority})")
        }
        EventKind::TaskChanged => {
            let (id, priority) = task()?;
            format!("{id} changed ({priority})")
        }
        EventKind::TaskRejected => format!("{} rejected in review", task()?.0),
        EventKind::TaskInReview => format!("{} waits for review", task()?.0),
        EventKind::TaskDone => format!("{} done", task()?.0),
        EventKind::QuestionAsked => format!("question #{}", item.question.as_ref()?.id),
        EventKind::QuestionAnswered => format!("answer to #{}", item.question.as_ref()?.id),
        EventKind::TrackChanged => {
            let t = item.track.as_ref()?;
            format!("track {} now {}", t.name, t.priority.as_str())
        }
    })
}

fn log(project: &Project, session: &str, role: &str, items: usize, outcome: &str) {
    let clean_room = project.config.roles.get(role).is_some_and(|r| r.clean_room);
    let entry = Entry {
        id: 0,
        at: unix_now(),
        session: session.to_string(),
        role: Some(role.to_string()),
        clean_room,
        channel: Channel::Cli,
        agent: activity::agent_from_env(|key| std::env::var(key).ok()),
        command: "hook stop".to_string(),
        target: Some(items.to_string()),
        changed: vec![outcome.to_string()],
        write: false,
        ok: true,
        error: None,
        pid: std::process::id(),
        build: activity::build().to_string(),
    };
    activity::record_quietly(&project.database, &entry);
}
