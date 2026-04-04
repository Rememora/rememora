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

    /// Configure detected agents to use rememora
    Setup {
        /// Apply changes (without this flag, only shows what would be done)
        #[arg(long)]
        apply: bool,
    },

    /// Run Claude CLI on a specific GitHub issue
    AgentRun {
        /// GitHub repo (owner/name)
        #[arg(long)]
        repo: String,

        /// Issue number
        #[arg(long)]
        issue: u64,

        /// Model to use
        #[arg(long)]
        model: Option<String>,

        /// Max budget in USD
        #[arg(long)]
        max_budget: Option<f64>,

        /// Max retry attempts for quality gate
        #[arg(long, default_value = "3")]
        retries: u32,

        /// Skip all permission checks (use in sandboxed environments only)
        #[arg(long)]
        dangerously_skip_permissions: bool,
    },

    /// Watch project board for Ready-For-Dev issues and auto-dispatch to Claude CLI
    AgentLoop {
        /// GitHub repo (owner/name)
        #[arg(long)]
        repo: String,

        /// Poll interval in seconds
        #[arg(long, default_value = "300")]
        poll: u64,

        /// Model to use
        #[arg(long)]
        model: Option<String>,

        /// Max budget in USD per issue
        #[arg(long)]
        max_budget: Option<f64>,

        /// Max retry attempts per issue for quality gate
        #[arg(long, default_value = "3")]
        retries: u32,

        /// Skip all permission checks (use in sandboxed environments only)
        #[arg(long)]
        dangerously_skip_permissions: bool,

        /// Run once and exit (don't loop)
        #[arg(long)]
        once: bool,
    },

    /// Curate memories from Claude Code session transcripts
    Curate {
        /// Path to a specific JSONL file
        #[arg(long)]
        file: Option<String>,

        /// Read JSONL from stdin
        #[arg(long)]
        from_stdin: bool,

        /// Auto-discover Claude Code session files
        #[arg(long)]
        auto: bool,

        /// Show what would be done without modifying memory
        #[arg(long)]
        dry_run: bool,

        /// Backend: subagent (default) or api
        #[arg(long, default_value = "subagent")]
        backend: String,

        /// Reset watermark(s) to re-curate from beginning
        #[arg(long)]
        reset_watermark: bool,

        /// Project scope
        #[arg(long)]
        project: Option<String>,
    },

    /// Consolidate similar/redundant memories using LLM
    Evolve {
        /// Project scope (required)
        #[arg(long)]
        project: Option<String>,

        /// Show proposed changes without modifying the database
        #[arg(long)]
        dry_run: bool,

        /// Minimum similarity threshold for clustering (0.0-1.0)
        #[arg(long, default_value = "0.3")]
        min_similarity: f64,

        /// Maximum number of clusters to process per run
        #[arg(long, default_value = "50")]
        max_batch: usize,
    },

    /// Evaluate DB compliance metrics (session, memory, transfer rates)
    Eval {
        /// Filter by project
        #[arg(long)]
        project: Option<String>,

        /// Time window in days
        #[arg(long, default_value = "30")]
        days: u32,
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

    /// End the active session for the current project (hook-friendly)
    EndActive {
        /// Project name (auto-detected from CWD if omitted)
        #[arg(long)]
        project: Option<String>,

        /// Summary of what was accomplished
        #[arg(long)]
        summary: Option<String>,

        /// Current working state (blockers, next steps)
        #[arg(long)]
        working_state: Option<String>,

        /// Auto-generate summary from session metadata
        #[arg(long)]
        auto_summary: bool,
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
            SessionAction::EndActive {
                project,
                summary,
                working_state,
                auto_summary,
            } => commands::session::end_active(
                &conn,
                project.as_deref(),
                summary.as_deref(),
                working_state.as_deref(),
                auto_summary,
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

        Commands::Setup { apply } => commands::setup::run(apply),

        Commands::AgentRun {
            repo,
            issue,
            model,
            max_budget,
            retries,
            dangerously_skip_permissions,
        } => commands::agent_run::run(&commands::agent_run::AgentRunArgs {
            repo,
            issue,
            model,
            max_budget,
            allow_skip_permissions: dangerously_skip_permissions,
            retries,
        }),

        Commands::AgentLoop {
            repo,
            poll,
            model,
            max_budget,
            retries,
            dangerously_skip_permissions,
            once,
        } => commands::agent_loop::run(&commands::agent_loop::AgentLoopArgs {
            repo,
            poll_secs: poll,
            model,
            max_budget,
            allow_skip_permissions: dangerously_skip_permissions,
            once,
            retries,
        }),

        Commands::Curate {
            file,
            from_stdin,
            auto,
            dry_run,
            backend,
            reset_watermark,
            project,
        } => {
            let backend = rememora::curator::Backend::from_str(&backend)?;
            commands::curate::run(
                &conn,
                &commands::curate::CurateArgs {
                    file,
                    from_stdin,
                    auto,
                    dry_run,
                    backend,
                    reset_watermark,
                    project,
                },
                cli.json,
            )
        }

        Commands::Evolve {
            project,
            dry_run,
            min_similarity,
            max_batch,
        } => commands::evolve::run(
            &conn,
            project.as_deref(),
            dry_run,
            min_similarity,
            max_batch,
            cli.json,
        ),

        Commands::Eval { project, days } => commands::eval::run(
            &conn,
            &commands::eval::EvalArgs { project, days },
            cli.json,
        ),

        Commands::Status => commands::status::run(&conn, cli.json),

        Commands::Export { project, format } => {
            commands::export::run(&conn, project.as_deref(), &format)
        }
    }
}
