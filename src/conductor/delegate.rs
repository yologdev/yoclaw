use crate::config::Config;
use std::sync::Arc;
use yoagent::provider::StreamProvider;
use yoagent::sub_agent::SubAgentTool;
use yoagent::types::AgentTool;

/// Summary of a configured worker (for inspect output).
#[derive(Debug, Clone)]
pub struct WorkerInfo {
    pub name: String,
    pub provider: String,
    pub model: String,
    pub max_turns: usize,
    pub system_prompt: Option<String>,
}

/// Build SubAgentTools from the `[agent.workers.*]` config sections.
///
/// Returns a list of (SubAgentTool, WorkerInfo) pairs. The SubAgentTool should
/// be registered on the Agent via `agent.with_sub_agent(sub)`. Each worker gets
/// the specified tools (or a default set).
pub fn build_workers(
    config: &Config,
    tools: &[Arc<dyn AgentTool>],
) -> Vec<(SubAgentTool, WorkerInfo)> {
    let workers_config = &config.agent.workers;
    let mut result = Vec::new();

    // Default values from [agent.workers]
    let default_provider = workers_config
        .provider
        .as_deref()
        .unwrap_or(&config.agent.provider);
    let default_model = workers_config
        .model
        .as_deref()
        .unwrap_or(&config.agent.model);
    let default_max_tokens = workers_config.max_tokens.or(config.agent.max_tokens);

    for (name, worker) in &workers_config.named {
        let provider_name = worker
            .provider
            .as_deref()
            .unwrap_or(default_provider);
        let model = worker.model.as_deref().unwrap_or(default_model);
        let api_key = worker
            .api_key
            .as_deref()
            .unwrap_or(&config.agent.api_key);
        let max_turns = worker.max_turns.unwrap_or(10);

        let provider = resolve_arc_provider(provider_name);

        let description = match &worker.system_prompt {
            Some(prompt) => {
                let snippet = if prompt.len() > 100 {
                    format!("{}...", &prompt[..100])
                } else {
                    prompt.clone()
                };
                format!(
                    "Delegate a task to the '{}' worker ({}). {}",
                    name, model, snippet
                )
            }
            None => format!("Delegate a task to the '{}' worker ({})", name, model),
        };

        let mut sub = SubAgentTool::new(name, provider)
            .with_description(description)
            .with_model(model)
            .with_api_key(api_key)
            .with_max_turns(max_turns)
            .with_tools(tools.to_vec());

        if let Some(ref prompt) = worker.system_prompt {
            sub = sub.with_system_prompt(prompt);
        }

        if let Some(max_tokens) = worker.max_tokens.or(default_max_tokens) {
            sub = sub.with_max_tokens(max_tokens);
        }

        let info = WorkerInfo {
            name: name.clone(),
            provider: provider_name.to_string(),
            model: model.to_string(),
            max_turns,
            system_prompt: worker.system_prompt.clone(),
        };

        result.push((sub, info));
    }

    // Sort by name for deterministic order
    result.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    result
}

/// Resolve a provider name to an Arc<dyn StreamProvider>.
fn resolve_arc_provider(name: &str) -> Arc<dyn StreamProvider> {
    use yoagent::provider::*;
    match name {
        "anthropic" => Arc::new(AnthropicProvider),
        "openai" => Arc::new(OpenAiCompatProvider),
        "google" => Arc::new(GoogleProvider),
        other => {
            tracing::warn!(
                "Unknown provider '{}' for worker, defaulting to anthropic",
                other
            );
            Arc::new(AnthropicProvider)
        }
    }
}

/// Format worker info for display (inspect command).
pub fn format_workers_info(workers: &[WorkerInfo]) -> String {
    if workers.is_empty() {
        return "No workers configured.".to_string();
    }

    workers
        .iter()
        .map(|w| {
            let prompt_hint = w
                .system_prompt
                .as_ref()
                .map(|p| {
                    let snippet = if p.len() > 50 {
                        format!("{}...", &p[..50])
                    } else {
                        p.clone()
                    };
                    format!(" \"{}\"", snippet)
                })
                .unwrap_or_default();
            format!(
                "  {} — {} / {} (max_turns: {}{})",
                w.name, w.provider, w.model, w.max_turns, prompt_hint
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_config;

    #[test]
    fn test_build_workers_from_config() {
        let toml = r#"
[agent]
model = "claude-sonnet-4-20250514"
api_key = "sk-test"

[agent.workers]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"

[agent.workers.coding]
model = "claude-sonnet-4-20250514"
system_prompt = "You are a coding assistant."
max_turns = 20

[agent.workers.research]
max_turns = 15
"#;
        let config = parse_config(toml).unwrap();
        let tools: Vec<Arc<dyn AgentTool>> = Vec::new();
        let workers = build_workers(&config, &tools);

        assert_eq!(workers.len(), 2);

        // Sorted by name: coding, research
        assert_eq!(workers[0].1.name, "coding");
        assert_eq!(workers[0].1.model, "claude-sonnet-4-20250514");
        assert_eq!(workers[0].1.max_turns, 20);
        assert!(workers[0].1.system_prompt.is_some());

        assert_eq!(workers[1].1.name, "research");
        // Falls back to workers default model
        assert_eq!(workers[1].1.model, "claude-haiku-4-5-20251001");
        assert_eq!(workers[1].1.max_turns, 15);
    }

    #[test]
    fn test_build_workers_empty() {
        let toml = r#"
[agent]
model = "test"
api_key = "key"
"#;
        let config = parse_config(toml).unwrap();
        let tools: Vec<Arc<dyn AgentTool>> = Vec::new();
        let workers = build_workers(&config, &tools);
        assert!(workers.is_empty());
    }

    #[test]
    fn test_format_workers_info() {
        let workers = vec![
            WorkerInfo {
                name: "coding".into(),
                provider: "anthropic".into(),
                model: "claude-sonnet-4-20250514".into(),
                max_turns: 20,
                system_prompt: Some("You are a coding assistant.".into()),
            },
            WorkerInfo {
                name: "research".into(),
                provider: "anthropic".into(),
                model: "claude-haiku-4-5-20251001".into(),
                max_turns: 15,
                system_prompt: None,
            },
        ];
        let info = format_workers_info(&workers);
        assert!(info.contains("coding"));
        assert!(info.contains("research"));
        assert!(info.contains("max_turns: 20"));
    }
}
