//! Problems found in the activity log: failures, clean-room sessions writing
//! through the CLI, bursts of errors, corrected answers, and airlock
//! processes older than the newest build. Pure: the caller supplies the
//! entries, the live processes and the clock-free build identity.

use crate::activity::{Channel, Entry, Process, build_time};
use crate::views::utc_minute;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// CLI commands that change project state a clean-room session should only
/// change through MCP (or leave to the lead).
pub const CLEAN_ROOM_FLAGGED: &[&str] = &[
    "task add",
    "task edit",
    "task approve",
    "task reject",
    "track add",
    "track edit",
    "track remove",
    "question answer",
    "question resolve",
    "sync",
    "import",
    "role set",
    "role remove",
    "use",
];
/// This many failures of one session within [`BURST_WINDOW_SECS`] are a burst.
pub const BURST_COUNT: usize = 5;
pub const BURST_WINDOW_SECS: i64 = 600;

const CORRECTION: &str = "correction";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProblemKind {
    /// Failed calls of one command by one session
    Failure,
    /// A clean-room session changed state through the CLI
    CleanRoomCli,
    /// Many failures of one session in a short time
    ErrorBurst,
    /// An answer was corrected after it was given
    AnswerCorrected,
    /// A live process runs an older build than the newest one
    OldProcess,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Problem {
    pub kind: ProblemKind,
    pub session: Option<String>,
    pub role: Option<String>,
    pub command: Option<String>,
    pub target: Option<String>,
    pub message: String,
    /// How many entries the problem sums up
    pub count: usize,
    pub first_at: i64,
    pub last_at: i64,
    /// The log entries behind the problem, oldest first
    pub entry_ids: Vec<u64>,
}

/// All problems in `entries` and `processes`, newest (`last_at`) first.
/// `own_build` is the build of the program asking: a process older than it, or
/// older than any other process, is an old process.
pub fn problems(entries: &[Entry], processes: &[Process], own_build: &str) -> Vec<Problem> {
    let mut entries: Vec<&Entry> = entries.iter().collect();
    entries.sort_by_key(|e| (e.at, e.id));

    let mut found = failures(&entries);
    found.extend(clean_room_cli(&entries));
    found.extend(error_bursts(&entries));
    found.extend(corrected_answers(&entries));
    found.extend(old_processes(processes, own_build));
    found.sort_by_key(|p| std::cmp::Reverse(p.last_at));
    found
}

/// A problem summing up `group` (oldest first, never empty); the caller
/// fills in what differs per kind.
fn summarize(kind: ProblemKind, group: &[&Entry], message: String) -> Problem {
    let first = group[0];
    let last = group[group.len() - 1];
    Problem {
        kind,
        session: Some(last.session.clone()),
        role: last.role.clone(),
        command: Some(last.command.clone()),
        target: common(group.iter().map(|e| e.target.as_deref())).map(str::to_string),
        message,
        count: group.len(),
        first_at: first.at,
        last_at: last.at,
        entry_ids: group.iter().map(|e| e.id).collect(),
    }
}

/// The value all items share, if they do and it exists.
fn common<'a>(mut items: impl Iterator<Item = Option<&'a str>>) -> Option<&'a str> {
    let first = items.next()??;
    items.all(|item| item == Some(first)).then_some(first)
}

/// Failed calls, one problem per (session, command); the message is the
/// latest error.
fn failures(entries: &[&Entry]) -> Vec<Problem> {
    let mut groups: BTreeMap<(&str, &str), Vec<&Entry>> = BTreeMap::new();
    for entry in entries.iter().filter(|e| !e.ok) {
        groups
            .entry((&entry.session, &entry.command))
            .or_default()
            .push(entry);
    }
    groups
        .into_values()
        .map(|group| {
            let message = group[group.len() - 1]
                .error
                .clone()
                .unwrap_or_else(|| "failed".to_string());
            summarize(ProblemKind::Failure, &group, message)
        })
        .collect()
}

/// Clean-room sessions that wrote through the CLI, one problem per session.
fn clean_room_cli(entries: &[&Entry]) -> Vec<Problem> {
    let mut groups: BTreeMap<&str, Vec<&Entry>> = BTreeMap::new();
    for entry in entries.iter().filter(|e| {
        e.channel == Channel::Cli
            && e.clean_room
            && CLEAN_ROOM_FLAGGED.contains(&e.command.as_str())
    }) {
        groups.entry(&entry.session).or_default().push(entry);
    }
    groups
        .into_iter()
        .map(|(session, group)| {
            let mut commands: Vec<&str> = Vec::new();
            for entry in &group {
                if !commands.contains(&entry.command.as_str()) {
                    commands.push(&entry.command);
                }
            }
            let message = format!(
                "{session} (clean-room) writes through the CLI: {}",
                commands.join(", ")
            );
            let mut problem = summarize(ProblemKind::CleanRoomCli, &group, message);
            if commands.len() > 1 {
                problem.command = None;
            }
            problem
        })
        .collect()
}

/// Bursts of failures: per session, every run of failures in which
/// [`BURST_COUNT`] consecutive ones span at most [`BURST_WINDOW_SECS`];
/// overlapping windows form one burst.
fn error_bursts(entries: &[&Entry]) -> Vec<Problem> {
    let mut by_session: BTreeMap<&str, Vec<&Entry>> = BTreeMap::new();
    for entry in entries.iter().filter(|e| !e.ok) {
        by_session.entry(&entry.session).or_default().push(entry);
    }
    let mut bursts = Vec::new();
    for (session, failed) in by_session {
        // Index ranges of the clusters, grown while the next window overlaps.
        let mut clusters: Vec<(usize, usize)> = Vec::new();
        for start in 0..failed.len().saturating_sub(BURST_COUNT - 1) {
            let end = start + BURST_COUNT - 1;
            if failed[end].at - failed[start].at > BURST_WINDOW_SECS {
                continue;
            }
            match clusters.last_mut() {
                Some((_, last_end)) if start <= *last_end => *last_end = end,
                _ => clusters.push((start, end)),
            }
        }
        for (start, end) in clusters {
            let group = &failed[start..=end];
            let message = format!(
                "{session}: {} failed calls between {} and {} UTC",
                group.len(),
                utc_minute(group[0].at),
                utc_minute(group[group.len() - 1].at)
            );
            let mut problem = summarize(ProblemKind::ErrorBurst, group, message);
            if common(group.iter().map(|e| Some(e.command.as_str()))).is_none() {
                problem.command = None;
            }
            bursts.push(problem);
        }
    }
    bursts
}

/// Successful answer corrections, one problem per (session, question).
fn corrected_answers(entries: &[&Entry]) -> Vec<Problem> {
    let mut groups: BTreeMap<(&str, Option<&str>), Vec<&Entry>> = BTreeMap::new();
    for entry in entries
        .iter()
        .filter(|e| e.ok && e.changed.iter().any(|c| c == CORRECTION))
    {
        groups
            .entry((&entry.session, entry.target.as_deref()))
            .or_default()
            .push(entry);
    }
    groups
        .into_iter()
        .map(|((_, target), group)| {
            let message = match target {
                Some(target) => format!("answer to question {target} corrected"),
                None => "answer to a question corrected".to_string(),
            };
            summarize(ProblemKind::AnswerCorrected, &group, message)
        })
        .collect()
}

/// Processes whose build is older than the newest one known.
fn old_processes(processes: &[Process], own_build: &str) -> Vec<Problem> {
    let newest = processes
        .iter()
        .map(|p| build_time(&p.build))
        .chain([build_time(own_build)])
        .max()
        .unwrap_or(0);
    processes
        .iter()
        .filter(|p| build_time(&p.build) < newest)
        .map(|p| Problem {
            kind: ProblemKind::OldProcess,
            session: p.session.clone(),
            role: p.role.clone(),
            command: None,
            target: None,
            message: format!(
                "old airlock process: PID {}, {}, started {}, build {}",
                p.pid,
                p.session.as_deref().unwrap_or(&p.kind),
                utc_minute(p.started_at),
                p.build
            ),
            count: 1,
            first_at: p.started_at,
            last_at: p.last_seen,
            entry_ids: Vec::new(),
        })
        .collect()
}
