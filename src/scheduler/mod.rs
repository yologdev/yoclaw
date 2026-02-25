pub mod cortex;
pub mod cron;
pub mod tools;

use crate::channels::OutgoingMessage;
use crate::config::{Config, SchedulerConfig};
use crate::db::Db;
use std::time::Duration;
use tokio::sync::mpsc;

/// Agent configuration needed to spawn ephemeral agents for cron/cortex tasks.
#[derive(Clone)]
pub struct AgentRunConfig {
    pub provider: String,
    pub model: String,
    pub api_key: String,
}

/// Unified scheduler for both cortex maintenance and user-defined cron jobs.
pub struct Scheduler {
    db: Db,
    config: SchedulerConfig,
    agent_config: AgentRunConfig,
    /// Sender for delivering cron job results to channel adapters.
    delivery_tx: Option<mpsc::UnboundedSender<OutgoingMessage>>,
}

impl Scheduler {
    pub fn new(
        db: Db,
        config: &Config,
        delivery_tx: Option<mpsc::UnboundedSender<OutgoingMessage>>,
    ) -> Self {
        Self {
            db,
            config: SchedulerConfig {
                enabled: config.scheduler.enabled,
                tick_interval_secs: config.scheduler.tick_interval_secs,
                cortex: crate::config::CortexConfig {
                    interval_hours: config.scheduler.cortex.interval_hours,
                    model: config.scheduler.cortex.model.clone(),
                },
                cron: crate::config::CronConfig {
                    jobs: config.scheduler.cron.jobs.clone(),
                },
            },
            agent_config: AgentRunConfig {
                provider: config.agent.provider.clone(),
                model: config.agent.model.clone(),
                api_key: config.agent.api_key.clone(),
            },
            delivery_tx,
        }
    }

    /// Run the scheduler tick loop. Blocks forever (should be spawned).
    pub async fn run(self) {
        let tick = Duration::from_secs(self.config.tick_interval_secs);
        let mut cortex_last_run: Option<std::time::Instant> = None;
        let cortex_interval = Duration::from_secs(self.config.cortex.interval_hours * 3600);

        // Load static cron jobs from config into DB
        if let Err(e) = self.sync_config_jobs().await {
            tracing::error!("Failed to sync cron jobs from config: {}", e);
        }

        tracing::info!(
            "Scheduler started (tick: {}s, cortex interval: {}h, {} cron jobs)",
            self.config.tick_interval_secs,
            self.config.cortex.interval_hours,
            self.config.cron.jobs.len(),
        );

        loop {
            tokio::time::sleep(tick).await;

            // 1. Check cortex: time for maintenance?
            let run_cortex = match cortex_last_run {
                Some(last) => last.elapsed() >= cortex_interval,
                None => true, // run on first tick
            };

            if run_cortex {
                tracing::info!("Running cortex maintenance...");
                let cortex_model = self.config.cortex.model.clone();
                let cortex_agent = AgentRunConfig {
                    provider: self.agent_config.provider.clone(),
                    model: cortex_model,
                    api_key: self.agent_config.api_key.clone(),
                };
                match cortex::run_maintenance(&self.db, &cortex_agent).await {
                    Ok(summary) => {
                        tracing::info!("Cortex maintenance complete: {}", summary);
                        cortex_last_run = Some(std::time::Instant::now());
                    }
                    Err(e) => {
                        tracing::error!("Cortex maintenance error: {}", e);
                    }
                }
            }

            // 2. Check cron jobs: any jobs due?
            match cron::check_and_run_due_jobs(
                &self.db,
                &self.agent_config,
                self.delivery_tx.as_ref(),
            )
            .await
            {
                Ok(ran) => {
                    if ran > 0 {
                        tracing::info!("Ran {} cron job(s)", ran);
                    }
                }
                Err(e) => {
                    tracing::error!("Cron check error: {}", e);
                }
            }
        }
    }

    /// Sync static cron jobs from config into the database.
    async fn sync_config_jobs(&self) -> Result<(), crate::db::DbError> {
        for job in &self.config.cron.jobs {
            let name = job.name.clone();
            let schedule = job.schedule.clone();
            let prompt = job.prompt.clone();
            let target = job.target.clone();
            let session = job.session.clone();

            self.db
                .exec(move |conn| {
                    let ts = crate::db::now_ms() as i64;
                    conn.execute(
                        "INSERT INTO cron_jobs (name, schedule, prompt, target_channel, session_mode, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
                         ON CONFLICT(name) DO UPDATE SET
                            schedule = excluded.schedule,
                            prompt = excluded.prompt,
                            target_channel = excluded.target_channel,
                            session_mode = excluded.session_mode,
                            updated_at = excluded.updated_at",
                        rusqlite::params![name, schedule, prompt, target, session, ts],
                    )?;
                    Ok(())
                })
                .await?;
        }
        Ok(())
    }
}

/// Run an ephemeral agent with a single prompt and return the text response.
/// Uses `agent_loop` directly for a fresh, stateless agent invocation.
pub async fn run_ephemeral_prompt(
    agent_config: &AgentRunConfig,
    system_prompt: &str,
    task: &str,
) -> Result<String, anyhow::Error> {
    use crate::conductor::resolve_provider;
    use yoagent::agent_loop::{agent_loop, AgentLoopConfig};
    use yoagent::context::ExecutionLimits;
    use yoagent::types::*;

    let provider = resolve_provider(&agent_config.provider);
    let provider_ref: &dyn yoagent::provider::StreamProvider = &provider;

    let mut context = AgentContext {
        system_prompt: system_prompt.to_string(),
        messages: Vec::new(),
        tools: Vec::new(),
    };

    let config = AgentLoopConfig {
        provider: provider_ref,
        model: agent_config.model.clone(),
        api_key: agent_config.api_key.clone(),
        thinking_level: ThinkingLevel::Off,
        max_tokens: None,
        temperature: None,
        convert_to_llm: None,
        transform_context: None,
        get_steering_messages: None,
        get_follow_up_messages: None,
        context_config: None,
        compaction_strategy: None,
        input_filters: Vec::new(),
        execution_limits: Some(ExecutionLimits {
            max_turns: 1,
            max_total_tokens: 100_000,
            max_duration: std::time::Duration::from_secs(120),
        }),
        cache_config: CacheConfig::default(),
        tool_execution: ToolExecutionStrategy::default(),
        retry_config: yoagent::RetryConfig::default(),
        before_turn: None,
        after_turn: None,
        on_error: None,
    };

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let cancel = tokio_util::sync::CancellationToken::new();

    let prompt_msg = AgentMessage::Llm(Message::user(task));
    let messages = agent_loop(vec![prompt_msg], &mut context, &config, tx, cancel).await;

    // Extract text from the last assistant message
    for msg in messages.iter().rev() {
        if let AgentMessage::Llm(Message::Assistant { content, .. }) = msg {
            let texts: Vec<&str> = content
                .iter()
                .filter_map(|c| match c {
                    Content::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            if !texts.is_empty() {
                return Ok(texts.join("\n"));
            }
        }
    }

    Ok("(no response)".to_string())
}

/// Run a persistent agent: loads prior conversation from tape, appends the new prompt,
/// runs agent_loop, then saves the full conversation back.
pub async fn run_persistent_prompt(
    db: &Db,
    agent_config: &AgentRunConfig,
    session_id: &str,
    system_prompt: &str,
    task: &str,
) -> Result<String, anyhow::Error> {
    use crate::conductor::resolve_provider;
    use yoagent::agent_loop::{agent_loop, AgentLoopConfig};
    use yoagent::context::ExecutionLimits;
    use yoagent::types::*;

    // 1. Load prior messages from tape
    let mut prompts = db.tape_load_messages(session_id).await?;
    // 2. Append new user message
    prompts.push(AgentMessage::Llm(Message::user(task)));

    let provider = resolve_provider(&agent_config.provider);
    let provider_ref: &dyn yoagent::provider::StreamProvider = &provider;

    let mut context = AgentContext {
        system_prompt: system_prompt.to_string(),
        messages: Vec::new(),
        tools: Vec::new(),
    };

    let config = AgentLoopConfig {
        provider: provider_ref,
        model: agent_config.model.clone(),
        api_key: agent_config.api_key.clone(),
        thinking_level: ThinkingLevel::Off,
        max_tokens: None,
        temperature: None,
        convert_to_llm: None,
        transform_context: None,
        get_steering_messages: None,
        get_follow_up_messages: None,
        context_config: None,
        compaction_strategy: None,
        input_filters: Vec::new(),
        execution_limits: Some(ExecutionLimits {
            max_turns: 5,
            max_total_tokens: 100_000,
            max_duration: std::time::Duration::from_secs(120),
        }),
        cache_config: CacheConfig::default(),
        tool_execution: ToolExecutionStrategy::default(),
        retry_config: yoagent::RetryConfig::default(),
        before_turn: None,
        after_turn: None,
        on_error: None,
    };

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let cancel = tokio_util::sync::CancellationToken::new();

    // 3. Run agent_loop — returns prompts + all new messages
    let all_messages = agent_loop(prompts, &mut context, &config, tx, cancel).await;

    // 4. Save full conversation back to tape
    db.tape_save_messages(session_id, &all_messages).await?;

    // 5. Extract text from the last assistant message
    for msg in all_messages.iter().rev() {
        if let AgentMessage::Llm(Message::Assistant { content, .. }) = msg {
            let texts: Vec<&str> = content
                .iter()
                .filter_map(|c| match c {
                    Content::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            if !texts.is_empty() {
                return Ok(texts.join("\n"));
            }
        }
    }

    Ok("(no response)".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_config;

    #[tokio::test]
    async fn test_sync_config_jobs() {
        let db = Db::open_memory().unwrap();
        let config = parse_config(
            r#"
[agent]
model = "test"
api_key = "key"

[scheduler]
enabled = true

[[scheduler.cron.jobs]]
name = "test-job"
schedule = "0 9 * * *"
prompt = "Do something"
target = "telegram"
"#,
        )
        .unwrap();

        let scheduler = Scheduler::new(db.clone(), &config, None);
        scheduler.sync_config_jobs().await.unwrap();

        // Verify job was created in DB
        let count = db
            .exec(|conn| {
                let c: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM cron_jobs WHERE name = 'test-job'",
                    [],
                    |r| r.get(0),
                )?;
                Ok(c)
            })
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
}
