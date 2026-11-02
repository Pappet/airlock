//! Command-line interface definition (Spec 02).

use crate::activity::{Channel, Since};
use crate::core::{Priority, QuestionStatus, TaskStatus};
use crate::layout::Template;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "airlock",
    version,
    author,
    about,
    long_about = "Airlock coordinates concurrent AI agent and human developer sessions \
                  through a central task and question store shared by all worktrees \
                  of a project, with CLI and MCP interfaces."
)]
pub struct Cli {
    /// Run as if started in DIR (marker discovery, project root, worktree binding)
    #[arg(short = 'C', global = true, value_name = "DIR")]
    pub directory: Option<PathBuf>,

    /// Project id or name; overrides `.airlock` marker discovery
    #[arg(long, global = true, env = "AIRLOCK_PROJECT")]
    pub project: Option<String>,

    /// Session id for claims, questions, and answers [default: the worktree binding's, else human:$USER]
    #[arg(long, global = true, env = "AIRLOCK_SESSION")]
    pub session: Option<String>,

    /// Print machine-readable JSON instead of human-readable output
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

/// Harness hook events.
#[derive(Subcommand, Debug)]
pub enum HookEvent {
    /// Claude Code `Stop`: block the stop when the inbox has news (reads the
    /// hook JSON on stdin)
    Stop {
        /// Role whose inbox to read [default: the worktree binding's]; with
        /// `--session`, for a session in a shared checkout
        #[arg(long, env = "AIRLOCK_ROLE")]
        role: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Register this checkout as a project, apply a template, and create worktrees and bindings
    Init {
        /// Project name; defaults to the directory name
        #[arg(long)]
        name: Option<String>,
        /// Roles and layout to start from [default: asked on a terminal, else `default`]
        #[arg(long, value_enum)]
        template: Option<Template>,
        /// Number of dev worktrees for the `parallel` template
        #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u32).range(1..=16))]
        agents: u32,
        /// Do not ask; allows init to commit and create worktrees without a terminal
        #[arg(long)]
        yes: bool,
    },
    /// Bind this worktree to a role and session, or show its binding
    Use {
        /// Role to bind; omit to show the current binding
        role: Option<String>,
        /// Refuse later role changes without --force (for clean-room worktrees)
        #[arg(long, requires = "role")]
        lock: bool,
        /// Override a locked binding
        #[arg(long)]
        force: bool,
        /// Remove the binding
        #[arg(long, conflicts_with_all = ["role", "lock"])]
        clear: bool,
    },
    /// Print the session briefing: role, rules, claimed work, next package
    Start {
        /// Role to brief [default: the worktree binding's]; for a second role in a shared checkout
        #[arg(long, env = "AIRLOCK_ROLE")]
        role: Option<String>,
        /// Name commands as CLI calls or as MCP tools (for an agent that works through MCP,
        /// e.g. a Claude Code SessionStart hook)
        #[arg(long, value_enum, default_value = "cli")]
        hints: Hints,
    },
    /// Show what changed for your role since you last looked: ready or changed
    /// tasks, answers to your questions, rejected reviews, and for coordinating
    /// roles new questions and tasks in review
    Inbox {
        /// Role to read for [default: the worktree binding's]
        #[arg(long, env = "AIRLOCK_ROLE")]
        role: Option<String>,
        /// Show without marking as read
        #[arg(long, conflicts_with = "wait")]
        peek: bool,
        /// Wait until something arrives (or the timeout passes), then show it
        #[arg(long)]
        wait: bool,
        /// Seconds to wait at most with --wait
        #[arg(long, default_value_t = 240, requires = "wait")]
        timeout: u64,
    },
    /// Check the installation, worktrees, bindings, and harness configurations
    Doctor,
    /// Print the MCP configuration (or Claude Code hooks) of this worktree for an agent harness
    Setup {
        #[arg(value_enum)]
        harness: Harness,
        /// Role to configure [default: the worktree binding's]
        #[arg(long)]
        role: Option<String>,
        /// With `claude-hooks`: also a Stop hook that sends the session back to
        /// work when its inbox has news
        #[arg(long)]
        auto_continue: bool,
    },
    /// Entry points for agent harness hooks; they never fail
    #[command(subcommand, hide = true)]
    Hook(HookEvent),
    /// Role management
    #[command(subcommand)]
    Role(RoleCommand),
    /// Task operations
    #[command(subcommand)]
    Task(TaskCommand),
    /// Track operations: named lines of work with their own priority
    #[command(subcommand)]
    Track(TrackCommand),
    /// Question operations across the airlock
    #[command(subcommand)]
    Question(QuestionCommand),
    /// Run as an MCP (Model Context Protocol) server over stdio
    Mcp {
        /// Bind the server to one role; clean-room roles get a restricted tool set
        #[arg(long)]
        role: Option<String>,
    },
    /// Serve the local web dashboard and REST API on 127.0.0.1
    Serve {
        /// Port on 127.0.0.1 and ::1; 0 picks a free port
        #[arg(long, default_value_t = 3000)]
        port: u16,
    },
    /// Show the activity log: who called what, when, and what went wrong
    Activity {
        /// Only this role
        #[arg(long)]
        role: Option<String>,
        /// Only calls that came in through this channel
        #[arg(long, value_enum)]
        channel: Option<Channel>,
        /// Only calls since then: 30m, 2h, 1d, or unix seconds
        #[arg(long)]
        since: Option<Since>,
        /// List problems instead of calls: the whole 30-day log, narrowed
        /// only by --session and --role
        #[arg(long)]
        problems: bool,
        /// Only calls that change something
        #[arg(long)]
        writes: bool,
        /// Only calls that failed
        #[arg(long)]
        failures: bool,
        /// Show at most this many calls, newest first
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Export the state snapshot and Markdown views into this project root
    Sync {
        /// Overwrite a snapshot that was changed outside this hub
        #[arg(long)]
        force: bool,
    },
    /// Replace the project state with the contents of a JSON snapshot
    Import {
        /// Snapshot to read [default: the configured `state_file`]
        #[arg(long)]
        from: Option<PathBuf>,
        /// Skip the confirmation prompt
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum TaskCommand {
    /// List tasks, optionally filtered by role, status, track, and priority
    List {
        #[arg(long)]
        role: Option<String>,
        #[arg(long, value_enum)]
        status: Option<TaskStatus>,
        /// Only tasks of this track
        #[arg(long)]
        track: Option<String>,
        /// Only tasks without a track
        #[arg(long, conflicts_with = "track")]
        no_track: bool,
        /// Only tasks of at least this priority: critical, high, normal, or low
        #[arg(long, value_enum)]
        priority: Option<Priority>,
    },
    /// Show the next ready task for a role
    Next {
        /// Role [default: the worktree binding's]
        #[arg(long, env = "AIRLOCK_ROLE")]
        role: Option<String>,
    },
    /// Show one task
    Show { id: String },
    /// Create a work package
    Add(TaskSpec),
    /// Change the content of a work package
    Edit(TaskEditSpec),
    /// Claim a ready task and start a lease
    Claim {
        id: String,
        /// Lease duration in minutes [default: project `lease_minutes`]
        #[arg(long)]
        lease: Option<u32>,
    },
    /// Renew the lease on a claimed task
    Heartbeat {
        id: String,
        #[arg(long)]
        lease: Option<u32>,
        /// Running note for your successor: done so far, remaining, next step.
        /// Replaces the previous note; "" removes it
        #[arg(long)]
        notes: Option<String>,
    },
    /// Put an open task, or your own claimed one, on hold: `task next` passes it over until it is resumed
    Hold {
        id: String,
        /// What the task waits for (a capture, a decision)
        #[arg(long)]
        reason: String,
    },
    /// Take a task off hold
    Resume { id: String },
    /// Give a claimed task back (state returns to open)
    Release {
        id: String,
        /// Release a lease held by another session
        #[arg(long)]
        force: bool,
    },
    /// Finish a claimed task
    Done {
        id: String,
        #[arg(long)]
        notes: Option<String>,
        /// Submit for review instead of marking done
        #[arg(long)]
        review: bool,
        /// Finish a task claimed by another session (or not claimed at all)
        #[arg(long)]
        force: bool,
    },
    /// Accept a task under review
    Approve {
        id: String,
        #[arg(long)]
        notes: Option<String>,
    },
    /// Send a task under review back to open with feedback
    Reject {
        id: String,
        #[arg(long)]
        notes: String,
    },
}

/// Fields of a new work package.
#[derive(Debug, Args)]
pub struct TaskSpec {
    /// The id is assigned: the role's prefix and the next free number (e.g. "E-7")
    pub title: String,
    #[arg(long)]
    pub role: String,
    /// Spec or documentation file to read (repeatable)
    #[arg(long)]
    pub read: Vec<String>,
    /// File the task may create or modify (repeatable)
    #[arg(long)]
    pub touch: Vec<String>,
    /// Concrete behavioral instructions
    #[arg(long = "do", value_name = "INSTRUCTIONS")]
    pub r#do: String,
    /// Verification command or scenario (never executed by Airlock)
    #[arg(long)]
    pub verify: String,
    /// Prerequisite task ids, comma-separated; must already exist
    #[arg(long, value_delimiter = ',')]
    pub blocked_by: Vec<String>,
    /// critical, high, normal, or low
    #[arg(long, value_enum, default_value_t)]
    pub priority: Priority,
    /// Track the task belongs to; create it first with `airlock track add`
    #[arg(long)]
    pub track: Option<String>,
}

/// How the briefing names commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Hints {
    /// `airlock task claim <id>`
    Cli,
    /// `task_claim`
    Mcp,
}

/// Agent harnesses `airlock setup` knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Harness {
    /// `.mcp.json` for Claude Code
    Claude,
    /// `.claude/settings.local.json` with a SessionStart hook that runs `airlock start`
    ClaudeHooks,
    Antigravity,
    Dsh,
}

/// Changes to an existing work package; omitted fields keep their values.
#[derive(Debug, Args)]
pub struct TaskEditSpec {
    pub id: String,
    #[arg(long)]
    pub title: Option<String>,
    #[arg(long)]
    pub role: Option<String>,
    /// critical, high, normal, or low
    #[arg(long, value_enum)]
    pub priority: Option<Priority>,
    /// Move the task to this track
    #[arg(long)]
    pub track: Option<String>,
    /// Take the task out of its track
    #[arg(long, conflicts_with = "track")]
    pub clear_track: bool,
    /// Spec or documentation file to read (repeatable; replaces the list)
    #[arg(long)]
    pub read: Vec<String>,
    /// Empty the reading list
    #[arg(long, conflicts_with = "read")]
    pub clear_read: bool,
    /// File the task may create or modify (repeatable; replaces the list)
    #[arg(long)]
    pub touch: Vec<String>,
    /// Empty the touch list
    #[arg(long, conflicts_with = "touch")]
    pub clear_touch: bool,
    /// Concrete behavioral instructions
    #[arg(long = "do", value_name = "INSTRUCTIONS")]
    pub r#do: Option<String>,
    /// Verification command or scenario (never executed by Airlock)
    #[arg(long)]
    pub verify: Option<String>,
    /// Prerequisite task ids, comma-separated; replaces the list
    #[arg(long, value_delimiter = ',')]
    pub blocked_by: Vec<String>,
    /// Remove all prerequisites
    #[arg(long, conflicts_with = "blocked_by")]
    pub clear_blocked_by: bool,
    /// Edit a task that is in progress, in review, or done
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Subcommand)]
pub enum RoleCommand {
    /// List roles
    List,
    /// Create or change a role
    Set {
        name: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long, value_name = "BOOL")]
        clean_room: Option<bool>,
        /// Short rules shown in the session briefing
        #[arg(long)]
        instructions: Option<String>,
        /// File every session of this role reads first (repeatable; replaces the list)
        #[arg(long)]
        read: Vec<String>,
        /// Empty the reading list
        #[arg(long, conflicts_with = "read")]
        clear_read: bool,
        /// Task id prefix, e.g. "E" for E-1, E-2, … [default: the name's first letter, upper-cased]; "" resets it
        #[arg(long)]
        prefix: Option<String>,
    },
    /// Remove a role no task or binding uses
    Remove { name: String },
}

#[derive(Debug, Subcommand)]
pub enum TrackCommand {
    /// List tracks, most important first, with their task counts
    List,
    /// Create a track
    Add {
        /// Letters, digits, `-`, `_`
        name: String,
        /// critical, high, normal, or low
        #[arg(long, value_enum, default_value_t)]
        priority: Priority,
        #[arg(long, default_value = "")]
        description: String,
    },
    /// Rename or change a track; its tasks follow a rename
    Edit {
        name: String,
        /// New name
        #[arg(long = "name", value_name = "NEW_NAME")]
        new_name: Option<String>,
        /// critical, high, normal, or low
        #[arg(long, value_enum)]
        priority: Option<Priority>,
        #[arg(long)]
        description: Option<String>,
    },
    /// Remove a track no task belongs to
    Remove { name: String },
}

#[derive(Debug, Subcommand)]
pub enum QuestionCommand {
    /// List questions
    List {
        #[arg(long)]
        topic: Option<String>,
        #[arg(long, value_enum)]
        status: Option<QuestionStatus>,
    },
    /// Ask a behavioral question across the airlock
    Ask {
        question: String,
        #[arg(long)]
        topic: String,
    },
    /// Answer a question; on an answered or resolved one, correct its answer
    Answer {
        id: u64,
        answer: String,
        /// Spec file in which the answer was codified
        #[arg(long)]
        spec: Option<String>,
    },
    /// Close an answered question
    Resolve {
        id: u64,
        #[arg(long)]
        spec: Option<String>,
    },
}

impl Command {
    /// The subcommand path as typed, e.g. `"task claim"`.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Init { .. } => "init",
            Self::Use { .. } => "use",
            Self::Start { .. } => "start",
            Self::Inbox { .. } => "inbox",
            Self::Doctor => "doctor",
            Self::Setup { .. } => "setup",
            Self::Hook(HookEvent::Stop { .. }) => "hook stop",
            Self::Role(sub) => match sub {
                RoleCommand::List => "role list",
                RoleCommand::Set { .. } => "role set",
                RoleCommand::Remove { .. } => "role remove",
            },
            Self::Task(sub) => match sub {
                TaskCommand::List { .. } => "task list",
                TaskCommand::Next { .. } => "task next",
                TaskCommand::Show { .. } => "task show",
                TaskCommand::Add(_) => "task add",
                TaskCommand::Edit(_) => "task edit",
                TaskCommand::Claim { .. } => "task claim",
                TaskCommand::Heartbeat { .. } => "task heartbeat",
                TaskCommand::Release { .. } => "task release",
                TaskCommand::Hold { .. } => "task hold",
                TaskCommand::Resume { .. } => "task resume",
                TaskCommand::Done { .. } => "task done",
                TaskCommand::Approve { .. } => "task approve",
                TaskCommand::Reject { .. } => "task reject",
            },
            Self::Track(sub) => match sub {
                TrackCommand::List => "track list",
                TrackCommand::Add { .. } => "track add",
                TrackCommand::Edit { .. } => "track edit",
                TrackCommand::Remove { .. } => "track remove",
            },
            Self::Question(sub) => match sub {
                QuestionCommand::List { .. } => "question list",
                QuestionCommand::Ask { .. } => "question ask",
                QuestionCommand::Answer { .. } => "question answer",
                QuestionCommand::Resolve { .. } => "question resolve",
            },
            Self::Activity { .. } => "activity",
            Self::Mcp { .. } => "mcp",
            Self::Serve { .. } => "serve",
            Self::Sync { .. } => "sync",
            Self::Import { .. } => "import",
        }
    }
}
