use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Config file not found: {0}")]
    NotFound(PathBuf),
    #[error("Environment variable not set: ${0}")]
    MissingEnvVar(String),
    #[error("Parse error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

// ---------------------------------------------------------------------------
// Top-level config
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct Config {
    pub agent: AgentConfig,
    #[serde(default)]
    pub channels: ChannelsConfig,
    #[serde(default)]
    pub persistence: PersistenceConfig,
    #[serde(default)]
    pub security: SecurityConfig,
}

// ---------------------------------------------------------------------------
// Agent
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct AgentConfig {
    /// Provider name: "anthropic", "openai", "google", etc.
    #[serde(default = "default_provider")]
    pub provider: String,
    /// Model ID passed directly to yoagent (e.g. "claude-sonnet-4-20250514")
    pub model: String,
    /// API key (supports ${ENV_VAR} expansion)
    pub api_key: String,
    /// Path to persona file, relative to config dir
    #[serde(default)]
    pub persona: Option<String>,
    /// Skill directories
    #[serde(default)]
    pub skills_dirs: Vec<String>,
    /// Max tokens per response
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Thinking level: "off", "low", "medium", "high"
    #[serde(default)]
    pub thinking: Option<String>,
    /// Budget limits
    #[serde(default)]
    pub budget: BudgetConfig,
    /// Worker configurations
    #[serde(default)]
    pub workers: WorkersConfig,
}

#[derive(Debug, Deserialize, Default)]
pub struct BudgetConfig {
    pub max_tokens_per_day: Option<u64>,
    pub max_turns_per_session: Option<usize>,
}

#[derive(Debug, Deserialize, Default)]
pub struct WorkersConfig {
    /// Default provider for workers
    pub provider: Option<String>,
    /// Default model for workers
    pub model: Option<String>,
    /// Default max_tokens for workers
    pub max_tokens: Option<u32>,

    /// Named worker overrides — populated via custom deserialization
    #[serde(flatten)]
    pub named: HashMap<String, WorkerConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WorkerConfig {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub system_prompt: Option<String>,
    pub max_tokens: Option<u32>,
    pub max_turns: Option<usize>,
}

// ---------------------------------------------------------------------------
// Channels
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct ChannelsConfig {
    pub telegram: Option<TelegramConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TelegramConfig {
    pub bot_token: String,
    #[serde(default)]
    pub allowed_senders: Vec<i64>,
    #[serde(default = "default_debounce_ms")]
    pub debounce_ms: u64,
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct PersistenceConfig {
    #[serde(default = "default_db_path")]
    pub db_path: String,
}

impl Default for PersistenceConfig {
    fn default() -> Self {
        Self {
            db_path: default_db_path(),
        }
    }
}

// ---------------------------------------------------------------------------
// Security
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct SecurityConfig {
    #[serde(default)]
    pub shell_deny_patterns: Vec<String>,
    #[serde(default)]
    pub tools: HashMap<String, ToolPermission>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ToolPermission {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub allowed_paths: Vec<String>,
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    #[serde(default)]
    pub requires_approval: bool,
}

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

fn default_provider() -> String {
    "anthropic".to_string()
}

fn default_debounce_ms() -> u64 {
    2000
}

fn default_db_path() -> String {
    "~/.yoclaw/yoclaw.db".to_string()
}

fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// Expand `~` to home directory in a path string.
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

/// Expand `${VAR_NAME}` patterns in a string using environment variables.
fn expand_env_vars(input: &str) -> Result<String, ConfigError> {
    let re = regex::Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}").unwrap();
    let mut result = input.to_string();
    // Collect captures first to avoid borrow issues
    let captures: Vec<(String, String)> = re
        .captures_iter(input)
        .map(|cap| (cap[0].to_string(), cap[1].to_string()))
        .collect();
    for (full_match, var_name) in captures {
        let value = std::env::var(&var_name)
            .map_err(|_| ConfigError::MissingEnvVar(var_name.clone()))?;
        result = result.replace(&full_match, &value);
    }
    Ok(result)
}

/// Default config directory: ~/.yoclaw/
pub fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".yoclaw")
}

/// Load config from `~/.yoclaw/config.toml` (or a custom path).
pub fn load_config(path: Option<&Path>) -> Result<Config, ConfigError> {
    let config_path = match path {
        Some(p) => p.to_path_buf(),
        None => config_dir().join("config.toml"),
    };

    if !config_path.exists() {
        return Err(ConfigError::NotFound(config_path));
    }

    let raw = std::fs::read_to_string(&config_path)?;
    parse_config(&raw)
}

/// Parse a config string (after reading from file).
pub fn parse_config(raw: &str) -> Result<Config, ConfigError> {
    let expanded = expand_env_vars(raw)?;
    let config: Config = toml::from_str(&expanded)?;
    Ok(config)
}

impl Config {
    /// Resolve the persona file path.
    pub fn persona_path(&self) -> PathBuf {
        match &self.agent.persona {
            Some(p) => {
                let path = expand_tilde(p);
                if path.is_absolute() {
                    path
                } else {
                    config_dir().join(p)
                }
            }
            None => config_dir().join("persona.md"),
        }
    }

    /// Resolve skills directories.
    pub fn skills_dirs(&self) -> Vec<PathBuf> {
        if self.agent.skills_dirs.is_empty() {
            vec![config_dir().join("skills")]
        } else {
            self.agent.skills_dirs.iter().map(|s| expand_tilde(s)).collect()
        }
    }

    /// Resolve the database path.
    pub fn db_path(&self) -> PathBuf {
        expand_tilde(&self.persistence.db_path)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_config() {
        let toml = r#"
[agent]
model = "claude-sonnet-4-20250514"
api_key = "sk-test-key"

[channels.telegram]
bot_token = "123:ABC"
"#;
        let config = parse_config(toml).unwrap();
        assert_eq!(config.agent.model, "claude-sonnet-4-20250514");
        assert_eq!(config.agent.api_key, "sk-test-key");
        assert_eq!(config.agent.provider, "anthropic");
        assert!(config.channels.telegram.is_some());
        let tg = config.channels.telegram.unwrap();
        assert_eq!(tg.bot_token, "123:ABC");
        assert_eq!(tg.debounce_ms, 2000);
        assert!(tg.allowed_senders.is_empty());
    }

    #[test]
    fn test_parse_full_config() {
        let toml = r#"
[agent]
provider = "openai"
model = "gpt-4o"
api_key = "sk-test"
persona = "my-persona.md"
max_tokens = 4096
thinking = "medium"

[agent.budget]
max_tokens_per_day = 500000
max_turns_per_session = 20

[channels.telegram]
bot_token = "123:ABC"
allowed_senders = [111, 222]
debounce_ms = 3000

[persistence]
db_path = "/tmp/test.db"

[security]
shell_deny_patterns = ["rm -rf", "sudo"]

[security.tools.shell]
enabled = true
requires_approval = true

[security.tools.http]
enabled = true
allowed_hosts = ["api.example.com"]

[security.tools.read_file]
enabled = true
allowed_paths = ["/tmp/"]
"#;
        let config = parse_config(toml).unwrap();
        assert_eq!(config.agent.provider, "openai");
        assert_eq!(config.agent.model, "gpt-4o");
        assert_eq!(config.agent.max_tokens, Some(4096));
        assert_eq!(config.agent.thinking.as_deref(), Some("medium"));
        assert_eq!(config.agent.budget.max_tokens_per_day, Some(500000));
        assert_eq!(config.agent.budget.max_turns_per_session, Some(20));

        let tg = config.channels.telegram.unwrap();
        assert_eq!(tg.allowed_senders, vec![111, 222]);
        assert_eq!(tg.debounce_ms, 3000);

        assert_eq!(config.persistence.db_path, "/tmp/test.db");
        assert_eq!(config.security.shell_deny_patterns, vec!["rm -rf", "sudo"]);

        let shell = config.security.tools.get("shell").unwrap();
        assert!(shell.enabled);
        assert!(shell.requires_approval);

        let http = config.security.tools.get("http").unwrap();
        assert_eq!(http.allowed_hosts, vec!["api.example.com"]);
    }

    #[test]
    fn test_env_var_expansion() {
        std::env::set_var("YOCLAW_TEST_KEY", "expanded-value");
        let toml = r#"
[agent]
model = "test"
api_key = "${YOCLAW_TEST_KEY}"
"#;
        let config = parse_config(toml).unwrap();
        assert_eq!(config.agent.api_key, "expanded-value");
        std::env::remove_var("YOCLAW_TEST_KEY");
    }

    #[test]
    fn test_missing_env_var() {
        let toml = r#"
[agent]
model = "test"
api_key = "${YOCLAW_NONEXISTENT_VAR}"
"#;
        let err = parse_config(toml).unwrap_err();
        assert!(matches!(err, ConfigError::MissingEnvVar(ref v) if v == "YOCLAW_NONEXISTENT_VAR"));
    }

    #[test]
    fn test_expand_tilde() {
        let path = expand_tilde("~/.yoclaw/config.toml");
        assert!(path.to_str().unwrap().contains(".yoclaw/config.toml"));
        assert!(!path.to_str().unwrap().starts_with("~"));

        let abs = expand_tilde("/absolute/path");
        assert_eq!(abs, PathBuf::from("/absolute/path"));
    }

    #[test]
    fn test_defaults() {
        let toml = r#"
[agent]
model = "test"
api_key = "key"
"#;
        let config = parse_config(toml).unwrap();
        assert_eq!(config.agent.provider, "anthropic");
        assert!(config.agent.budget.max_tokens_per_day.is_none());
        assert!(config.channels.telegram.is_none());
        assert_eq!(config.persistence.db_path, "~/.yoclaw/yoclaw.db");
    }
}
