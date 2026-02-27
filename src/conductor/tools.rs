use crate::db::Db;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use yoagent::types::*;

/// Tool for searching the agent's long-term memory via FTS5 (with temporal decay).
pub struct MemorySearchTool {
    db: Db,
}

impl MemorySearchTool {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl AgentTool for MemorySearchTool {
    fn name(&self) -> &str {
        "memory_search"
    }

    fn label(&self) -> &str {
        "Search Memory"
    }

    fn description(&self) -> &str {
        "Search the agent's long-term memory. Results are ranked by relevance with temporal decay \
         (task memories fade faster than preferences/decisions). Returns category and importance metadata."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query for finding relevant memories"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 10)"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let query = params["query"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("Missing 'query' parameter".into()))?;
        let limit = params["limit"].as_u64().unwrap_or(10) as usize;

        let results = self
            .db
            .memory_search(query, limit)
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;

        let text = if results.is_empty() {
            format!("No memories found for '{}'.", query)
        } else {
            results
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let tags = m.tags.as_deref().unwrap_or("");
                    let key = m
                        .key
                        .as_ref()
                        .map(|k| format!(" (key: {})", k))
                        .unwrap_or_default();
                    format!(
                        "{}. [{}|{}|imp:{}]{} {}",
                        i + 1,
                        m.category,
                        tags,
                        m.importance,
                        key,
                        m.content
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        Ok(ToolResult {
            content: vec![Content::Text { text }],
            details: serde_json::json!({ "count": results.len() }),
        })
    }
}

/// Tool for storing information in the agent's long-term memory.
pub struct MemoryStoreTool {
    db: Db,
}

impl MemoryStoreTool {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl AgentTool for MemoryStoreTool {
    fn name(&self) -> &str {
        "memory_store"
    }

    fn label(&self) -> &str {
        "Store Memory"
    }

    fn description(&self) -> &str {
        "Save information to long-term memory with optional category and importance. Categories: \
         fact, preference, decision, event, task, reflection. Importance: 1-10 (higher = more important, \
         less likely to be pruned). Decisions never decay; tasks decay in ~7 days; preferences persist ~90 days."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The information to remember"
                },
                "key": {
                    "type": "string",
                    "description": "Optional unique key for direct lookup and upsert (e.g. 'user_name', 'preferred_language')"
                },
                "tags": {
                    "type": "string",
                    "description": "Optional comma-separated tags for categorization (e.g. 'preference,user')"
                },
                "category": {
                    "type": "string",
                    "description": "Memory category: fact, preference, decision, event, task, reflection (default: fact)",
                    "enum": ["fact", "preference", "decision", "event", "task", "reflection"]
                },
                "importance": {
                    "type": "integer",
                    "description": "Importance score 1-10 (default: 5). Higher = more important, less likely to be pruned."
                }
            },
            "required": ["content"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let content = params["content"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("Missing 'content' parameter".into()))?;
        let key = params["key"].as_str();
        let tags = params["tags"].as_str();
        let category = params["category"].as_str().unwrap_or("fact");
        let importance = params["importance"].as_i64().unwrap_or(5) as i32;

        self.db
            .memory_store_with_meta(key, content, tags, Some("agent"), category, importance)
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;

        let msg = match key {
            Some(k) => format!(
                "Stored {} memory (importance: {}) with key '{}'.",
                category, importance, k
            ),
            None => format!("Stored {} memory (importance: {}).", category, importance),
        };

        Ok(ToolResult {
            content: vec![Content::Text { text: msg }],
            details: serde_json::json!({}),
        })
    }
}

/// Tool that lets the agent send a message to the user mid-task via progress events.
/// The message is delivered immediately through the channel adapter, NOT stored in tape.
pub struct SendMessageTool;

#[async_trait::async_trait]
impl AgentTool for SendMessageTool {
    fn name(&self) -> &str {
        "send_message"
    }

    fn label(&self) -> &str {
        "Send Message"
    }

    fn description(&self) -> &str {
        "Send a message to the user immediately without waiting for the full response. \
         Use this to provide progress updates, ask follow-up questions during long tasks, \
         or deliver partial results. The message is delivered in real-time."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "The message to send to the user immediately"
                }
            },
            "required": ["message"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let message = params["message"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("Missing 'message' parameter".into()))?;

        // Emit via progress callback — this will be routed to the channel adapter
        if let Some(ref on_progress) = ctx.on_progress {
            on_progress(message.to_string());
        }

        Ok(ToolResult {
            content: vec![Content::Text {
                text: "Message sent.".to_string(),
            }],
            details: serde_json::json!({}),
        })
    }
}

// ---------------------------------------------------------------------------
// Dynamic Worker Tools
// ---------------------------------------------------------------------------

/// Tool for spawning a dynamic worker sub-agent at runtime.
pub struct SpawnWorkerTool {
    db: Db,
    provider: Arc<dyn yoagent::provider::StreamProvider>,
    model: String,
    api_key: String,
    worker_tools: Vec<Arc<dyn AgentTool>>,
    active_count: Arc<AtomicUsize>,
    max_concurrent: usize,
    max_turns: usize,
}

/// Config for creating a SpawnWorkerTool.
pub struct SpawnWorkerConfig {
    pub db: Db,
    pub provider: Arc<dyn yoagent::provider::StreamProvider>,
    pub model: String,
    pub api_key: String,
    pub worker_tools: Vec<Arc<dyn AgentTool>>,
    pub active_count: Arc<AtomicUsize>,
    pub max_concurrent: usize,
    pub max_turns: usize,
}

impl SpawnWorkerTool {
    pub fn new(config: SpawnWorkerConfig) -> Self {
        Self {
            db: config.db,
            provider: config.provider,
            model: config.model,
            api_key: config.api_key,
            worker_tools: config.worker_tools,
            active_count: config.active_count,
            max_concurrent: config.max_concurrent,
            max_turns: config.max_turns,
        }
    }
}

#[async_trait::async_trait]
impl AgentTool for SpawnWorkerTool {
    fn name(&self) -> &str {
        "spawn_worker"
    }

    fn label(&self) -> &str {
        "Spawn Worker"
    }

    fn description(&self) -> &str {
        "Spawn a dynamic sub-agent to handle a specific task. The worker runs with its own system \
         prompt and returns the result. Use 'save: true' to save the worker definition for reuse. \
         If 'system_prompt' is omitted, looks up a previously saved worker by name."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Short name for the worker"
                },
                "system_prompt": {
                    "type": "string",
                    "description": "System prompt for the worker (omit to use a saved worker)"
                },
                "task": {
                    "type": "string",
                    "description": "The task to delegate to the worker"
                },
                "save": {
                    "type": "boolean",
                    "description": "Save this worker definition for reuse (default: false)"
                }
            },
            "required": ["name", "task"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let name = params["name"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("Missing 'name' parameter".into()))?;
        let task = params["task"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("Missing 'task' parameter".into()))?;
        let save = params["save"].as_bool().unwrap_or(false);

        // Resolve system prompt: param > saved worker > error
        let system_prompt = if let Some(prompt) = params["system_prompt"].as_str() {
            prompt.to_string()
        } else {
            match self.db.saved_workers_get(name).await {
                Ok(Some(w)) => w.system_prompt,
                Ok(None) => {
                    return Err(ToolError::InvalidArgs(format!(
                        "No system_prompt provided and no saved worker named '{}'",
                        name
                    )));
                }
                Err(e) => return Err(ToolError::Failed(format!("DB error: {}", e))),
            }
        };

        // Check concurrent limit
        let current = self.active_count.fetch_add(1, Ordering::SeqCst);
        if current >= self.max_concurrent {
            self.active_count.fetch_sub(1, Ordering::SeqCst);
            return Err(ToolError::Failed(format!(
                "Max concurrent workers reached ({}/{})",
                current, self.max_concurrent
            )));
        }

        // Report progress
        if let Some(ref on_progress) = ctx.on_progress {
            on_progress(format!("Spawning worker '{}'...", name));
        }

        // Build and run ephemeral sub-agent
        let sub = yoagent::sub_agent::SubAgentTool::new(name, self.provider.clone())
            .with_system_prompt(&system_prompt)
            .with_model(&self.model)
            .with_api_key(&self.api_key)
            .with_max_turns(self.max_turns)
            .with_tools(self.worker_tools.clone());

        let sub_ctx = ToolContext {
            tool_call_id: ctx.tool_call_id.clone(),
            tool_name: name.to_string(),
            cancel: ctx.cancel.clone(),
            on_update: ctx.on_update.clone(),
            on_progress: ctx.on_progress.clone(),
        };

        let result = sub
            .execute(serde_json::json!({"task": task}), sub_ctx)
            .await;

        // Decrement active count
        self.active_count.fetch_sub(1, Ordering::SeqCst);

        // Save if requested
        if save {
            if let Err(e) = self.db.saved_workers_upsert(name, &system_prompt).await {
                tracing::warn!("Failed to save worker '{}': {}", name, e);
            }
        }

        result
    }
}

/// Tool for listing saved dynamic workers.
pub struct ListWorkersTool {
    db: Db,
}

impl ListWorkersTool {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl AgentTool for ListWorkersTool {
    fn name(&self) -> &str {
        "list_workers"
    }

    fn label(&self) -> &str {
        "List Workers"
    }

    fn description(&self) -> &str {
        "List all saved dynamic worker definitions that can be reused with spawn_worker."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(
        &self,
        _params: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let workers = self
            .db
            .saved_workers_list()
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;

        let text = if workers.is_empty() {
            "No saved workers.".to_string()
        } else {
            workers
                .iter()
                .map(|w| {
                    let snippet = if w.system_prompt.len() > 80 {
                        format!("{}...", &w.system_prompt[..80])
                    } else {
                        w.system_prompt.clone()
                    };
                    format!("- {} — \"{}\"", w.name, snippet)
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        Ok(ToolResult {
            content: vec![Content::Text { text }],
            details: serde_json::json!({ "count": workers.len() }),
        })
    }
}

/// Tool for removing a saved dynamic worker.
pub struct RemoveWorkerTool {
    db: Db,
}

impl RemoveWorkerTool {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl AgentTool for RemoveWorkerTool {
    fn name(&self) -> &str {
        "remove_worker"
    }

    fn label(&self) -> &str {
        "Remove Worker"
    }

    fn description(&self) -> &str {
        "Remove a saved dynamic worker definition by name."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the saved worker to remove"
                }
            },
            "required": ["name"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let name = params["name"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("Missing 'name' parameter".into()))?;

        let deleted = self
            .db
            .saved_workers_remove(name)
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;

        let text = if deleted {
            format!("Worker '{}' removed.", name)
        } else {
            format!("No saved worker named '{}'.", name)
        };

        Ok(ToolResult {
            content: vec![Content::Text { text }],
            details: serde_json::json!({}),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    fn test_ctx() -> ToolContext {
        ToolContext {
            tool_call_id: "test".to_string(),
            tool_name: "test".to_string(),
            cancel: tokio_util::sync::CancellationToken::new(),
            on_update: None,
            on_progress: None,
        }
    }

    #[tokio::test]
    async fn test_memory_store_and_search() {
        let db = Db::open_memory().unwrap();
        let store = MemoryStoreTool::new(db.clone());
        let search = MemorySearchTool::new(db);

        // Store
        let result = store
            .execute(
                serde_json::json!({"content": "User prefers dark mode", "key": "theme", "tags": "preference"}),
                test_ctx(),
            )
            .await
            .unwrap();
        assert!(content_text(&result.content[0]).contains("Stored"));

        // Search
        let result = search
            .execute(serde_json::json!({"query": "dark mode"}), test_ctx())
            .await
            .unwrap();
        assert!(content_text(&result.content[0]).contains("dark mode"));
    }

    #[tokio::test]
    async fn test_send_message_tool_with_progress() {
        let tool = SendMessageTool;
        let progress_msgs = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let msgs_clone = progress_msgs.clone();

        let ctx = ToolContext {
            tool_call_id: "tc-1".to_string(),
            tool_name: "send_message".to_string(),
            cancel: tokio_util::sync::CancellationToken::new(),
            on_update: None,
            on_progress: Some(std::sync::Arc::new(move |text: String| {
                msgs_clone.lock().unwrap().push(text);
            })),
        };

        let result = tool
            .execute(serde_json::json!({"message": "Processing step 1..."}), ctx)
            .await
            .unwrap();

        assert!(content_text(&result.content[0]).contains("Message sent"));
        let captured = progress_msgs.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0], "Processing step 1...");
    }

    #[tokio::test]
    async fn test_send_message_tool_without_progress() {
        let tool = SendMessageTool;
        // No on_progress callback — should still succeed without error
        let result = tool
            .execute(serde_json::json!({"message": "Hello"}), test_ctx())
            .await
            .unwrap();
        assert!(content_text(&result.content[0]).contains("Message sent"));
    }

    #[tokio::test]
    async fn test_send_message_tool_missing_param() {
        let tool = SendMessageTool;
        let result = tool.execute(serde_json::json!({}), test_ctx()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_memory_store_with_category() {
        let db = Db::open_memory().unwrap();
        let store = MemoryStoreTool::new(db.clone());

        let result = store
            .execute(
                serde_json::json!({
                    "content": "Always use bun instead of npm",
                    "category": "decision",
                    "importance": 9
                }),
                test_ctx(),
            )
            .await
            .unwrap();
        assert!(content_text(&result.content[0]).contains("decision"));
        assert!(content_text(&result.content[0]).contains("9"));
    }

    // --- Dynamic Worker Tests ---

    #[tokio::test]
    async fn test_spawn_worker_basic() {
        use yoagent::provider::MockProvider;

        let db = Db::open_memory().unwrap();
        let provider = Arc::new(MockProvider::text("Worker result here"));
        let active_count = Arc::new(AtomicUsize::new(0));

        let tool = SpawnWorkerTool::new(SpawnWorkerConfig {
            db,
            provider,
            model: "mock".into(),
            api_key: "test".into(),
            worker_tools: vec![],
            active_count: active_count.clone(),
            max_concurrent: 3,
            max_turns: 10,
        });

        let result = tool
            .execute(
                serde_json::json!({
                    "name": "test-worker",
                    "system_prompt": "You are a test worker.",
                    "task": "Do something"
                }),
                test_ctx(),
            )
            .await
            .unwrap();

        assert!(content_text(&result.content[0]).contains("Worker result here"));
        // Active count should be back to 0
        assert_eq!(active_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_spawn_worker_concurrent_limit() {
        use yoagent::provider::MockProvider;

        let db = Db::open_memory().unwrap();
        let provider = Arc::new(MockProvider::text("ok"));
        let active_count = Arc::new(AtomicUsize::new(3)); // Already at max

        let tool = SpawnWorkerTool::new(SpawnWorkerConfig {
            db,
            provider,
            model: "mock".into(),
            api_key: "test".into(),
            worker_tools: vec![],
            active_count,
            max_concurrent: 3,
            max_turns: 10,
        });

        let result = tool
            .execute(
                serde_json::json!({
                    "name": "worker",
                    "system_prompt": "test",
                    "task": "do stuff"
                }),
                test_ctx(),
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Max concurrent workers"));
    }

    #[tokio::test]
    async fn test_spawn_worker_no_recursive_spawn() {
        // The spawn_worker tool should NOT be in the worker's tool list.
        // This is enforced by the Conductor's wiring, not by the tool itself.
        // Here we verify the tool_list passed to SpawnWorkerTool excludes spawn_worker.

        // Just verify the tool names we pass don't include spawn_worker
        let worker_tools: Vec<Arc<dyn AgentTool>> =
            vec![Arc::new(MemorySearchTool::new(Db::open_memory().unwrap()))];
        for t in &worker_tools {
            assert_ne!(t.name(), "spawn_worker");
            assert_ne!(t.name(), "list_workers");
            assert_ne!(t.name(), "remove_worker");
        }
    }

    #[tokio::test]
    async fn test_saved_workers_lifecycle() {
        let db = Db::open_memory().unwrap();
        let list_tool = ListWorkersTool::new(db.clone());
        let remove_tool = RemoveWorkerTool::new(db.clone());

        // Initially empty
        let result = list_tool
            .execute(serde_json::json!({}), test_ctx())
            .await
            .unwrap();
        assert!(content_text(&result.content[0]).contains("No saved workers"));

        // Save a worker via DB directly
        db.saved_workers_upsert("researcher", "You are a research assistant.")
            .await
            .unwrap();

        // List should now show it
        let result = list_tool
            .execute(serde_json::json!({}), test_ctx())
            .await
            .unwrap();
        assert!(content_text(&result.content[0]).contains("researcher"));

        // Remove it
        let result = remove_tool
            .execute(serde_json::json!({"name": "researcher"}), test_ctx())
            .await
            .unwrap();
        assert!(content_text(&result.content[0]).contains("removed"));

        // List should be empty again
        let result = list_tool
            .execute(serde_json::json!({}), test_ctx())
            .await
            .unwrap();
        assert!(content_text(&result.content[0]).contains("No saved workers"));
    }

    #[tokio::test]
    async fn test_spawn_worker_uses_saved_prompt() {
        use yoagent::provider::MockProvider;

        let db = Db::open_memory().unwrap();
        // Save a worker definition
        db.saved_workers_upsert("cached-worker", "You are a saved worker.")
            .await
            .unwrap();

        let provider = Arc::new(MockProvider::text("Saved worker result"));
        let active_count = Arc::new(AtomicUsize::new(0));

        let tool = SpawnWorkerTool::new(SpawnWorkerConfig {
            db,
            provider,
            model: "mock".into(),
            api_key: "test".into(),
            worker_tools: vec![],
            active_count,
            max_concurrent: 3,
            max_turns: 10,
        });

        // Spawn without system_prompt — should use saved definition
        let result = tool
            .execute(
                serde_json::json!({
                    "name": "cached-worker",
                    "task": "Do something"
                }),
                test_ctx(),
            )
            .await
            .unwrap();

        assert!(content_text(&result.content[0]).contains("Saved worker result"));
    }

    #[tokio::test]
    async fn test_spawn_worker_no_saved_no_prompt_errors() {
        use yoagent::provider::MockProvider;

        let db = Db::open_memory().unwrap();
        let provider = Arc::new(MockProvider::text("ok"));
        let active_count = Arc::new(AtomicUsize::new(0));

        let tool = SpawnWorkerTool::new(SpawnWorkerConfig {
            db,
            provider,
            model: "mock".into(),
            api_key: "test".into(),
            worker_tools: vec![],
            active_count,
            max_concurrent: 3,
            max_turns: 10,
        });

        let result = tool
            .execute(
                serde_json::json!({
                    "name": "nonexistent",
                    "task": "Do something"
                }),
                test_ctx(),
            )
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No system_prompt"));
    }
}

/// Helper: extract text from Content (test-only).
#[cfg(test)]
fn content_text(c: &Content) -> &str {
    match c {
        Content::Text { text } => text,
        _ => "",
    }
}
