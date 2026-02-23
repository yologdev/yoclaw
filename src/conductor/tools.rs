use crate::db::Db;
use yoagent::types::*;

/// Tool for searching the agent's long-term memory via FTS5.
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
        "Search the agent's long-term memory using full-text search. Use this to recall previous conversations, user preferences, stored facts, or any context saved earlier."
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
        _tool_call_id: &str,
        params: serde_json::Value,
        _cancel: tokio_util::sync::CancellationToken,
        _on_update: Option<ToolUpdateFn>,
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
                    format!("{}. [{}]{} {}", i + 1, tags, key, m.content)
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
        "Save information to long-term memory for later recall. Use this to remember user preferences, important facts, decisions, or context that should persist across conversations."
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
                }
            },
            "required": ["content"]
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        _cancel: tokio_util::sync::CancellationToken,
        _on_update: Option<ToolUpdateFn>,
    ) -> Result<ToolResult, ToolError> {
        let content = params["content"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("Missing 'content' parameter".into()))?;
        let key = params["key"].as_str();
        let tags = params["tags"].as_str();

        self.db
            .memory_store(key, content, tags, Some("agent"))
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;

        let msg = match key {
            Some(k) => format!("Stored memory with key '{}'.", k),
            None => "Stored memory.".to_string(),
        };

        Ok(ToolResult {
            content: vec![Content::Text { text: msg }],
            details: serde_json::json!({}),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    #[tokio::test]
    async fn test_memory_store_and_search() {
        let db = Db::open_memory().unwrap();
        let store = MemoryStoreTool::new(db.clone());
        let search = MemorySearchTool::new(db);
        let cancel = tokio_util::sync::CancellationToken::new();

        // Store
        let result = store
            .execute(
                "1",
                serde_json::json!({"content": "User prefers dark mode", "key": "theme", "tags": "preference"}),
                cancel.clone(),
                None,
            )
            .await
            .unwrap();
        assert!(content_text(&result.content[0]).contains("Stored"));

        // Search
        let result = search
            .execute(
                "2",
                serde_json::json!({"query": "dark mode"}),
                cancel,
                None,
            )
            .await
            .unwrap();
        assert!(content_text(&result.content[0]).contains("dark mode"));
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
