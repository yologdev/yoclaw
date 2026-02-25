//! AgentTool for managing cron jobs conversationally.

use crate::db::Db;
use yoagent::types::*;

/// Tool for the agent to create, list, and delete cron jobs.
pub struct CronScheduleTool {
    db: Db,
}

impl CronScheduleTool {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl AgentTool for CronScheduleTool {
    fn name(&self) -> &str {
        "cron_schedule"
    }

    fn label(&self) -> &str {
        "Manage Cron Jobs"
    }

    fn description(&self) -> &str {
        "Create, list, delete, or toggle scheduled cron jobs. Jobs run on a cron schedule \
         and can deliver results to a configured channel. Actions: 'create' (new job), \
         'list' (show all jobs), 'delete' (remove a job by name), 'toggle' (enable/disable a job)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "The action to perform",
                    "enum": ["create", "list", "delete", "toggle"]
                },
                "name": {
                    "type": "string",
                    "description": "Job name (required for create, delete, toggle)"
                },
                "schedule": {
                    "type": "string",
                    "description": "Cron expression, e.g. '0 9 * * *' for 9am daily (required for create)"
                },
                "prompt": {
                    "type": "string",
                    "description": "The prompt/task for the agent to execute on schedule (required for create)"
                },
                "target": {
                    "type": "string",
                    "description": "Target channel to deliver results (e.g. 'telegram', 'discord')"
                },
                "session": {
                    "type": "string",
                    "description": "Session mode: 'isolated' (fresh session per run) or 'main' (inject into current session)",
                    "enum": ["isolated", "main"]
                },
                "enabled": {
                    "type": "boolean",
                    "description": "For toggle action: whether to enable (true) or disable (false) the job"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let action = params["action"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("Missing 'action' parameter".into()))?;

        let text = match action {
            "create" => self.handle_create(&params).await?,
            "list" => self.handle_list().await?,
            "delete" => self.handle_delete(&params).await?,
            "toggle" => self.handle_toggle(&params).await?,
            _ => {
                return Err(ToolError::InvalidArgs(format!(
                    "Unknown action: {}",
                    action
                )))
            }
        };

        Ok(ToolResult {
            content: vec![Content::Text { text }],
            details: serde_json::json!({}),
        })
    }
}

impl CronScheduleTool {
    async fn handle_create(&self, params: &serde_json::Value) -> Result<String, ToolError> {
        let name = params["name"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("Missing 'name' for create".into()))?;
        let schedule = params["schedule"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("Missing 'schedule' for create".into()))?;
        let prompt = params["prompt"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("Missing 'prompt' for create".into()))?;
        let target = params["target"].as_str();
        let session = params["session"].as_str().unwrap_or("isolated");

        super::cron::create_job(&self.db, name, schedule, prompt, target, session)
            .await
            .map_err(|e| ToolError::Failed(format!("Failed to create job: {}", e)))?;

        Ok(format!(
            "Created cron job '{}' with schedule '{}'. Target: {}. Session: {}.",
            name,
            schedule,
            target.unwrap_or("none"),
            session
        ))
    }

    async fn handle_list(&self) -> Result<String, ToolError> {
        let jobs = super::cron::list_jobs(&self.db)
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;

        if jobs.is_empty() {
            return Ok("No cron jobs configured.".to_string());
        }

        let lines: Vec<String> = jobs
            .iter()
            .map(|j| {
                let status = if j.enabled { "enabled" } else { "disabled" };
                let target = j.target_channel.as_deref().unwrap_or("none");
                format!(
                    "- {} [{}] schedule='{}' target={} session={} prompt='{}'",
                    j.name,
                    status,
                    j.schedule,
                    target,
                    j.session_mode,
                    truncate_str(&j.prompt, 60)
                )
            })
            .collect();

        Ok(format!("{} cron job(s):\n{}", jobs.len(), lines.join("\n")))
    }

    async fn handle_delete(&self, params: &serde_json::Value) -> Result<String, ToolError> {
        let name = params["name"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("Missing 'name' for delete".into()))?;

        let deleted = super::cron::delete_job(&self.db, name)
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;

        if deleted {
            Ok(format!("Deleted cron job '{}'.", name))
        } else {
            Ok(format!("No cron job named '{}' found.", name))
        }
    }

    async fn handle_toggle(&self, params: &serde_json::Value) -> Result<String, ToolError> {
        let name = params["name"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("Missing 'name' for toggle".into()))?;
        let enabled = params["enabled"]
            .as_bool()
            .ok_or_else(|| ToolError::InvalidArgs("Missing 'enabled' (bool) for toggle".into()))?;

        let result = super::cron::toggle_job(&self.db, name, enabled)
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;

        match result {
            Some(true) => Ok(format!("Enabled cron job '{}'.", name)),
            Some(false) => Ok(format!("Disabled cron job '{}'.", name)),
            None => Ok(format!("No cron job named '{}' found.", name)),
        }
    }
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    fn test_ctx() -> ToolContext {
        ToolContext {
            tool_call_id: "test".to_string(),
            tool_name: "cron_schedule".to_string(),
            cancel: tokio_util::sync::CancellationToken::new(),
            on_update: None,
            on_progress: None,
        }
    }

    #[tokio::test]
    async fn test_cron_tool_create_and_list() {
        let db = Db::open_memory().unwrap();
        let tool = CronScheduleTool::new(db);

        // Create a job
        let result = tool
            .execute(
                serde_json::json!({
                    "action": "create",
                    "name": "morning-check",
                    "schedule": "0 9 * * *",
                    "prompt": "Check my email",
                    "target": "telegram"
                }),
                test_ctx(),
            )
            .await
            .unwrap();
        let text = content_text(&result.content[0]);
        assert!(text.contains("Created cron job 'morning-check'"));

        // List jobs
        let result = tool
            .execute(serde_json::json!({ "action": "list" }), test_ctx())
            .await
            .unwrap();
        let text = content_text(&result.content[0]);
        assert!(text.contains("morning-check"));
        assert!(text.contains("0 9 * * *"));
    }

    #[tokio::test]
    async fn test_cron_tool_delete() {
        let db = Db::open_memory().unwrap();
        let tool = CronScheduleTool::new(db);

        // Create then delete
        tool.execute(
            serde_json::json!({
                "action": "create",
                "name": "to-remove",
                "schedule": "0 9 * * *",
                "prompt": "test"
            }),
            test_ctx(),
        )
        .await
        .unwrap();

        let result = tool
            .execute(
                serde_json::json!({ "action": "delete", "name": "to-remove" }),
                test_ctx(),
            )
            .await
            .unwrap();
        assert!(content_text(&result.content[0]).contains("Deleted"));

        // Delete nonexistent
        let result = tool
            .execute(
                serde_json::json!({ "action": "delete", "name": "to-remove" }),
                test_ctx(),
            )
            .await
            .unwrap();
        assert!(content_text(&result.content[0]).contains("No cron job"));
    }

    #[tokio::test]
    async fn test_cron_tool_toggle() {
        let db = Db::open_memory().unwrap();
        let tool = CronScheduleTool::new(db);

        tool.execute(
            serde_json::json!({
                "action": "create",
                "name": "toggler",
                "schedule": "0 9 * * *",
                "prompt": "test"
            }),
            test_ctx(),
        )
        .await
        .unwrap();

        let result = tool
            .execute(
                serde_json::json!({ "action": "toggle", "name": "toggler", "enabled": false }),
                test_ctx(),
            )
            .await
            .unwrap();
        assert!(content_text(&result.content[0]).contains("Disabled"));
    }

    /// Helper: extract text from Content.
    fn content_text(c: &Content) -> &str {
        match c {
            Content::Text { text } => text,
            _ => "",
        }
    }
}
