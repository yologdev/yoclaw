use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use yoclaw::channels::ChannelAdapter;

#[derive(Parser)]
#[command(
    name = "yoclaw",
    version,
    about = "Secure, single-binary AI agent orchestrator"
)]
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
    /// Migrate from an OpenClaw installation
    Migrate {
        /// Path to the OpenClaw data directory
        openclaw_dir: std::path::PathBuf,
    },
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
        Some(Commands::Init) => run_init(cli.config.as_deref()),
        Some(Commands::Inspect {
            session,
            skills,
            workers,
        }) => run_inspect(cli.config.as_deref(), session, skills, workers).await,
        Some(Commands::Migrate { openclaw_dir }) => yoclaw::migrate::run_migrate(&openclaw_dir),
        None => run_main(cli.config.as_deref()).await,
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

fn run_init(config_override: Option<&std::path::Path>) -> anyhow::Result<()> {
    let dir = match config_override {
        Some(p) => p
            .parent()
            .map(|d| d.to_path_buf())
            .unwrap_or_else(yoclaw::config::config_dir),
        None => yoclaw::config::config_dir(),
    };
    std::fs::create_dir_all(&dir)?;
    std::fs::create_dir_all(dir.join("skills"))?;

    let config_path = match config_override {
        Some(p) => p.to_path_buf(),
        None => dir.join("config.toml"),
    };
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
        let skills_refs: Vec<&std::path::Path> = skills_dirs.iter().map(|p| p.as_path()).collect();
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
    let config_file_path = match config_path {
        Some(p) => p.to_path_buf(),
        None => yoclaw::config::config_dir().join("config.toml"),
    };
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

    // Build per-channel debounce map
    let mut channel_debounce: HashMap<String, Duration> = HashMap::new();
    if let Some(ref tg) = config.channels.telegram {
        channel_debounce.insert("telegram".into(), Duration::from_millis(tg.debounce_ms));
    }
    if let Some(ref dc) = config.channels.discord {
        channel_debounce.insert("discord".into(), Duration::from_millis(dc.debounce_ms));
    }
    if let Some(ref sl) = config.channels.slack {
        channel_debounce.insert("slack".into(), Duration::from_millis(sl.debounce_ms));
    }

    let coalescer = yoclaw::channels::coalesce::MessageCoalescer::new(
        Duration::from_secs(2),
        raw_rx,
        coalesced_tx,
    )
    .with_channel_debounce(channel_debounce);
    let shared_debounce = coalescer.shared_debounce();
    tokio::spawn(coalescer.run());

    // Collect adapters for sending responses (Arc for sharing with scheduler delivery)
    let mut adapters: Vec<Arc<dyn yoclaw::channels::ChannelAdapter>> = Vec::new();

    if let Some(tg_config) = config.channels.telegram.clone() {
        let adapter = yoclaw::channels::telegram::TelegramAdapter::new(tg_config);
        adapter.start(raw_tx.clone()).await?;
        adapters.push(Arc::new(adapter));
    }

    if let Some(dc_config) = config.channels.discord.clone() {
        let adapter = yoclaw::channels::discord::DiscordAdapter::new(dc_config);
        adapter.start(raw_tx.clone()).await?;
        adapters.push(Arc::new(adapter));
    }

    if let Some(sl_config) = config.channels.slack.clone() {
        let adapter = yoclaw::channels::slack::SlackAdapter::new(sl_config);
        adapter.start(raw_tx.clone()).await?;
        adapters.push(Arc::new(adapter));
    }

    if adapters.is_empty() {
        anyhow::bail!("No channels configured. Add [channels.telegram], [channels.discord], or [channels.slack] to config.toml.");
    }

    // Web UI
    let (sse_tx, _) = tokio::sync::broadcast::channel::<yoclaw::web::SseEvent>(256);
    let sse_tx_clone = sse_tx.clone();

    if config.web.enabled {
        let web_db = db.clone();
        let web_sse_tx = sse_tx.clone();
        // Scheduler needs &config below, so build Arc separately for the web server
        let web_config = Arc::new(yoclaw::config::load_config(config_path)?);
        tokio::spawn(async move {
            if let Err(e) = yoclaw::web::start_server(web_db, web_config, web_sse_tx).await {
                tracing::error!("Web server error: {}", e);
            }
        });
    }

    // Scheduler
    if config.scheduler.enabled {
        // Create a delivery channel for cron job results
        let (delivery_tx, mut delivery_rx) =
            tokio::sync::mpsc::unbounded_channel::<yoclaw::channels::OutgoingMessage>();

        let scheduler = yoclaw::scheduler::Scheduler::new(db.clone(), &config, Some(delivery_tx));
        tokio::spawn(async move {
            scheduler.run().await;
        });

        // Route scheduler deliveries to channel adapters
        let delivery_adapters = adapters.clone();
        tokio::spawn(async move {
            while let Some(outgoing) = delivery_rx.recv().await {
                tracing::info!(
                    "Scheduler delivery to {}: {}",
                    outgoing.channel,
                    if outgoing.content.len() > 80 {
                        format!("{}...", &outgoing.content[..80])
                    } else {
                        outgoing.content.clone()
                    }
                );
                for adapter in &delivery_adapters {
                    if adapter.name() == outgoing.channel {
                        if let Err(e) = adapter.send(outgoing.clone()).await {
                            tracing::error!("Scheduler delivery error: {}", e);
                        }
                        break;
                    }
                }
            }
        });
    }

    // Ctrl+C handler: first signal logs + exits cleanly, second forces exit
    tokio::spawn(async {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("Shutting down...");
        // Give a moment for cleanup, then force exit
        tokio::time::sleep(Duration::from_millis(500)).await;
        std::process::exit(0);
    });

    // Config hot-reload watcher (polls every 5 seconds)
    let mut config_watcher = yoclaw::watcher::ConfigWatcher::new(config_file_path);
    let mut current_config = config;
    let mut reload_interval = tokio::time::interval(Duration::from_secs(5));
    reload_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    tracing::info!("yoclaw running. Waiting for messages...");

    // Process loop
    loop {
        tokio::select! {
            // Config hot-reload poll
            _ = reload_interval.tick() => {
                if let Some(new_config) = config_watcher.check() {
                    let diff = yoclaw::watcher::diff_configs(&current_config, &new_config);
                    yoclaw::watcher::apply_hot_reload(&diff, &new_config, &mut conductor, &shared_debounce);
                    current_config = new_config;
                }
                continue;
            }
            // Incoming message
            msg = coalesced_rx.recv() => {
                let incoming = match msg {
                    Some(m) => m,
                    None => break, // channel closed
                };

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

        // Find the adapter for this channel
        let adapter = adapters
            .iter()
            .find(|a| a.name() == incoming.channel)
            .cloned();

        // Start typing indicator
        let typing_handle = adapter.as_ref().and_then(|a| a.start_typing(&incoming.session_id));

        // Send a streaming placeholder message
        let placeholder = if let Some(ref adapter) = adapter {
            adapter.send_placeholder(&incoming.session_id, "...").await
        } else {
            None
        };

        // Build debounced on_chunk callback for streaming edits
        let on_chunk: Option<yoclaw::conductor::OnStreamChunk> = {
            if let (Some(ref ph), Some(ref adapter)) = (&placeholder, &adapter) {
                let ph = ph.clone();
                let adapter = adapter.clone();
                // Get stream debounce from current config
                let debounce_ms = match incoming.channel.as_str() {
                    "telegram" => current_config.channels.telegram.as_ref().map(|c| c.stream_debounce_ms).unwrap_or(300),
                    "discord" => current_config.channels.discord.as_ref().map(|c| c.stream_debounce_ms).unwrap_or(300),
                    "slack" => current_config.channels.slack.as_ref().map(|c| c.stream_debounce_ms).unwrap_or(300),
                    _ => 300,
                };
                let debounce = Duration::from_millis(debounce_ms);
                let last_edit = Arc::new(std::sync::Mutex::new(std::time::Instant::now() - debounce));
                // Also emit SSE events for web UI streaming
                let sse_tx = sse_tx_clone.clone();
                let sse_session = incoming.session_id.clone();
                let sse_channel = incoming.channel.clone();

                Some(Box::new(move |accumulated: &str| {
                    let mut last = last_edit.lock().unwrap();
                    if last.elapsed() >= debounce {
                        *last = std::time::Instant::now();
                        let ph = ph.clone();
                        let adapter = adapter.clone();
                        let text = accumulated.to_string();
                        tokio::spawn(async move {
                            let _ = adapter.edit_message(&ph, &text).await;
                        });
                    }
                    // Emit SSE stream chunk
                    let _ = sse_tx.send(yoclaw::web::SseEvent::StreamChunk {
                        session_id: sse_session.clone(),
                        channel: sse_channel.clone(),
                        text: accumulated.to_string(),
                    });
                }) as yoclaw::conductor::OnStreamChunk)
            } else {
                None
            }
        };

        // Build progress callback to route send_message tool output to the channel
        let on_progress: Option<Box<dyn Fn(String) + Send + Sync>> = {
            if let Some(ref adapter) = adapter {
                let adapter = adapter.clone();
                let channel = incoming.channel.clone();
                let session_id = incoming.session_id.clone();
                Some(Box::new(move |text: String| {
                    let outgoing = yoclaw::channels::OutgoingMessage {
                        channel: channel.clone(),
                        session_id: session_id.clone(),
                        content: text,
                        reply_to: None,
                    };
                    let adapter = adapter.clone();
                    tokio::spawn(async move {
                        let _ = adapter.send(outgoing).await;
                    });
                }))
            } else {
                None
            }
        };

        let result = if let Some(ref worker_name) = incoming.worker_hint {
            conductor
                .delegate_to_worker(&incoming.session_id, worker_name, &incoming.content)
                .await
        } else if incoming.is_group {
            conductor
                .process_group_message(&incoming.session_id, &incoming.content, on_chunk, on_progress)
                .await
        } else {
            conductor
                .process_message(&incoming.session_id, &incoming.content, on_chunk, on_progress)
                .await
        };

        // Stop typing indicator
        if let Some(handle) = typing_handle {
            handle.abort();
        }

        match result {
            Ok(response) => {
                tracing::info!("Response: {}", truncate(&response, 80));

                // Final edit to ensure complete text if we had a placeholder
                if let Some(ref ph) = placeholder {
                    if let Some(ref adapter) = adapter {
                        let _ = adapter.edit_message(ph, &response).await;
                    }
                } else {
                    // No placeholder — send the full response as a new message
                    let outgoing = yoclaw::channels::OutgoingMessage {
                        channel: incoming.channel.clone(),
                        session_id: incoming.session_id.clone(),
                        content: response,
                        reply_to: None,
                    };

                    if let Some(ref adapter) = adapter {
                        if let Err(e) = adapter.send(outgoing).await {
                            tracing::error!("Failed to send response: {}", e);
                        }
                    }
                }

                db.queue_mark_done(queue_id).await?;

                // Emit SSE events for web UI
                let _ = sse_tx_clone.send(yoclaw::web::SseEvent::StreamEnd {
                    session_id: incoming.session_id.clone(),
                    channel: incoming.channel.clone(),
                });
                let _ = sse_tx_clone.send(yoclaw::web::SseEvent::MessageProcessed {
                    session_id: incoming.session_id.clone(),
                    channel: incoming.channel.clone(),
                });
            }
            Err(e) => {
                tracing::error!("Processing error: {}", e);
                db.queue_mark_failed(queue_id, &e.to_string()).await?;
            }
        }
            } // end select msg arm
        } // end select
    } // end loop

    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
