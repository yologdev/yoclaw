pub mod compaction;
pub mod delegate;
pub mod tools;

use crate::config::Config;
use crate::db::Db;
use crate::security::budget::BudgetTracker;
use crate::security::{self, SecurityPolicy};
use crate::skills::LoadedSkill;
use delegate::WorkerInfo;
use std::collections::HashMap;
use std::sync::Arc;
use yoagent::provider;
use yoagent::types::*;
use yoagent::Agent;

/// The Conductor owns the yoagent Agent and mediates all interactions.
pub struct Conductor {
    agent: Agent,
    db: Db,
    current_session: String,
    session_id_ref: Arc<std::sync::RwLock<String>>,
    policy_ref: Arc<std::sync::RwLock<SecurityPolicy>>,
    budget: BudgetTracker,
    loaded_skills: Vec<LoadedSkill>,
    worker_infos: Vec<WorkerInfo>,
    /// Worker sub-agent tools for direct delegation (bypassing main agent).
    direct_workers: HashMap<String, Box<dyn AgentTool>>,
    /// Max messages to restore for group chat catch-up.
    max_group_catchup: usize,
    /// Messages trimmed from the front during group chat catch-up.
    /// Prepended back when saving to preserve the full tape.
    group_catchup_prefix: Vec<AgentMessage>,
}

impl Conductor {
    /// Create a new Conductor from config.
    pub async fn new(config: &Config, db: Db) -> Result<Self, anyhow::Error> {
        // 1. Load persona
        let persona_path = config.persona_path();
        let persona = if persona_path.exists() {
            std::fs::read_to_string(&persona_path)?
        } else {
            "You are a helpful AI assistant.".to_string()
        };

        // 2. Load skills with capability filtering
        let skills_dirs = config.skills_dirs();
        let skills_refs: Vec<&std::path::Path> = skills_dirs.iter().map(|p| p.as_path()).collect();
        let policy = SecurityPolicy::from_config(&config.security);
        let (skills_prompt, loaded_skills) =
            crate::skills::load_filtered_skills(&skills_refs, &policy);
        let policy_ref = Arc::new(std::sync::RwLock::new(policy));

        if !loaded_skills.is_empty() {
            tracing::info!("Loaded {} skill(s)", loaded_skills.len());
        }

        // Append skills to persona
        let persona = if skills_prompt.is_empty() {
            persona
        } else {
            format!("{}\n\n{}", persona, skills_prompt)
        };

        // 3. Build tools
        let mut tool_list: Vec<Box<dyn AgentTool>> = yoagent::tools::default_tools();
        tool_list.push(Box::new(tools::MemorySearchTool::new(db.clone())));
        tool_list.push(Box::new(tools::MemoryStoreTool::new(db.clone())));
        tool_list.push(Box::new(crate::scheduler::tools::CronScheduleTool::new(
            db.clone(),
        )));
        tool_list.push(Box::new(tools::SendMessageTool));

        // 4. Wrap with security
        let session_id_ref = Arc::new(std::sync::RwLock::new(String::new()));
        let mut wrapped_tools = security::wrap_tools(
            tool_list,
            policy_ref.clone(),
            db.clone(),
            session_id_ref.clone(),
        );

        // 5. Build budget tracker
        let budget = BudgetTracker::new(
            config.agent.budget.max_tokens_per_day,
            config.agent.budget.max_turns_per_session,
            db.clone(),
        );
        budget.load_from_db().await?;

        // 6. Build worker sub-agents from config
        // Workers get security-wrapped tools so their internal tool calls are
        // audit-logged and policy-checked (Gap 2 fix)
        let worker_tools: Vec<Arc<dyn AgentTool>> = vec![
            Arc::new(security::SecureToolWrapper {
                inner: Box::new(tools::MemorySearchTool::new(db.clone())),
                policy: policy_ref.clone(),
                db: db.clone(),
                session_id: session_id_ref.clone(),
            }),
            Arc::new(security::SecureToolWrapper {
                inner: Box::new(tools::MemoryStoreTool::new(db.clone())),
                policy: policy_ref.clone(),
                db: db.clone(),
                session_id: session_id_ref.clone(),
            }),
        ];
        let workers = delegate::build_workers(config, &worker_tools);
        let worker_infos: Vec<WorkerInfo> = workers.iter().map(|(_, info)| info.clone()).collect();

        if !worker_infos.is_empty() {
            tracing::info!("Configured {} worker(s)", worker_infos.len());
        }

        // Build a second set of workers for direct delegation (bypassing main agent).
        // No outer SecureToolWrapper here — the SubAgentTool's inner tools are already
        // security-wrapped via worker_tools, and wrapping the SubAgentTool itself would
        // produce misleading audit entries under the worker name (e.g., "coding").
        let direct_workers_raw = delegate::build_workers(config, &worker_tools);
        let mut direct_workers: HashMap<String, Box<dyn AgentTool>> = HashMap::new();
        for (sub_agent, info) in direct_workers_raw {
            direct_workers.insert(info.name.clone(), Box::new(sub_agent));
        }

        // Wrap each SubAgentTool with SecureToolWrapper so worker delegations
        // are audit-logged and security-checked (Gap 1 fix)
        for (sub_agent, _info) in workers {
            wrapped_tools.push(Box::new(security::SecureToolWrapper {
                inner: Box::new(sub_agent),
                policy: policy_ref.clone(),
                db: db.clone(),
                session_id: session_id_ref.clone(),
            }));
        }

        // 7. Resolve provider
        let provider = resolve_provider(&config.agent.provider);

        // 8. Build agent — workers are included in wrapped_tools, no with_sub_agent needed
        let budget_check = budget.clone();
        let budget_record = budget.clone();
        let db_usage = db.clone();
        let session_id_usage = session_id_ref.clone();
        let mut agent = Agent::new(provider)
            .with_system_prompt(&persona)
            .with_model(&config.agent.model)
            .with_api_key(&config.agent.api_key)
            .with_tools(wrapped_tools)
            .on_before_turn(move |_messages, _turn| budget_check.can_continue())
            .on_after_turn(move |_messages, usage| {
                budget_record.record_usage(usage.input, usage.output);
                budget_record.record_turn();
                // Persist token usage to audit table so budget survives restarts
                let total = usage.input + usage.output;
                if total > 0 {
                    let sid = session_id_usage.read().unwrap().clone();
                    let ts = crate::db::now_ms() as i64;
                    let _ = db_usage.exec_sync(|conn| {
                        conn.execute(
                            "INSERT INTO audit (session_id, event_type, tokens_used, timestamp) \
                             VALUES (?1, ?2, ?3, ?4)",
                            rusqlite::params![sid, "llm_usage", total as i64, ts],
                        )?;
                        Ok(())
                    });
                }
            });

        // 8a. Wire up context management from config
        let ctx = &config.agent.context;
        if ctx.max_context_tokens.is_some()
            || ctx.keep_recent.is_some()
            || ctx.tool_output_max_lines.is_some()
        {
            let mut ctx_config = yoagent::context::ContextConfig::default();
            if let Some(max) = ctx.max_context_tokens {
                ctx_config.max_context_tokens = max as usize;
            }
            if let Some(keep) = ctx.keep_recent {
                ctx_config.keep_recent = keep;
            }
            if let Some(max_lines) = ctx.tool_output_max_lines {
                ctx_config.tool_output_max_lines = max_lines;
            }
            agent = agent.with_context_config(ctx_config);
            agent = agent.with_compaction_strategy(compaction::MemoryAwareCompaction::new(
                db.clone(),
                session_id_ref.clone(),
            ));
            tracing::info!("Context management enabled");
        }

        // 8b. Wire up injection detection if enabled
        if config.security.injection.enabled {
            let detector = crate::security::injection::InjectionDetector::new(
                &config.security.injection.action,
                &config.security.injection.extra_patterns,
            );
            agent = agent.with_input_filter(detector);
            tracing::info!(
                "Injection detection enabled (action: {})",
                config.security.injection.action
            );
        }

        if let Some(max_tokens) = config.agent.max_tokens {
            agent = agent.with_max_tokens(max_tokens);
        }

        if let Some(ref thinking) = config.agent.thinking {
            let level = match thinking.as_str() {
                "off" => ThinkingLevel::Off,
                "low" => ThinkingLevel::Low,
                "medium" => ThinkingLevel::Medium,
                "high" => ThinkingLevel::High,
                _ => ThinkingLevel::Off,
            };
            agent = agent.with_thinking(level);
        }

        Ok(Self {
            agent,
            db,
            current_session: String::new(),
            session_id_ref,
            policy_ref,
            budget,
            loaded_skills,
            worker_infos,
            direct_workers,
            max_group_catchup: config.agent.context.max_group_catchup_messages,
            group_catchup_prefix: Vec::new(),
        })
    }

    /// Get loaded skills info.
    pub fn loaded_skills(&self) -> &[LoadedSkill] {
        &self.loaded_skills
    }

    /// Get configured worker info.
    pub fn worker_infos(&self) -> &[WorkerInfo] {
        &self.worker_infos
    }

    /// Update budget limits at runtime (hot-reload).
    pub fn update_budget(&mut self, max_tokens: Option<u64>, max_turns: Option<usize>) {
        self.budget.update_limits(max_tokens, max_turns);
        tracing::info!(
            "Budget updated: max_tokens={:?}, max_turns={:?}",
            max_tokens,
            max_turns
        );
    }

    /// Replace the security policy at runtime (hot-reload).
    /// This propagates to all SecureToolWrapper instances via the shared Arc<RwLock>.
    pub fn update_security(&self, new_policy: SecurityPolicy) {
        *self.policy_ref.write().unwrap() = new_policy;
        tracing::info!("Security policy reloaded");
    }

    /// Update max group catchup messages (hot-reload).
    pub fn update_max_group_catchup(&mut self, max: usize) {
        self.max_group_catchup = max;
    }

    /// Process a user message and return the assistant's text response.
    /// If `on_progress` is provided, ProgressMessage events (from send_message tool)
    /// are forwarded in real-time. `is_group` enables group chat catch-up slicing.
    pub async fn process_message(
        &mut self,
        session_id: &str,
        text: &str,
        on_progress: Option<Box<dyn Fn(String) + Send + Sync>>,
    ) -> Result<String, anyhow::Error> {
        self.process_message_inner(session_id, text, false, on_progress)
            .await
    }

    /// Process a message from a group chat. Only loads messages since the last
    /// assistant reply (catch-up mode) instead of the full conversation history.
    pub async fn process_group_message(
        &mut self,
        session_id: &str,
        text: &str,
        on_progress: Option<Box<dyn Fn(String) + Send + Sync>>,
    ) -> Result<String, anyhow::Error> {
        self.process_message_inner(session_id, text, true, on_progress)
            .await
    }

    async fn process_message_inner(
        &mut self,
        session_id: &str,
        text: &str,
        is_group: bool,
        on_progress: Option<Box<dyn Fn(String) + Send + Sync>>,
    ) -> Result<String, anyhow::Error> {
        // Switch session if needed
        if self.current_session != session_id {
            self.switch_session(session_id, is_group).await?;
        }

        // Run the agent
        let rx = self.agent.prompt(text).await;

        // Drain events and collect response
        let result = drain_response(rx, on_progress).await;

        // Audit log if input was rejected (e.g. by injection detector)
        if let Some(ref reason) = result.input_rejected {
            let _ = self
                .db
                .audit_log(Some(session_id), "input_rejected", None, Some(reason), 0)
                .await;
            return Ok("I can't process that message.".to_string());
        }

        // Persist conversation state — reconstruct full tape if group catchup trimmed a prefix
        let prefix = std::mem::take(&mut self.group_catchup_prefix);
        if prefix.is_empty() {
            self.db
                .tape_save_messages(session_id, self.agent.messages())
                .await?;
        } else {
            let mut full_tape = prefix;
            full_tape.extend_from_slice(self.agent.messages());
            self.db.tape_save_messages(session_id, &full_tape).await?;
        }

        Ok(result.response)
    }

    async fn switch_session(
        &mut self,
        new_session: &str,
        is_group: bool,
    ) -> Result<(), anyhow::Error> {
        // Save current session if any
        if !self.current_session.is_empty() {
            let messages = self.agent.messages();
            if !messages.is_empty() {
                self.db
                    .tape_save_messages(&self.current_session, messages)
                    .await?;
            }
        }

        // Load new session
        let mut messages = self.db.tape_load_messages(new_session).await?;

        // Group chat catch-up: only load messages since the last assistant reply.
        // Store the trimmed prefix so we can reconstruct the full tape when saving.
        self.group_catchup_prefix = Vec::new();
        if is_group && !messages.is_empty() {
            let catchup = catchup_messages(messages.clone(), self.max_group_catchup);
            let prefix_len = messages.len() - catchup.len();
            if prefix_len > 0 {
                self.group_catchup_prefix = messages[..prefix_len].to_vec();
            }
            messages = catchup;
            tracing::info!(
                "Group catch-up for {}: loading {} messages ({} preserved in prefix)",
                new_session,
                messages.len(),
                prefix_len,
            );
        }

        if messages.is_empty() {
            self.agent.clear_messages();
        } else {
            let json = serde_json::to_string(&messages)?;
            self.agent.restore_messages(&json)?;
        }

        self.current_session = new_session.to_string();
        *self.session_id_ref.write().unwrap() = new_session.to_string();
        self.budget.reset_turns();

        tracing::info!(
            "Switched to session: {} ({} messages)",
            new_session,
            messages.len()
        );
        Ok(())
    }

    /// Get current session ID.
    pub fn session_id(&self) -> &str {
        &self.current_session
    }

    /// Delegate a message directly to a named worker's sub-agent, bypassing the main conductor.
    /// Used for channel routing (e.g., Discord channel → specific worker).
    pub async fn delegate_to_worker(
        &mut self,
        session_id: &str,
        worker_name: &str,
        text: &str,
    ) -> Result<String, anyhow::Error> {
        if !self.direct_workers.contains_key(worker_name) {
            anyhow::bail!("Worker '{}' not found", worker_name);
        }

        tracing::info!(
            "Delegating to worker '{}' for session {}",
            worker_name,
            session_id
        );

        // Update session_id reference for audit logging
        *self.session_id_ref.write().unwrap() = session_id.to_string();

        // Execute the worker's sub-agent directly
        let params = serde_json::json!({"task": text});
        let ctx = ToolContext {
            tool_call_id: "direct-delegate".to_string(),
            tool_name: worker_name.to_string(),
            cancel: tokio_util::sync::CancellationToken::new(),
            on_update: None,
            on_progress: None,
        };
        let worker_tool = self.direct_workers.get(worker_name).unwrap();
        let result = worker_tool
            .execute(params, ctx)
            .await
            .map_err(|e| anyhow::anyhow!("Worker '{}' failed: {:?}", worker_name, e))?;

        let response = result
            .content
            .iter()
            .filter_map(|c| match c {
                Content::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        // Save current agent state if we're in this session
        if self.current_session == session_id {
            let messages = self.agent.messages();
            self.db.tape_save_messages(session_id, messages).await?;
        }

        // Append the worker exchange to the session tape
        let mut messages = self.db.tape_load_messages(session_id).await?;
        messages.push(AgentMessage::Llm(Message::user(text)));
        messages.push(AgentMessage::Llm(Message::Assistant {
            content: vec![Content::Text {
                text: response.clone(),
            }],
            stop_reason: StopReason::Stop,
            model: format!("worker:{}", worker_name),
            provider: "worker".to_string(),
            usage: Usage::default(),
            timestamp: crate::db::now_ms(),
            error_message: None,
        }));
        self.db.tape_save_messages(session_id, &messages).await?;

        // Invalidate current session so next process_message reloads from tape
        self.current_session = String::new();

        Ok(response)
    }
}

/// For group chats, slice the message tape from the last assistant message onward,
/// capped at `max_messages`. This gives the agent context of what happened since it
/// last spoke, without loading the entire conversation history.
fn catchup_messages(messages: Vec<AgentMessage>, max_messages: usize) -> Vec<AgentMessage> {
    let last_assistant_idx = messages
        .iter()
        .rposition(|msg| matches!(msg, AgentMessage::Llm(Message::Assistant { .. })));
    let sliced = match last_assistant_idx {
        Some(idx) => messages[idx..].to_vec(),
        None => messages, // bot has never replied — use all
    };
    // Cap to max_messages from the end
    if sliced.len() > max_messages {
        sliced[sliced.len() - max_messages..].to_vec()
    } else {
        sliced
    }
}

/// Result of draining an agent event stream.
struct DrainResult {
    response: String,
    /// If input was rejected by a filter (e.g. injection detection).
    input_rejected: Option<String>,
}

/// Drain an AgentEvent receiver and return the final response text.
/// ProgressMessage events are forwarded via the optional callback.
async fn drain_response(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    on_progress: Option<Box<dyn Fn(String) + Send + Sync>>,
) -> DrainResult {
    let mut response = String::new();
    let mut input_rejected = None;
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::ProgressMessage { text, .. } => {
                if let Some(ref cb) = on_progress {
                    cb(text);
                }
            }
            AgentEvent::InputRejected { reason } => {
                input_rejected = Some(reason);
            }
            AgentEvent::AgentEnd { ref messages } => {
                // Extract text from the last assistant message
                for msg in messages.iter().rev() {
                    if let AgentMessage::Llm(Message::Assistant { ref content, .. }) = msg {
                        for c in content {
                            if let Content::Text { ref text } = c {
                                if response.is_empty() {
                                    response = text.clone();
                                }
                            }
                        }
                        if !response.is_empty() {
                            break;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    DrainResult {
        response,
        input_rejected,
    }
}

/// Wrapper that allows `resolve_provider` to return different provider types
/// as a single concrete type that implements `StreamProvider`.
pub struct DynProvider(Box<dyn provider::StreamProvider>);

#[async_trait::async_trait]
impl provider::StreamProvider for DynProvider {
    async fn stream(
        &self,
        config: provider::StreamConfig,
        tx: tokio::sync::mpsc::UnboundedSender<provider::StreamEvent>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<Message, provider::ProviderError> {
        self.0.stream(config, tx, cancel).await
    }
}

/// Resolve a provider name to a StreamProvider implementation.
pub fn resolve_provider(name: &str) -> DynProvider {
    DynProvider(match name {
        "anthropic" => Box::new(provider::AnthropicProvider),
        "openai" => Box::new(provider::OpenAiCompatProvider),
        "google" => Box::new(provider::GoogleProvider),
        "vertex" => Box::new(provider::GoogleVertexProvider),
        "azure" => Box::new(provider::AzureOpenAiProvider),
        "bedrock" => Box::new(provider::BedrockProvider),
        "openai_responses" => Box::new(provider::OpenAiResponsesProvider),
        _ => {
            tracing::warn!("Unknown provider '{}', defaulting to anthropic", name);
            Box::new(provider::AnthropicProvider)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_config;
    use yoagent::provider::MockProvider;

    /// Helper to create a Conductor with MockProvider for testing.
    async fn test_conductor(mock_response: &str) -> (Conductor, Db) {
        let db = Db::open_memory().unwrap();
        let config_str = r#"
[agent]
model = "mock"
api_key = "test-key"
"#;
        let _config = parse_config(config_str).unwrap();

        // Build conductor manually with MockProvider
        let provider = MockProvider::text(mock_response);
        let mut tools: Vec<Box<dyn AgentTool>> = Vec::new();
        tools.push(Box::new(tools::MemorySearchTool::new(db.clone())));
        tools.push(Box::new(tools::MemoryStoreTool::new(db.clone())));

        let budget = BudgetTracker::new(None, None, db.clone());
        let session_id_ref = Arc::new(std::sync::RwLock::new(String::new()));

        let agent = Agent::new(provider)
            .with_system_prompt("You are a test assistant.")
            .with_model("mock")
            .with_api_key("test")
            .with_tools(tools)
            .without_context_management();

        let policy_ref = Arc::new(std::sync::RwLock::new(SecurityPolicy {
            shell_deny_patterns: vec![],
            tool_permissions: HashMap::new(),
        }));
        let conductor = Conductor {
            agent,
            db: db.clone(),
            current_session: String::new(),
            session_id_ref,
            policy_ref,
            budget,
            loaded_skills: Vec::new(),
            worker_infos: Vec::new(),
            direct_workers: HashMap::new(),
            max_group_catchup: 50,
            group_catchup_prefix: Vec::new(),
        };

        (conductor, db)
    }

    #[tokio::test]
    async fn test_process_message() {
        let (mut conductor, _db) = test_conductor("Hello! How can I help?").await;
        let response = conductor
            .process_message("test-session", "Hi there", None)
            .await
            .unwrap();
        assert_eq!(response, "Hello! How can I help?");
    }

    #[tokio::test]
    async fn test_session_persistence() {
        let db = Db::open_memory().unwrap();
        let provider = MockProvider::texts(vec!["Response 1", "Response 2"]);
        let budget = BudgetTracker::new(None, None, db.clone());
        let session_id_ref = Arc::new(std::sync::RwLock::new(String::new()));
        let policy_ref = Arc::new(std::sync::RwLock::new(SecurityPolicy {
            shell_deny_patterns: vec![],
            tool_permissions: HashMap::new(),
        }));

        let agent = Agent::new(provider)
            .with_system_prompt("test")
            .with_model("mock")
            .with_api_key("test")
            .without_context_management();

        let mut conductor = Conductor {
            agent,
            db: db.clone(),
            current_session: String::new(),
            session_id_ref,
            policy_ref,
            budget,
            loaded_skills: Vec::new(),
            worker_infos: Vec::new(),
            direct_workers: HashMap::new(),
            max_group_catchup: 50,
            group_catchup_prefix: Vec::new(),
        };

        // Send a message
        conductor
            .process_message("s1", "Hello", None)
            .await
            .unwrap();

        // Verify it was saved to tape
        let messages = db.tape_load_messages("s1").await.unwrap();
        assert!(!messages.is_empty());
    }

    #[test]
    fn test_catchup_messages_slices_from_last_assistant() {
        let messages = vec![
            AgentMessage::Llm(Message::user("old msg 1")),
            AgentMessage::Llm(Message::Assistant {
                content: vec![Content::Text {
                    text: "old reply".to_string(),
                }],
                stop_reason: StopReason::Stop,
                model: "m".to_string(),
                provider: "p".to_string(),
                usage: Usage::default(),
                timestamp: 0,
                error_message: None,
            }),
            AgentMessage::Llm(Message::user("new msg after reply")),
            AgentMessage::Llm(Message::user("another new msg")),
        ];

        let sliced = catchup_messages(messages, 50);
        // Should include the assistant message + the 2 user messages after it
        assert_eq!(sliced.len(), 3);
        // First should be assistant
        assert!(matches!(
            sliced[0],
            AgentMessage::Llm(Message::Assistant { .. })
        ));
    }

    #[test]
    fn test_catchup_messages_no_assistant() {
        let messages = vec![
            AgentMessage::Llm(Message::user("msg1")),
            AgentMessage::Llm(Message::user("msg2")),
        ];
        let sliced = catchup_messages(messages.clone(), 50);
        // No assistant message — returns all messages
        assert_eq!(sliced.len(), 2);
    }

    #[test]
    fn test_catchup_messages_respects_max() {
        let mut messages = Vec::new();
        messages.push(AgentMessage::Llm(Message::Assistant {
            content: vec![Content::Text {
                text: "reply".to_string(),
            }],
            stop_reason: StopReason::Stop,
            model: "m".to_string(),
            provider: "p".to_string(),
            usage: Usage::default(),
            timestamp: 0,
            error_message: None,
        }));
        // Add 100 user messages after the reply
        for i in 0..100 {
            messages.push(AgentMessage::Llm(Message::user(format!("msg {}", i))));
        }

        let sliced = catchup_messages(messages, 10);
        assert_eq!(sliced.len(), 10);
    }

    #[tokio::test]
    async fn test_process_group_message_catch_up() {
        let db = Db::open_memory().unwrap();

        // Pre-populate tape with old conversation
        let old_messages = vec![
            AgentMessage::Llm(Message::user("old question")),
            AgentMessage::Llm(Message::Assistant {
                content: vec![Content::Text {
                    text: "old answer".to_string(),
                }],
                stop_reason: StopReason::Stop,
                model: "mock".to_string(),
                provider: "mock".to_string(),
                usage: Usage::default(),
                timestamp: 0,
                error_message: None,
            }),
            AgentMessage::Llm(Message::user("new group msg 1")),
            AgentMessage::Llm(Message::user("new group msg 2")),
        ];
        db.tape_save_messages("group-session", &old_messages)
            .await
            .unwrap();

        let provider = MockProvider::text("Group response");
        let budget = BudgetTracker::new(None, None, db.clone());
        let session_id_ref = Arc::new(std::sync::RwLock::new(String::new()));
        let policy_ref = Arc::new(std::sync::RwLock::new(SecurityPolicy {
            shell_deny_patterns: vec![],
            tool_permissions: HashMap::new(),
        }));

        let agent = Agent::new(provider)
            .with_system_prompt("test")
            .with_model("mock")
            .with_api_key("test")
            .without_context_management();

        let mut conductor = Conductor {
            agent,
            db: db.clone(),
            current_session: String::new(),
            session_id_ref,
            policy_ref,
            budget,
            loaded_skills: Vec::new(),
            worker_infos: Vec::new(),
            direct_workers: HashMap::new(),
            max_group_catchup: 50,
            group_catchup_prefix: Vec::new(),
        };

        let response = conductor
            .process_group_message("group-session", "new msg 3", None)
            .await
            .unwrap();
        assert_eq!(response, "Group response");
    }

    #[tokio::test]
    async fn test_drain_response_forwards_progress() {
        use tokio::sync::mpsc;

        let (tx, rx) = mpsc::unbounded_channel();
        let progress_msgs = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let msgs_clone = progress_msgs.clone();

        let on_progress: Box<dyn Fn(String) + Send + Sync> = Box::new(move |text: String| {
            msgs_clone.lock().unwrap().push(text);
        });

        // Simulate events: a progress message followed by agent end
        tx.send(AgentEvent::ProgressMessage {
            tool_call_id: "tc-1".to_string(),
            tool_name: "send_message".to_string(),
            text: "Step 1 done".to_string(),
        })
        .unwrap();
        tx.send(AgentEvent::AgentEnd {
            messages: vec![AgentMessage::Llm(Message::Assistant {
                content: vec![Content::Text {
                    text: "Final response".to_string(),
                }],
                stop_reason: StopReason::Stop,
                model: "mock".to_string(),
                provider: "mock".to_string(),
                usage: Usage::default(),
                timestamp: 0,
                error_message: None,
            })],
        })
        .unwrap();
        drop(tx);

        let result = drain_response(rx, Some(on_progress)).await;
        assert_eq!(result.response, "Final response");
        assert!(result.input_rejected.is_none());
        let captured = progress_msgs.lock().unwrap();
        assert_eq!(&*captured, &["Step 1 done"]);
    }

    #[tokio::test]
    async fn test_group_catchup_preserves_full_tape() {
        let db = Db::open_memory().unwrap();

        // Pre-populate tape with old conversation (4 messages)
        let old_messages = vec![
            AgentMessage::Llm(Message::user("ancient question")),
            AgentMessage::Llm(Message::Assistant {
                content: vec![Content::Text {
                    text: "ancient answer".to_string(),
                }],
                stop_reason: StopReason::Stop,
                model: "mock".to_string(),
                provider: "mock".to_string(),
                usage: Usage::default(),
                timestamp: 0,
                error_message: None,
            }),
            AgentMessage::Llm(Message::user("recent question")),
            AgentMessage::Llm(Message::Assistant {
                content: vec![Content::Text {
                    text: "recent answer".to_string(),
                }],
                stop_reason: StopReason::Stop,
                model: "mock".to_string(),
                provider: "mock".to_string(),
                usage: Usage::default(),
                timestamp: 1,
                error_message: None,
            }),
            AgentMessage::Llm(Message::user("new group msg")),
        ];
        db.tape_save_messages("group-full", &old_messages)
            .await
            .unwrap();

        let provider = MockProvider::text("Group reply");
        let budget = BudgetTracker::new(None, None, db.clone());
        let session_id_ref = Arc::new(std::sync::RwLock::new(String::new()));
        let policy_ref = Arc::new(std::sync::RwLock::new(SecurityPolicy {
            shell_deny_patterns: vec![],
            tool_permissions: HashMap::new(),
        }));

        let agent = Agent::new(provider)
            .with_system_prompt("test")
            .with_model("mock")
            .with_api_key("test")
            .without_context_management();

        let mut conductor = Conductor {
            agent,
            db: db.clone(),
            current_session: String::new(),
            session_id_ref,
            policy_ref,
            budget,
            loaded_skills: Vec::new(),
            worker_infos: Vec::new(),
            direct_workers: HashMap::new(),
            max_group_catchup: 50,
            group_catchup_prefix: Vec::new(),
        };

        // Process a group message — should use catchup slicing
        conductor
            .process_group_message("group-full", "another msg", None)
            .await
            .unwrap();

        // Verify the full tape is preserved (not just the catchup slice)
        let saved = db.tape_load_messages("group-full").await.unwrap();
        // Should contain: ancient question, ancient answer, recent question, recent answer,
        // new group msg (from pre-populated), another msg (new user input), Group reply (new response)
        assert!(
            saved.len() >= old_messages.len(),
            "Full tape was truncated! Expected >= {} messages, got {}",
            old_messages.len(),
            saved.len()
        );
        // The first message should still be the ancient question
        if let AgentMessage::Llm(Message::User { ref content, .. }) = saved[0] {
            if let Some(Content::Text { ref text }) = content.first() {
                assert_eq!(text, "ancient question");
            } else {
                panic!("Expected text content in first message");
            }
        } else {
            panic!("Expected user message as first message");
        }
    }

    #[test]
    fn test_resolve_provider_anthropic() {
        let _p = resolve_provider("anthropic");
    }

    #[test]
    fn test_resolve_provider_openai() {
        let _p = resolve_provider("openai");
    }

    #[test]
    fn test_resolve_provider_unknown_defaults() {
        // Unknown name should not panic — falls back to anthropic
        let _p = resolve_provider("some-unknown-provider");
    }
}
