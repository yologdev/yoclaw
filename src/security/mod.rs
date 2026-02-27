pub mod budget;
pub mod heuristics;
pub mod injection;
pub mod llm_judge;

use crate::config::SecurityConfig;
use crate::db::Db;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum SecurityDenied {
    #[error("Tool '{tool}' is disabled")]
    ToolDisabled { tool: String },
    #[error("Command blocked by deny pattern: {pattern}")]
    CommandBlocked { pattern: String },
    #[error("Path '{path}' not in allowed paths for tool '{tool}'")]
    PathNotAllowed { tool: String, path: String },
    #[error("Host '{host}' not in allowed hosts for tool '{tool}'")]
    HostNotAllowed { tool: String, host: String },
}

/// Security policy derived from config.
#[derive(Debug, Clone)]
pub struct SecurityPolicy {
    pub shell_deny_patterns: Vec<String>,
    pub tool_permissions: HashMap<String, ToolPerm>,
}

#[derive(Debug, Clone)]
pub struct ToolPerm {
    pub enabled: bool,
    pub allowed_paths: Vec<String>,
    pub allowed_hosts: Vec<String>,
    pub requires_approval: bool,
}

impl SecurityPolicy {
    pub fn from_config(config: &SecurityConfig) -> Self {
        let tool_permissions = config
            .tools
            .iter()
            .map(|(name, perm)| {
                (
                    name.clone(),
                    ToolPerm {
                        enabled: perm.enabled,
                        allowed_paths: perm.allowed_paths.clone(),
                        allowed_hosts: perm.allowed_hosts.clone(),
                        requires_approval: perm.requires_approval,
                    },
                )
            })
            .collect();
        Self {
            shell_deny_patterns: config.shell_deny_patterns.clone(),
            tool_permissions,
        }
    }

    /// Check if a tool call is allowed.
    pub fn check_tool_call(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> Result<(), SecurityDenied> {
        // Map yoagent tool names to our security config names
        let config_name = match tool_name {
            "bash" => "shell",
            "read_file" => "read_file",
            "write_file" => "write_file",
            "edit_file" => "write_file", // edit shares write_file permissions
            "list_files" => "read_file",
            "search" => "read_file",
            _ => tool_name,
        };

        if let Some(perm) = self.tool_permissions.get(config_name) {
            if !perm.enabled {
                return Err(SecurityDenied::ToolDisabled {
                    tool: tool_name.to_string(),
                });
            }

            // Check shell deny patterns for bash tool
            if tool_name == "bash" {
                if let Some(command) = args.get("command").and_then(|v| v.as_str()) {
                    for pattern in &self.shell_deny_patterns {
                        if command.contains(pattern) {
                            return Err(SecurityDenied::CommandBlocked {
                                pattern: pattern.clone(),
                            });
                        }
                    }
                }
            }

            // Check path allowlists for file tools
            if matches!(
                tool_name,
                "read_file" | "write_file" | "edit_file" | "list_files" | "search"
            ) && !perm.allowed_paths.is_empty()
            {
                let file_path = args
                    .get("file_path")
                    .or_else(|| args.get("path"))
                    .and_then(|v| v.as_str());
                if let Some(path) = file_path {
                    let path_expanded = crate::config::expand_tilde(path);
                    let allowed = perm.allowed_paths.iter().any(|allowed| {
                        let allowed_expanded = crate::config::expand_tilde(allowed);
                        path_expanded.starts_with(&allowed_expanded)
                    });
                    if !allowed {
                        return Err(SecurityDenied::PathNotAllowed {
                            tool: tool_name.to_string(),
                            path: path.to_string(),
                        });
                    }
                }
            }

            // Check host allowlists for http tool
            if tool_name == "http" && !perm.allowed_hosts.is_empty() {
                if let Some(url) = args.get("url").and_then(|v| v.as_str()) {
                    let allowed = perm.allowed_hosts.iter().any(|host| url.contains(host));
                    if !allowed {
                        return Err(SecurityDenied::HostNotAllowed {
                            tool: tool_name.to_string(),
                            host: url.to_string(),
                        });
                    }
                }
            }
        }

        Ok(())
    }
}

/// Wraps an AgentTool with security policy checks.
pub struct SecureToolWrapper {
    pub inner: Box<dyn yoagent::AgentTool>,
    pub policy: Arc<std::sync::RwLock<SecurityPolicy>>,
    pub db: Db,
    pub session_id: Arc<std::sync::RwLock<String>>,
}

#[async_trait::async_trait]
impl yoagent::AgentTool for SecureToolWrapper {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn label(&self) -> &str {
        self.inner.label()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.inner.parameters_schema()
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: yoagent::types::ToolContext,
    ) -> Result<yoagent::ToolResult, yoagent::ToolError> {
        // Check security policy (scoped to drop read guard before await)
        let denied = {
            let policy = self.policy.read().unwrap();
            policy.check_tool_call(self.inner.name(), &params).err()
        };
        if let Some(denied) = denied {
            let session = self.session_id.read().unwrap().clone();
            let _ = self
                .db
                .audit_log(
                    Some(&session),
                    "denied",
                    Some(self.inner.name()),
                    Some(&denied.to_string()),
                    0,
                )
                .await;
            return Err(yoagent::ToolError::Failed(format!(
                "Security policy: {}",
                denied
            )));
        }

        // Log the tool call
        let session = self.session_id.read().unwrap().clone();
        let args_str = serde_json::to_string(&params).unwrap_or_default();
        let _ = self
            .db
            .audit_log(
                Some(&session),
                "tool_call",
                Some(self.inner.name()),
                Some(&args_str),
                0,
            )
            .await;

        // Execute the actual tool
        self.inner.execute(params, ctx).await
    }
}

/// Wrap a list of tools with security policy enforcement.
pub fn wrap_tools(
    tools: Vec<Box<dyn yoagent::AgentTool>>,
    policy: Arc<std::sync::RwLock<SecurityPolicy>>,
    db: Db,
    session_id: Arc<std::sync::RwLock<String>>,
) -> Vec<Box<dyn yoagent::AgentTool>> {
    tools
        .into_iter()
        .map(|tool| {
            Box::new(SecureToolWrapper {
                inner: tool,
                policy: policy.clone(),
                db: db.clone(),
                session_id: session_id.clone(),
            }) as Box<dyn yoagent::AgentTool>
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_policy() -> SecurityPolicy {
        SecurityPolicy {
            shell_deny_patterns: vec!["rm -rf".to_string(), "sudo".to_string()],
            tool_permissions: HashMap::from([
                (
                    "shell".to_string(),
                    ToolPerm {
                        enabled: true,
                        allowed_paths: vec![],
                        allowed_hosts: vec![],
                        requires_approval: false,
                    },
                ),
                (
                    "read_file".to_string(),
                    ToolPerm {
                        enabled: true,
                        allowed_paths: vec!["/tmp/".to_string()],
                        allowed_hosts: vec![],
                        requires_approval: false,
                    },
                ),
                (
                    "write_file".to_string(),
                    ToolPerm {
                        enabled: false,
                        allowed_paths: vec![],
                        allowed_hosts: vec![],
                        requires_approval: false,
                    },
                ),
            ]),
        }
    }

    #[test]
    fn test_allow_safe_command() {
        let policy = test_policy();
        let result = policy.check_tool_call("bash", &json!({"command": "ls -la"}));
        assert!(result.is_ok());
    }

    #[test]
    fn test_deny_dangerous_command() {
        let policy = test_policy();
        let result = policy.check_tool_call("bash", &json!({"command": "rm -rf /"}));
        assert!(matches!(result, Err(SecurityDenied::CommandBlocked { .. })));
    }

    #[test]
    fn test_deny_sudo() {
        let policy = test_policy();
        let result = policy.check_tool_call("bash", &json!({"command": "sudo apt install foo"}));
        assert!(matches!(result, Err(SecurityDenied::CommandBlocked { .. })));
    }

    #[test]
    fn test_disabled_tool() {
        let policy = test_policy();
        let result = policy.check_tool_call("write_file", &json!({"file_path": "/tmp/test.txt"}));
        assert!(matches!(result, Err(SecurityDenied::ToolDisabled { .. })));
    }

    #[test]
    fn test_path_allowed() {
        let policy = test_policy();
        let result = policy.check_tool_call("read_file", &json!({"file_path": "/tmp/test.txt"}));
        assert!(result.is_ok());
    }

    #[test]
    fn test_path_denied() {
        let policy = test_policy();
        let result = policy.check_tool_call("read_file", &json!({"file_path": "/etc/passwd"}));
        assert!(matches!(result, Err(SecurityDenied::PathNotAllowed { .. })));
    }

    #[test]
    fn test_unknown_tool_allowed() {
        let policy = test_policy();
        // Tools not in the policy are allowed by default
        let result = policy.check_tool_call("memory_search", &json!({"query": "test"}));
        assert!(result.is_ok());
    }
}
