use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "airlock",
    author = "Peter Paprotny <pappet@web.de>",
    version,
    about = "Universal local-first multi-agent workflow coordinator and task management hub",
    long_about = "Airlock coordinates multi-agent and human developer sessions with zero git merge conflicts, clean-room boundary enforcement, dual MCP/CLI interfaces, and context-optimized work packages."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize Airlock in the current project repository
    Init {
        #[arg(short, long, default_value = "default")]
        template: String,
    },
    /// Task operations (list, next, claim, complete, add)
    Task {
        #[command(subcommand)]
        sub: TaskCommands,
    },
    /// Question operations across the airlock
    Question {
        #[command(subcommand)]
        sub: QuestionCommands,
    },
    /// Run as an MCP (Model Context Protocol) server over stdio
    Mcp,
    /// Start the local web dashboard and REST API server
    Serve {
        #[arg(short, long, default_value_t = 3000)]
        port: u16,
    },
    /// Synchronize database tasks to markdown documentation (e.g. PLAN.md)
    Sync,
}

#[derive(Subcommand)]
enum TaskCommands {
    /// List tasks, optionally filtered by role and status
    List {
        #[arg(short, long)]
        role: Option<String>,
        #[arg(short, long)]
        status: Option<String>,
    },
    /// Get the next ready task for a specific role
    Next {
        #[arg(short, long)]
        role: String,
    },
    /// Claim a task for the current session
    Claim {
        id: String,
        #[arg(short, long)]
        session: Option<String>,
    },
    /// Complete a task with optional verification notes
    Done {
        id: String,
        #[arg(short, long)]
        notes: Option<String>,
    },
}

#[derive(Subcommand)]
enum QuestionCommands {
    /// List open questions
    List {
        #[arg(short, long)]
        topic: Option<String>,
    },
    /// Ask a question across the airlock
    Ask {
        question: String,
        #[arg(short, long)]
        topic: String,
        #[arg(short, long)]
        session: Option<String>,
    },
    /// Answer an open question
    Answer {
        id: u64,
        answer: String,
        #[arg(short, long)]
        session: Option<String>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { template } => {
            println!("Initializing Airlock with template: {template}");
        }
        Commands::Task { sub } => match sub {
            TaskCommands::List { role, status } => {
                println!("Listing tasks (role: {role:?}, status: {status:?})");
            }
            TaskCommands::Next { role } => {
                println!("Finding next ready task for role: {role}");
            }
            TaskCommands::Claim { id, session } => {
                println!("Claiming task {id} for session {session:?}");
            }
            TaskCommands::Done { id, notes } => {
                println!("Marking task {id} done (notes: {notes:?})");
            }
        },
        Commands::Question { sub } => match sub {
            QuestionCommands::List { topic } => {
                println!("Listing questions (topic: {topic:?})");
            }
            QuestionCommands::Ask { question, topic, session } => {
                println!("Submitting question on topic '{topic}' by {session:?}: {question}");
            }
            QuestionCommands::Answer { id, answer, session } => {
                println!("Answering question #{id} by {session:?}: {answer}");
            }
        },
        Commands::Mcp => {
            eprintln!("Airlock MCP stdio server starting...");
        }
        Commands::Serve { port } => {
            println!("Airlock web server starting on http://localhost:{port}");
        }
        Commands::Sync => {
            println!("Synchronizing tasks with documentation files...");
        }
    }

    Ok(())
}
