//! Session briefing for `airlock start` and MCP `session_start` (Spec 06 §3).

use crate::app::next_ready;
use crate::core::{Question, QuestionStatus, Task, TaskState};
use crate::hub::Project;
use crate::store::Store;
use anyhow::Result;
use serde::Serialize;
use std::fmt::Write;

/// The compact form of a task handed to an agent.
#[derive(Debug, Clone, Serialize)]
pub struct WorkPackage {
    pub id: String,
    pub title: String,
    pub blocked_by: Vec<String>,
    pub read: Vec<String>,
    pub touch: Vec<String>,
    pub r#do: String,
    pub verify: String,
    /// Review feedback or earlier notes, if any
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl From<&Task> for WorkPackage {
    fn from(task: &Task) -> Self {
        Self {
            id: task.id.clone(),
            title: task.title.clone(),
            blocked_by: task.blocked_by.clone(),
            read: task.read.clone(),
            touch: task.touch.clone(),
            r#do: task.r#do.clone(),
            verify: task.verify.clone(),
            notes: task.notes.clone(),
        }
    }
}

/// A task of the same role held by another session.
#[derive(Debug, Clone, Serialize)]
pub struct OtherClaim {
    pub id: String,
    pub title: String,
    pub session: String,
    pub lease_minutes_left: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClaimedTask {
    #[serde(flatten)]
    pub package: WorkPackage,
    pub lease_minutes_left: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Briefing {
    pub project: String,
    pub session: String,
    pub role: Option<String>,
    pub clean_room: bool,
    pub description: Option<String>,
    pub instructions: Option<String>,
    pub read: Vec<String>,
    pub lease_minutes: u32,
    /// Tasks this session holds a lease on
    pub claimed: Vec<ClaimedTask>,
    /// The next ready package of the role, when nothing is claimed
    pub next: Option<WorkPackage>,
    /// Tasks of this role that other sessions hold, so a session can tell live work from stale leases
    pub others: Vec<OtherClaim>,
    /// The session's own questions that are answered but not resolved
    pub answered_questions: Vec<Question>,
}

/// Which interface the command hints in the Markdown refer to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    Cli,
    Mcp,
}

pub fn build(
    store: &Store,
    project: &Project,
    session: &str,
    role: Option<&str>,
    now: i64,
) -> Result<Briefing> {
    let tasks = store.tasks(now)?;
    let role_config = role.and_then(|r| project.config.roles.get(r));
    let claimed: Vec<ClaimedTask> = tasks
        .iter()
        .filter(|t| t.state == TaskState::InProgress && t.claimed_by.as_deref() == Some(session))
        .map(|t| ClaimedTask {
            package: t.into(),
            lease_minutes_left: t.lease_minutes_left(now),
        })
        .collect();
    let next = match role {
        Some(role) if claimed.is_empty() => {
            next_ready(&tasks, role).map(|v| WorkPackage::from(v.task))
        }
        _ => None,
    };
    let others = tasks
        .iter()
        .filter(|t| {
            t.state == TaskState::InProgress
                && Some(t.role.as_str()) == role
                && t.claimed_by.as_deref() != Some(session)
        })
        .map(|t| OtherClaim {
            id: t.id.clone(),
            title: t.title.clone(),
            session: t.claimed_by.clone().unwrap_or_default(),
            lease_minutes_left: t.lease_minutes_left(now),
        })
        .collect();
    let answered_questions = store
        .questions()?
        .into_iter()
        .filter(|q| q.asked_by == session && q.status == QuestionStatus::Answered)
        .collect();

    Ok(Briefing {
        project: project.config.name.clone(),
        session: session.to_string(),
        role: role.map(str::to_string),
        clean_room: role_config.is_some_and(|r| r.clean_room),
        description: role_config
            .map(|r| r.description.clone())
            .filter(|d| !d.is_empty()),
        instructions: role_config.and_then(|r| r.instructions.clone()),
        read: role_config.map(|r| r.read.clone()).unwrap_or_default(),
        lease_minutes: project.config.lease_minutes,
        claimed,
        next,
        others,
        answered_questions,
    })
}

impl Briefing {
    pub fn markdown(&self, via: Via) -> String {
        let cmd = |mcp: &str, cli: &str| match via {
            Via::Mcp => format!("`{mcp}`"),
            Via::Cli => format!("`{cli}`"),
        };
        let mut out = String::from("# Airlock session briefing\n\n");
        let _ = write!(
            out,
            "Project **{}** · session `{}`",
            self.project, self.session
        );
        match &self.role {
            Some(role) if self.clean_room => {
                let _ = writeln!(out, " · role **{role}** (clean-room)");
            }
            Some(role) => {
                let _ = writeln!(out, " · role **{role}**");
            }
            None => out.push_str(" · no role\n"),
        }
        if let Some(description) = &self.description {
            let _ = writeln!(out, "\n{description}");
        }

        if self.instructions.is_some() || !self.read.is_empty() {
            out.push_str("\n## Rules\n\n");
            if let Some(instructions) = &self.instructions {
                let _ = writeln!(out, "{}", instructions.trim_end());
            }
            if !self.read.is_empty() {
                let files: Vec<String> = self.read.iter().map(|f| format!("`{f}`")).collect();
                let _ = writeln!(out, "\nRead first: {}", files.join(", "));
            }
        }

        if self.role.is_none() {
            out.push_str(
                "\n## No role\n\nThis session has no role, so there is no work package to hand out. \
                 Bind the worktree with `airlock use <role>` or start the server with `--role`.\n",
            );
        } else if !self.claimed.is_empty() {
            out.push_str("\n## Your claimed work\n");
            for claimed in &self.claimed {
                let lease = claimed
                    .lease_minutes_left
                    .map(|m| format!(" (lease: {m} min left)"))
                    .unwrap_or_default();
                package(&mut out, &claimed.package, &lease);
            }
            let _ = writeln!(
                out,
                "\nContinue this work. Extend the lease with {} during long work; finish with {} once *Verify* passes.",
                cmd("task_heartbeat", "airlock task heartbeat <id>"),
                cmd("task_complete", "airlock task done <id> --notes \"…\"")
            );
        } else if let Some(next) = &self.next {
            out.push_str("\n## Next work package\n");
            package(&mut out, next, "");
            let _ = writeln!(
                out,
                "\nClaim it with {} before you start.",
                cmd("task_claim", &format!("airlock task claim {}", next.id))
            );
        } else {
            let _ = writeln!(
                out,
                "\n## No work ready\n\nNo task of role `{}` is ready. Ask the lead for work or check back later.",
                self.role.as_deref().unwrap_or_default()
            );
        }

        if !self.others.is_empty() {
            out.push_str("\n## In progress by other sessions of your role\n\n");
            for other in &self.others {
                let lease = other
                    .lease_minutes_left
                    .map(|m| format!(", lease {m} min left"))
                    .unwrap_or_default();
                let _ = writeln!(
                    out,
                    "- **{}** {} — `{}`{lease}",
                    other.id, other.title, other.session
                );
            }
            out.push_str(
                "\nLeave these alone: a lease stays alive only while its session sends heartbeats \
                 and becomes free on its own when it expires.\n",
            );
        }

        if !self.answered_questions.is_empty() {
            out.push_str("\n## Answers to your questions\n\n");
            for q in &self.answered_questions {
                let _ = writeln!(
                    out,
                    "- **#{} ({})** {}\n  → {}",
                    q.id,
                    q.topic,
                    q.question.trim_end().replace('\n', "\n  "),
                    q.answer
                        .as_deref()
                        .unwrap_or("")
                        .trim_end()
                        .replace('\n', "\n  ")
                );
            }
        }

        out.push_str("\n## Workflow\n\n");
        let _ = writeln!(out, "- Claim before you start; work on one task at a time.");
        let _ = writeln!(
            out,
            "- Extend the lease during long work — it expires after {} min without a heartbeat.",
            self.lease_minutes
        );
        let _ = writeln!(
            out,
            "- Complete only when the *Verify* step passes; say in the notes what you did and how you verified it."
        );
        let _ = writeln!(
            out,
            "- If a spec is ambiguous or incomplete, ask with {} instead of guessing.{}",
            cmd("question_ask", "airlock question ask \"…\" --topic <spec>"),
            if self.clean_room {
                " Never put implementation code into a question."
            } else {
                ""
            }
        );
        out
    }
}

fn package(out: &mut String, p: &WorkPackage, suffix: &str) {
    let list = |items: &[String]| {
        if items.is_empty() {
            "—".to_string()
        } else {
            items
                .iter()
                .map(|i| format!("`{i}`"))
                .collect::<Vec<_>>()
                .join(", ")
        }
    };
    let _ = writeln!(out, "\n### {} — {}{suffix}\n", p.id, p.title);
    if !p.blocked_by.is_empty() {
        let _ = writeln!(out, "- **Blocked by:** {}", p.blocked_by.join(", "));
    }
    let _ = writeln!(out, "- **Read:** {}", list(&p.read));
    let _ = writeln!(out, "- **Touch:** {}", list(&p.touch));
    let _ = writeln!(out, "- **Do:** {}", p.r#do.trim_end().replace('\n', "\n  "));
    let _ = writeln!(
        out,
        "- **Verify:** {}",
        p.verify.trim_end().replace('\n', "\n  ")
    );
    if let Some(notes) = &p.notes {
        let _ = writeln!(
            out,
            "- **Notes:** {}",
            notes.trim_end().replace('\n', "\n  ")
        );
    }
}
