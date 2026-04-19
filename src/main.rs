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

    /// Disable encryption (open database without applying encryption key)
    #[arg(long, global = true)]
    no_encryption: bool,
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

        /// Enable hierarchical score propagation (boost parents/children/siblings)
        #[arg(long)]
        propagate: bool,

        /// Decay factor per hop for propagation (default 0.3)
        #[arg(long, default_value = "0.3")]
        propagate_decay: f64,

        /// Maximum propagation hops (default 2)
        #[arg(long, default_value = "2")]
        propagate_depth: usize,
    },

    /// Get project context (L0 map + L1 top memories)
    Context {
        /// Project name
        #[arg(long)]
        project: Option<String>,

        /// Auto-detect project from current directory
        #[arg(long)]
        auto: bool,

        /// Compact cheatsheet: top-5 memories + working state + warnings
        #[arg(long)]
        cheatsheet: bool,
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
        #[arg(long, conflicts_with_all = ["from_stdin", "auto", "stream", "reset_watermark"])]
        file: Option<String>,

        /// Read JSONL from stdin
        #[arg(long, conflicts_with_all = ["auto", "stream", "reset_watermark"])]
        from_stdin: bool,

        /// Auto-discover Claude Code session files
        #[arg(long, conflicts_with_all = ["stream", "reset_watermark"])]
        auto: bool,

        /// Run as a long-lived streaming producer (Claude Code `monitors`
        /// entry point). Reads JSONL from stdin incrementally, gates +
        /// curates on fresh-byte deltas, emits rate-limited notifications.
        #[arg(long)]
        stream: bool,

        /// Session id used when streaming (emitted in the startup banner
        /// and recorded as the parent session on telemetry rows).
        #[arg(long)]
        session: Option<String>,

        /// Flush interval for the streaming curator in milliseconds.
        #[arg(long, default_value_t = rememora::stream::DEFAULT_FLUSH_MS)]
        stream_flush_ms: u64,

        /// Minimum seconds between stdout notifications in streaming mode.
        #[arg(long, default_value_t = rememora::stream::DEFAULT_NOTIFY_SECS)]
        stream_notify_secs: u64,

        /// Show what would be done without modifying memory
        #[arg(long)]
        dry_run: bool,

        /// Reset watermark(s) to re-curate from beginning
        #[arg(long)]
        reset_watermark: bool,

        /// Project scope
        #[arg(long)]
        project: Option<String>,
    },

    /// Consolidate memories using subagent (smart dedup, prune, merge)
    Consolidate {
        /// Project scope
        #[arg(long)]
        project: Option<String>,

        /// Show what would be done without modifying memory
        #[arg(long)]
        dry_run: bool,

        /// Only check if the dual gate (24h + 5 memories) is met (exit 42 = yes)
        #[arg(long)]
        check_only: bool,

        /// Minimum similarity threshold for clustering (0.0-1.0)
        #[arg(long, default_value = "0.3")]
        min_similarity: f64,

        /// Maximum number of clusters to process per run
        #[arg(long, default_value = "50")]
        max_batch: usize,
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

    /// Encrypt an existing unencrypted database
    Encrypt,

    /// Decrypt an encrypted database back to plain SQLite
    Decrypt,

    /// Interactive TUI dashboard for browsing memories
    Tui,

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

    /// Aggregate agent-invocation telemetry (tokens, cost, duration).
    Usage {
        /// Time range to aggregate over. Defaults to all recorded telemetry.
        /// Accepts e.g. `7d`, `24h`, `30m`, `all`.
        #[arg(long, default_value = "all")]
        since: String,

        /// How to group: `total`, `caller`, `model`, `project`, `session`.
        #[arg(long, default_value = "caller")]
        by: String,
    },

    /// Launch the Rememora Desktop app (Tauri)
    Desktop,
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

    // Commands that bypass the normal db::open() flow
    match &cli.command {
        Commands::Setup { apply } => return commands::setup::run(*apply),
        Commands::Encrypt => return commands::encrypt::run_encrypt(&db_path),
        Commands::Decrypt => return commands::encrypt::run_decrypt(&db_path),
        Commands::Desktop => return commands::desktop::run(),
        _ => {}
    }

    // First-run gate: require setup before any other command
    if !db_path.exists() {
        anyhow::bail!(
            "Rememora is not set up yet. Run `rememora setup` to get started."
        );
    }

    let conn = db::open_with_options(&db_path, cli.no_encryption)?;

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
            propagate,
            propagate_decay,
            propagate_depth,
        } => commands::search::run(
            &conn,
            &commands::search::SearchArgs {
                query,
                project,
                category,
                limit,
                propagate,
                propagate_decay,
                propagate_depth,
            },
            cli.json,
        ),

        Commands::Context { project, auto, cheatsheet } => {
            commands::context::run(&conn, project.as_deref(), auto, cheatsheet)
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

        Commands::Setup { .. } => unreachable!("handled above"),

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
            stream,
            session,
            stream_flush_ms,
            stream_notify_secs,
            dry_run,
            reset_watermark,
            project,
        } => commands::curate::run(
            &conn,
            &commands::curate::CurateArgs {
                file,
                from_stdin,
                auto,
                stream,
                session,
                stream_flush_ms,
                stream_notify_secs,
                dry_run,
                reset_watermark,
                project,
            },
            cli.json,
        ),

        Commands::Consolidate {
            project,
            dry_run,
            check_only,
            min_similarity,
            max_batch,
        } => commands::consolidate::run(
            &conn,
            &commands::consolidate::ConsolidateArgs {
                project,
                dry_run,
                check_only,
                min_similarity,
                max_batch,
            },
            cli.json,
        ),

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

        Commands::Encrypt | Commands::Decrypt | Commands::Desktop => unreachable!("handled above"),

        Commands::Tui => commands::tui::run(&conn),

        Commands::Status => commands::status::run(&conn, cli.json),

        Commands::Export { project, format } => {
            commands::export::run(&conn, project.as_deref(), &format)
        }

        Commands::Usage { since, by } => commands::usage::run(
            &conn,
            &commands::usage::UsageArgs { since, by },
            cli.json,
        ),
    }
}
