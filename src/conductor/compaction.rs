use crate::db::Db;
use std::sync::{Arc, RwLock};
use yoagent::context::{compact_messages, total_tokens, CompactionStrategy, ContextConfig};
use yoagent::types::*;

/// Compaction strategy that saves dropped conversation content to memory
/// before removal, making it searchable via MemorySearchTool.
pub struct MemoryAwareCompaction {
    db: Db,
    session_id: Arc<RwLock<String>>,
}

impl MemoryAwareCompaction {
    pub fn new(db: Db, session_id: Arc<RwLock<String>>) -> Self {
        Self { db, session_id }
    }
}

impl CompactionStrategy for MemoryAwareCompaction {
    fn compact(&self, messages: Vec<AgentMessage>, config: &ContextConfig) -> Vec<AgentMessage> {
        let budget = config
            .max_context_tokens
            .saturating_sub(config.system_prompt_tokens);
        if total_tokens(&messages) <= budget {
            return messages;
        }

        // Extract text from the droppable zone before compaction
        let keep_first = config.keep_first.min(messages.len());
        let keep_recent = config
            .keep_recent
            .min(messages.len().saturating_sub(keep_first));
        let drop_end = messages.len().saturating_sub(keep_recent);

        let droppable_text = if drop_end > keep_first {
            extract_text_content(&messages[keep_first..drop_end])
        } else {
            String::new()
        };

        let original_len = messages.len();
        let compacted = compact_messages(messages, config);

        // If messages were actually dropped, store extracted text to memory
        if compacted.len() < original_len && !droppable_text.is_empty() {
            let dropped_count = original_len - compacted.len();
            // Truncate to ~4000 chars to avoid storing excessive content
            let content = if droppable_text.len() > 4000 {
                let mut boundary = 4000;
                while boundary > 0 && !droppable_text.is_char_boundary(boundary) {
                    boundary -= 1;
                }
                format!("{}... [truncated]", &droppable_text[..boundary])
            } else {
                droppable_text
            };

            let session_id = self.session_id.read().unwrap().clone();
            let source = format!("compaction:{}", session_id);
            if let Err(e) = self
                .db
                .memory_store_compacted(&content, &source, dropped_count)
            {
                tracing::warn!("Failed to store compacted context to memory: {}", e);
            } else {
                tracing::info!(
                    "Stored {} dropped messages to memory for session {}",
                    dropped_count,
                    session_id,
                );
            }
        }

        compacted
    }
}

/// Extract user and assistant text content from messages, skipping tool calls,
/// tool results, and summary markers.
fn extract_text_content(messages: &[AgentMessage]) -> String {
    let mut parts = Vec::new();
    for msg in messages {
        if let AgentMessage::Llm(llm_msg) = msg {
            match llm_msg {
                Message::User { content, .. } => {
                    for c in content {
                        if let Content::Text { text } = c {
                            if !text.starts_with("[Summary]")
                                && !text.starts_with("[Context compacted")
                            {
                                parts.push(format!("User: {}", text));
                            }
                        }
                    }
                }
                Message::Assistant { content, .. } => {
                    for c in content {
                        if let Content::Text { text } = c {
                            if !text.starts_with("[Summary]")
                                && !text.starts_with("[Context compacted")
                            {
                                parts.push(format!("Assistant: {}", text));
                            }
                        }
                    }
                }
                Message::ToolResult { .. } => {} // skip tool results
            }
        }
    }
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_user_msg(text: &str) -> AgentMessage {
        AgentMessage::Llm(Message::user(text))
    }

    fn make_assistant_msg(text: &str) -> AgentMessage {
        AgentMessage::Llm(Message::Assistant {
            content: vec![Content::Text {
                text: text.to_string(),
            }],
            stop_reason: StopReason::Stop,
            model: "mock".to_string(),
            provider: "mock".to_string(),
            usage: Usage::default(),
            timestamp: 0,
            error_message: None,
        })
    }

    fn make_tool_call_msg(name: &str) -> AgentMessage {
        AgentMessage::Llm(Message::Assistant {
            content: vec![Content::ToolCall {
                id: "tc-1".to_string(),
                name: name.to_string(),
                arguments: serde_json::json!({}),
            }],
            stop_reason: StopReason::ToolUse,
            model: "mock".to_string(),
            provider: "mock".to_string(),
            usage: Usage::default(),
            timestamp: 0,
            error_message: None,
        })
    }

    fn make_tool_result_msg(name: &str, text: &str) -> AgentMessage {
        AgentMessage::Llm(Message::ToolResult {
            tool_call_id: "tc-1".to_string(),
            tool_name: name.to_string(),
            content: vec![Content::Text {
                text: text.to_string(),
            }],
            is_error: false,
            timestamp: 0,
        })
    }

    #[test]
    fn test_no_compaction_when_within_budget() {
        let db = Db::open_memory().unwrap();
        let session_id = Arc::new(RwLock::new("test-session".to_string()));
        let strategy = MemoryAwareCompaction::new(db.clone(), session_id);

        let messages = vec![make_user_msg("Hello"), make_assistant_msg("Hi there!")];

        let config = ContextConfig {
            max_context_tokens: 100_000,
            system_prompt_tokens: 1_000,
            keep_recent: 10,
            keep_first: 2,
            tool_output_max_lines: 50,
        };

        let result = strategy.compact(messages.clone(), &config);
        assert_eq!(result.len(), messages.len());

        // No memory should have been stored
        let memories = db
            .exec_sync(|conn| {
                let count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM memory WHERE category = 'context'",
                    [],
                    |r| r.get(0),
                )?;
                Ok(count)
            })
            .unwrap();
        assert_eq!(memories, 0);
    }

    #[test]
    fn test_compaction_stores_dropped_context() {
        let db = Db::open_memory().unwrap();
        let session_id = Arc::new(RwLock::new("tg-123".to_string()));
        let strategy = MemoryAwareCompaction::new(db.clone(), session_id);

        // Build many messages that exceed a tiny budget
        let mut messages = Vec::new();
        for i in 0..20 {
            messages.push(make_user_msg(&format!("Question number {}", i)));
            messages.push(make_assistant_msg(&format!(
                "This is a detailed answer to question {}. {}",
                i,
                "x".repeat(200)
            )));
        }

        let config = ContextConfig {
            max_context_tokens: 100, // very tight budget to force compaction
            system_prompt_tokens: 10,
            keep_recent: 2,
            keep_first: 2,
            tool_output_max_lines: 50,
        };

        let original_len = messages.len();
        let result = strategy.compact(messages, &config);
        assert!(
            result.len() < original_len,
            "Messages should have been compacted"
        );

        // Verify memory was stored
        let (count, source, category) = db
            .exec_sync(|conn| {
                let row = conn.query_row(
                    "SELECT COUNT(*), source, category FROM memory WHERE category = 'context'",
                    [],
                    |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                        ))
                    },
                )?;
                Ok(row)
            })
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(source, "compaction:tg-123");
        assert_eq!(category, "context");
    }

    #[test]
    fn test_extract_text_content_skips_tool_results() {
        let messages = vec![
            make_user_msg("What is the weather?"),
            make_tool_call_msg("get_weather"),
            make_tool_result_msg("get_weather", "Sunny, 72F"),
            make_assistant_msg("The weather is sunny and 72F."),
            make_user_msg("[Summary] Previous conversation summary"),
            make_assistant_msg("[Context compacted] Earlier messages removed"),
        ];

        let text = extract_text_content(&messages);

        // Should include user question and assistant answer
        assert!(text.contains("User: What is the weather?"));
        assert!(text.contains("Assistant: The weather is sunny and 72F."));

        // Should NOT include tool results or summary markers
        assert!(!text.contains("Sunny, 72F"));
        assert!(!text.contains("get_weather"));
        assert!(!text.contains("[Summary]"));
        assert!(!text.contains("[Context compacted"));
    }

    #[test]
    fn test_large_content_truncated() {
        let db = Db::open_memory().unwrap();
        let session_id = Arc::new(RwLock::new("tg-456".to_string()));
        let strategy = MemoryAwareCompaction::new(db.clone(), session_id);

        // Build messages with very long content in the droppable zone
        let mut messages = Vec::new();
        messages.push(make_user_msg("first")); // keep_first
        messages.push(make_assistant_msg("first reply")); // keep_first
        for i in 0..10 {
            messages.push(make_user_msg(&format!(
                "Long question {}: {}",
                i,
                "a".repeat(500)
            )));
            messages.push(make_assistant_msg(&format!(
                "Long answer {}: {}",
                i,
                "b".repeat(500)
            )));
        }
        messages.push(make_user_msg("recent")); // keep_recent
        messages.push(make_assistant_msg("recent reply")); // keep_recent

        let config = ContextConfig {
            max_context_tokens: 100, // very tight to force compaction
            system_prompt_tokens: 10,
            keep_recent: 2,
            keep_first: 2,
            tool_output_max_lines: 50,
        };

        let _ = strategy.compact(messages, &config);

        // Verify stored content is truncated
        let content = db
            .exec_sync(|conn| {
                let c: String = conn.query_row(
                    "SELECT content FROM memory WHERE category = 'context'",
                    [],
                    |r| r.get(0),
                )?;
                Ok(c)
            })
            .unwrap();

        assert!(
            content.len() <= 4200,
            "Content should be truncated to ~4000 chars, got {}",
            content.len()
        );
        assert!(content.ends_with("... [truncated]"));
    }
}
