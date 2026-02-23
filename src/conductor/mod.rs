pub mod delegate;
pub mod tools;

use crate::config::Config;
use crate::db::Db;
use crate::security::budget::BudgetTracker;
use crate::security::{self, SecurityPolicy};
use crate::skills::LoadedSkill;
use delegate::WorkerInfo;
use std::sync::Arc;
use yoagent::provider::AnthropicProvider;
use yoagent::types::*;
use yoagent::Agent;

/// The Conductor owns the yoagent Agent and mediates all interactions.
pub struct Conductor {
    agent: Agent,
    db: Db,
    current_session: String,
    session_id_ref: Arc<std::sync::RwLock<String>>,
    budget: BudgetTracker,
    loaded_skills: Vec<LoadedSkill>,
    worker_infos: Vec<WorkerInfo>,
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
        let policy = Arc::new(SecurityPolicy::from_config(&config.security));
        let (skills_prompt, loaded_skills) =
            crate::skills::load_filtered_skills(&skills_refs, &policy);

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

        // 4. Wrap with security
        let session_id_ref = Arc::new(std::sync::RwLock::new(String::new()));
        let mut wrapped_tools =
            security::wrap_tools(tool_list, policy.clone(), db.clone(), session_id_ref.clone());

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
                policy: policy.clone(),
                db: db.clone(),
                session_id: session_id_ref.clone(),
            }),
            Arc::new(security::SecureToolWrapper {
                inner: Box::new(tools::MemoryStoreTool::new(db.clone())),
                policy: policy.clone(),
                db: db.clone(),
                session_id: session_id_ref.clone(),
            }),
        ];
        let workers = delegate::build_workers(config, &worker_tools);
        let worker_infos: Vec<WorkerInfo> = workers.iter().map(|(_, info)| info.clone()).collect();

        if !worker_infos.is_empty() {
            tracing::info!("Configured {} worker(s)", worker_infos.len());
        }

        // Wrap each SubAgentTool with SecureToolWrapper so worker delegations
        // are audit-logged and security-checked (Gap 1 fix)
        for (sub_agent, _info) in workers {
            wrapped_tools.push(Box::new(security::SecureToolWrapper {
                inner: Box::new(sub_agent),
                policy: policy.clone(),
                db: db.clone(),
                session_id: session_id_ref.clone(),
            }));
        }

        // 7. Resolve provider
        let provider = resolve_provider(&config.agent.provider);

        // 8. Build agent — workers are included in wrapped_tools, no with_sub_agent needed
        let budget_check = budget.clone();
        let budget_record = budget.clone();
        let mut agent = Agent::new(provider)
            .with_system_prompt(&persona)
            .with_model(&config.agent.model)
            .with_api_key(&config.agent.api_key)
            .with_tools(wrapped_tools)
            .on_before_turn(move |_messages, _turn| budget_check.can_continue())
            .on_after_turn(move |_messages, usage| {
                budget_record.record_usage(usage.input, usage.output);
                budget_record.record_turn();
            });

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
            budget,
            loaded_skills,
            worker_infos,
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

    /// Process a user message and return the assistant's text response.
    pub async fn process_message(
        &mut self,
        session_id: &str,
        text: &str,
    ) -> Result<String, anyhow::Error> {
        // Switch session if needed
        if self.current_session != session_id {
            self.switch_session(session_id).await?;
        }

        // Run the agent
        let rx = self.agent.prompt(text).await;

        // Drain events and collect response
        let response = drain_response(rx).await;

        // Persist conversation state
        let messages = self.agent.messages();
        self.db
            .tape_save_messages(session_id, messages)
            .await?;

        Ok(response)
    }

    async fn switch_session(&mut self, new_session: &str) -> Result<(), anyhow::Error> {
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
        let messages = self.db.tape_load_messages(new_session).await?;
        if messages.is_empty() {
            self.agent.clear_messages();
        } else {
            let json = serde_json::to_string(&messages)?;
            self.agent.restore_messages(&json)?;
        }

        self.current_session = new_session.to_string();
        *self.session_id_ref.write().unwrap() = new_session.to_string();
        self.budget.reset_turns();

        tracing::info!("Switched to session: {} ({} messages)", new_session, messages.len());
        Ok(())
    }

    /// Get current session ID.
    pub fn session_id(&self) -> &str {
        &self.current_session
    }
}

/// Drain an AgentEvent receiver and return the final response text.
async fn drain_response(mut rx: tokio::sync::mpsc::UnboundedReceiver<AgentEvent>) -> String {
    let mut response = String::new();
    while let Some(event) = rx.recv().await {
        match event {
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
    response
}

/// Resolve a provider name to a StreamProvider implementation.
fn resolve_provider(name: &str) -> impl yoagent::provider::StreamProvider + 'static {
    // For now, always return AnthropicProvider.
    // TODO: support openai, google, etc. via ProviderRegistry
    match name {
        "anthropic" => AnthropicProvider,
        _ => {
            tracing::warn!("Unknown provider '{}', defaulting to anthropic", name);
            AnthropicProvider
        }
    }
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

        let conductor = Conductor {
            agent,
            db: db.clone(),
            current_session: String::new(),
            session_id_ref,
            budget,
            loaded_skills: Vec::new(),
            worker_infos: Vec::new(),
        };

        (conductor, db)
    }

    #[tokio::test]
    async fn test_process_message() {
        let (mut conductor, _db) = test_conductor("Hello! How can I help?").await;
        let response = conductor
            .process_message("test-session", "Hi there")
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
            budget,
            loaded_skills: Vec::new(),
            worker_infos: Vec::new(),
        };

        // Send a message
        conductor
            .process_message("s1", "Hello")
            .await
            .unwrap();

        // Verify it was saved to tape
        let messages = db.tape_load_messages("s1").await.unwrap();
        assert!(!messages.is_empty());
    }
}
