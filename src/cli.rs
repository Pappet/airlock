//! Command-line interface definition (Spec 02).

use crate::core::{QuestionStatus, TaskStatus};
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

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Register the current directory as an Airlock project (or adopt an existing marker)
    Init {
        /// Project name; defaults to the directory name
        #[arg(long)]
        name: Option<String>,
        /// Role template to start from
        #[arg(long, default_value = "default")]
        template: String,
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
    Start,
    /// Check the installation, worktrees, bindings, and harness configurations
    Doctor,
    /// Print the MCP configuration of this worktree for an agent harness
    Setup {
        #[arg(value_enum)]
        harness: Harness,
        /// Role to configure [default: the worktree binding's]
        #[arg(long)]
        role: Option<String>,
    },
    /// Role management
    #[command(subcommand)]
    Role(RoleCommand),
    /// Task operations
    #[command(subcommand)]
    Task(TaskCommand),
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
        #[arg(long, default_value_t = 3000)]
        port: u16,
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
    /// List tasks, optionally filtered by role and status
    List {
        #[arg(long)]
        role: Option<String>,
        #[arg(long, value_enum)]
        status: Option<TaskStatus>,
    },
    /// Show the next ready task for a role
    Next {
        /// Role [default: the worktree binding's]
        #[arg(long)]
        role: Option<String>,
    },
    /// Show one task
    Show { id: String },
    /// Create a work package
    Add(TaskSpec),
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
    },
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
    /// Unique task id (e.g. "E-6d")
    pub id: String,
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
    #[arg(long, default_value_t = 0, allow_negative_numbers = true)]
    pub priority: i32,
}

/// Agent harnesses `airlock setup` knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Harness {
    Claude,
    Antigravity,
    Dsh,
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
    },
    /// Remove a role no task or binding uses
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
    /// Answer (or revise the answer to) a question
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
            Self::Start => "start",
            Self::Doctor => "doctor",
            Self::Setup { .. } => "setup",
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
                TaskCommand::Claim { .. } => "task claim",
                TaskCommand::Heartbeat { .. } => "task heartbeat",
                TaskCommand::Release { .. } => "task release",
                TaskCommand::Done { .. } => "task done",
                TaskCommand::Approve { .. } => "task approve",
                TaskCommand::Reject { .. } => "task reject",
            },
            Self::Question(sub) => match sub {
                QuestionCommand::List { .. } => "question list",
                QuestionCommand::Ask { .. } => "question ask",
                QuestionCommand::Answer { .. } => "question answer",
                QuestionCommand::Resolve { .. } => "question resolve",
            },
            Self::Mcp { .. } => "mcp",
            Self::Serve { .. } => "serve",
            Self::Sync { .. } => "sync",
            Self::Import { .. } => "import",
        }
    }
}
