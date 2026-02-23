use clap::{Parser, Subcommand};
use std::time::Duration;
use yoclaw::channels::ChannelAdapter;

#[derive(Parser)]
#[command(name = "yoclaw", version, about = "Secure, single-binary AI agent orchestrator")]
struct Cli {
    /// Path to config file (default: ~/.yoclaw/config.toml)
    #[arg(short, long)]
    config: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Show queue state, recent sessions, and token usage
    Inspect {
        /// Filter by session ID
        #[arg(short, long)]
        session: Option<String>,
        /// Show loaded skills
        #[arg(long)]
        skills: bool,
        /// Show configured workers
        #[arg(long)]
        workers: bool,
    },
    /// Initialize a new yoclaw config directory
    Init,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("yoclaw=info".parse().unwrap()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Init) => run_init(),
        Some(Commands::Inspect {
            session,
            skills,
            workers,
        }) => run_inspect(cli.config.as_deref(), session, skills, workers).await,
        None => run_main(cli.config.as_deref()).await,
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

fn run_init() -> anyhow::Result<()> {
    let dir = yoclaw::config::config_dir();
    std::fs::create_dir_all(&dir)?;
    std::fs::create_dir_all(dir.join("skills"))?;

    let config_path = dir.join("config.toml");
    if !config_path.exists() {
        std::fs::write(
            &config_path,
            r#"[agent]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
api_key = "${ANTHROPIC_API_KEY}"

[agent.budget]
max_tokens_per_day = 1_000_000
max_turns_per_session = 50

[channels.telegram]
bot_token = "${TELEGRAM_BOT_TOKEN}"
allowed_senders = []
debounce_ms = 2000

[security]
shell_deny_patterns = ["rm -rf", "sudo", "chmod 777"]
"#,
        )?;
        println!("Created {}", config_path.display());
    } else {
        println!("Config already exists: {}", config_path.display());
    }

    let persona_path = dir.join("persona.md");
    if !persona_path.exists() {
        std::fs::write(
            &persona_path,
            "You are a helpful AI assistant. Be concise and clear in your responses.\n",
        )?;
        println!("Created {}", persona_path.display());
    }

    println!("yoclaw initialized at {}", dir.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// Inspect
// ---------------------------------------------------------------------------

async fn run_inspect(
    config_path: Option<&std::path::Path>,
    session_filter: Option<String>,
    show_skills: bool,
    show_workers: bool,
) -> anyhow::Result<()> {
    let config = yoclaw::config::load_config(config_path)?;
    let db = yoclaw::db::Db::open(&config.db_path())?;

    // Skills info
    if show_skills {
        let skills_dirs = config.skills_dirs();
        let skills_refs: Vec<&std::path::Path> =
            skills_dirs.iter().map(|p| p.as_path()).collect();
        let policy = yoclaw::security::SecurityPolicy::from_config(&config.security);
        let (_prompt, loaded) = yoclaw::skills::load_filtered_skills(&skills_refs, &policy);

        println!("=== Skills ({}) ===", loaded.len());
        println!("{}", yoclaw::skills::format_skills_info(&loaded));
        println!();
    }

    // Workers info
    if show_workers {
        let worker_tools: Vec<std::sync::Arc<dyn yoagent::AgentTool>> = Vec::new();
        let workers = yoclaw::conductor::delegate::build_workers(&config, &worker_tools);
        let infos: Vec<_> = workers.into_iter().map(|(_, info)| info).collect();

        println!("=== Workers ({}) ===", infos.len());
        println!(
            "{}",
            yoclaw::conductor::delegate::format_workers_info(&infos)
        );
        println!();
    }

    // Always show queue, sessions, budget, audit
    let pending = db.queue_pending_count().await?;
    println!("=== Queue ===");
    println!("Pending messages: {}", pending);
    println!();

    // Sessions
    let sessions = db.tape_list_sessions().await?;
    println!("=== Sessions ({}) ===", sessions.len());
    for s in &sessions {
        let updated = chrono::DateTime::from_timestamp_millis(s.updated_at as i64)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "unknown".to_string());
        println!(
            "  {} — {} messages, last updated {}",
            s.session_id, s.message_count, updated
        );
    }
    println!();

    // Token usage
    let tokens_today = db.audit_token_usage_today().await?;
    println!("=== Budget ===");
    println!("Tokens used today: {}", tokens_today);
    if let Some(max) = config.agent.budget.max_tokens_per_day {
        println!("Daily limit: {}", max);
        println!("Remaining: {}", max.saturating_sub(tokens_today));
    }
    println!();

    // Audit log (recent or filtered)
    let audit = db.audit_query(session_filter.as_deref(), 20).await?;
    if !audit.is_empty() {
        println!("=== Recent Audit ({}) ===", audit.len());
        for entry in &audit {
            let ts = chrono::DateTime::from_timestamp_millis(entry.timestamp as i64)
                .map(|dt| dt.format("%H:%M:%S").to_string())
                .unwrap_or_else(|| "?".to_string());
            println!(
                "  [{}] {} {} {}",
                ts,
                entry.event_type,
                entry.tool_name.as_deref().unwrap_or(""),
                entry
                    .detail
                    .as_ref()
                    .map(|d| {
                        if d.len() > 60 {
                            format!("{}...", &d[..60])
                        } else {
                            d.clone()
                        }
                    })
                    .unwrap_or_default()
            );
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Main loop
// ---------------------------------------------------------------------------

async fn run_main(config_path: Option<&std::path::Path>) -> anyhow::Result<()> {
    let config = yoclaw::config::load_config(config_path)?;
    let db_path = config.db_path();
    let db = yoclaw::db::Db::open(&db_path)?;

    tracing::info!("Database: {}", db_path.display());

    // Crash recovery: requeue stale messages
    let requeued = db.queue_requeue_stale().await?;
    if requeued > 0 {
        tracing::info!("Requeued {} messages from previous crash", requeued);
    }

    // Build conductor
    let mut conductor = yoclaw::conductor::Conductor::new(&config, db.clone()).await?;
    tracing::info!("Conductor initialized");

    // Channel adapters
    let (raw_tx, raw_rx) = tokio::sync::mpsc::unbounded_channel();
    let (coalesced_tx, mut coalesced_rx) = tokio::sync::mpsc::unbounded_channel();

    let debounce = config
        .channels
        .telegram
        .as_ref()
        .map(|t| Duration::from_millis(t.debounce_ms))
        .unwrap_or(Duration::from_secs(2));

    let coalescer =
        yoclaw::channels::coalesce::MessageCoalescer::new(debounce, raw_rx, coalesced_tx);
    tokio::spawn(coalescer.run());

    // Collect adapters for sending responses
    let mut adapters: Vec<Box<dyn yoclaw::channels::ChannelAdapter>> = Vec::new();

    if let Some(tg_config) = config.channels.telegram.clone() {
        let adapter = yoclaw::channels::telegram::TelegramAdapter::new(tg_config);
        adapter.start(raw_tx.clone()).await?;
        adapters.push(Box::new(adapter));
    }

    if adapters.is_empty() {
        anyhow::bail!("No channels configured. Add [channels.telegram] to config.toml.");
    }

    // Ctrl+C handler: first signal logs + exits cleanly, second forces exit
    tokio::spawn(async {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("Shutting down...");
        // Give a moment for cleanup, then force exit
        tokio::time::sleep(Duration::from_millis(500)).await;
        std::process::exit(0);
    });

    tracing::info!("yoclaw running. Waiting for messages...");

    // Process loop
    while let Some(incoming) = coalesced_rx.recv().await {
        let queue_entry = yoclaw::db::queue::QueueEntry::new(
            &incoming.channel,
            &incoming.sender_id,
            &incoming.session_id,
            &incoming.content,
        );
        let queue_id = db.queue_push(&queue_entry).await?;

        tracing::info!(
            "[{}] {} ({}): {}",
            incoming.channel,
            incoming.sender_name.as_deref().unwrap_or("unknown"),
            incoming.session_id,
            truncate(&incoming.content, 80)
        );

        match conductor
            .process_message(&incoming.session_id, &incoming.content)
            .await
        {
            Ok(response) => {
                tracing::info!("Response: {}", truncate(&response, 80));

                let outgoing = yoclaw::channels::OutgoingMessage {
                    channel: incoming.channel.clone(),
                    session_id: incoming.session_id.clone(),
                    content: response,
                    reply_to: None,
                };

                for adapter in &adapters {
                    if adapter.name() == incoming.channel {
                        if let Err(e) = adapter.send(outgoing.clone()).await {
                            tracing::error!("Failed to send response: {}", e);
                        }
                        break;
                    }
                }

                db.queue_mark_done(queue_id).await?;
            }
            Err(e) => {
                tracing::error!("Processing error: {}", e);
                db.queue_mark_failed(queue_id, &e.to_string()).await?;
            }
        }
    }

    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
