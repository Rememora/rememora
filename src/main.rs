mod commands;

use anyhow::Result;
use clap::{Parser, Subcommand};
use rememora::db;

#[derive(Parser)]
#[command(name = "rememora", version, about = "Cross-agent memory system")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Output as JSON instead of markdown
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Save a memory
    Save {
        /// The text content of the memory
        text: String,

        /// Memory category: preference, entity, decision, event, case, pattern
        #[arg(long, default_value = "entity")]
        category: String,

        /// Project scope (omit for global)
        #[arg(long)]
        project: Option<String>,

        /// Importance score 0.0-1.0
        #[arg(long, default_value = "0.5")]
        importance: f64,

        /// Source agent
        #[arg(long)]
        agent: Option<String>,

        /// Tags as JSON array
        #[arg(long)]
        tags: Option<String>,

        /// Explicit L0 abstract text
        #[arg(long, name = "abstract")]
        abstract_text: Option<String>,

        /// Explicit L1 overview text
        #[arg(long)]
        overview: Option<String>,

        /// Explicit L2 content text
        #[arg(long)]
        content: Option<String>,
    },

    /// Search memories
    Search {
        /// Search query
        query: String,

        /// Filter by project
        #[arg(long)]
        project: Option<String>,

        /// Filter by category
        #[arg(long)]
        category: Option<String>,

        /// Max results
        #[arg(long, default_value = "10")]
        limit: usize,
    },

    /// Get project context (L0 map + L1 top memories)
    Context {
        /// Project name
        #[arg(long)]
        project: Option<String>,

        /// Auto-detect project from current directory
        #[arg(long)]
        auto: bool,
    },

    /// Get a specific context by URI
    Get {
        /// The rememora:// URI
        uri: String,
    },

    /// Supersede an old memory with a new one
    Supersede {
        /// Old memory ID
        old_id: String,

        /// New memory ID
        #[arg(long)]
        by: String,
    },

    /// Session management
    Session {
        #[command(subcommand)]
        action: SessionAction,
    },

    /// Project management
    Project {
        #[command(subcommand)]
        action: ProjectAction,
    },

    /// Create a relation between two contexts
    Relate {
        /// Source URI
        source: String,

        /// Target URI
        target: String,

        /// Relation type: related, depends_on, derived_from, supersedes
        #[arg(long, name = "type", default_value = "related")]
        relation_type: String,

        /// Why they're related
        #[arg(long, default_value = "")]
        reason: String,
    },

    /// Extract memories from text using LLM
    Extract {
        /// Project scope
        #[arg(long)]
        project: Option<String>,

        /// Source agent
        #[arg(long)]
        agent: Option<String>,

        /// Read from file instead of stdin
        #[arg(long)]
        file: Option<String>,

        /// Save extracted memories to database
        #[arg(long)]
        save: bool,
    },

    /// Show system status
    Status,

    /// Export memories
    Export {
        /// Filter by project
        #[arg(long)]
        project: Option<String>,

        /// Output format: json, md
        #[arg(long, default_value = "json")]
        format: String,
    },
}

#[derive(Subcommand)]
enum SessionAction {
    /// Start a new session
    Start {
        /// Agent name
        #[arg(long)]
        agent: String,

        /// Project name
        #[arg(long)]
        project: Option<String>,

        /// What you're trying to accomplish
        #[arg(long, default_value = "")]
        intent: String,

        /// Parent session ID (for transfer chains)
        #[arg(long)]
        parent: Option<String>,
    },

    /// End a session
    End {
        /// Session ID
        id: String,

        /// Summary of what was accomplished
        #[arg(long, default_value = "")]
        summary: String,

        /// Current working state (blockers, next steps)
        #[arg(long)]
        working_state: Option<String>,

        /// Session status: ended, transferred
        #[arg(long)]
        status: Option<String>,
    },

    /// Resume from latest session for a project
    Resume {
        /// Project name
        #[arg(long)]
        project: String,
    },

    /// List recent sessions
    List {
        /// Filter by project
        #[arg(long)]
        project: Option<String>,

        /// Max results
        #[arg(long, default_value = "5")]
        limit: usize,
    },
}

#[derive(Subcommand)]
enum ProjectAction {
    /// Register a new project
    Add {
        /// Project name
        name: String,

        /// Filesystem path
        #[arg(long)]
        path: Option<String>,

        /// Short description
        #[arg(long, default_value = "")]
        description: String,

        /// Tech stack (comma-separated)
        #[arg(long, default_value = "")]
        stack: String,
    },

    /// List all projects
    List,

    /// Show project details
    Show {
        /// Project name
        name: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let db_path = db::default_db_path();
    let conn = db::open(&db_path)?;

    match cli.command {
        Commands::Save {
            text,
            category,
            project,
            importance,
            agent,
            tags,
            abstract_text,
            overview,
            content,
        } => commands::save::run(
            &conn,
            &commands::save::SaveArgs {
                text,
                category,
                project,
                importance,
                agent,
                tags,
                abstract_text,
                overview,
                content_text: content,
            },
            cli.json,
        ),

        Commands::Search {
            query,
            project,
            category,
            limit,
        } => commands::search::run(
            &conn,
            &commands::search::SearchArgs {
                query,
                project,
                category,
                limit,
            },
            cli.json,
        ),

        Commands::Context { project, auto } => {
            commands::context::run(&conn, project.as_deref(), auto)
        }

        Commands::Get { uri } => commands::get::run(&conn, &uri, cli.json),

        Commands::Supersede { old_id, by } => {
            commands::supersede::run(&conn, &old_id, &by, cli.json)
        }

        Commands::Session { action } => match action {
            SessionAction::Start {
                agent,
                project,
                intent,
                parent,
            } => commands::session::start(
                &conn,
                &agent,
                project.as_deref(),
                &intent,
                parent.as_deref(),
                cli.json,
            ),
            SessionAction::End {
                id,
                summary,
                working_state,
                status,
            } => commands::session::end(
                &conn,
                &id,
                &summary,
                working_state.as_deref(),
                status.as_deref(),
                cli.json,
            ),
            SessionAction::Resume { project } => commands::session::resume(&conn, &project),
            SessionAction::List { project, limit } => {
                commands::session::list(&conn, project.as_deref(), limit, cli.json)
            }
        },

        Commands::Project { action } => match action {
            ProjectAction::Add {
                name,
                path,
                description,
                stack,
            } => {
                let stack_vec: Vec<String> = stack
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                commands::project::add(
                    &conn,
                    &name,
                    path.as_deref(),
                    &description,
                    &stack_vec,
                    cli.json,
                )
            }
            ProjectAction::List => commands::project::list(&conn, cli.json),
            ProjectAction::Show { name } => commands::project::show(&conn, &name, cli.json),
        },

        Commands::Relate {
            source,
            target,
            relation_type,
            reason,
        } => commands::relate::run(&conn, &source, &target, &relation_type, &reason, cli.json),

        Commands::Extract {
            project,
            agent,
            file,
            save,
        } => commands::extract::run(
            &conn,
            project.as_deref(),
            agent.as_deref(),
            file.as_deref(),
            save,
            cli.json,
        ),

        Commands::Status => commands::status::run(&conn, cli.json),

        Commands::Export { project, format } => {
            commands::export::run(&conn, project.as_deref(), &format)
        }
    }
}
