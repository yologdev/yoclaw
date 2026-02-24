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
    #[serde(default)]
    pub web: WebConfig,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
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
    /// Context window management
    #[serde(default)]
    pub context: ContextConfig,
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
    pub discord: Option<DiscordConfig>,
    pub slack: Option<SlackConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TelegramConfig {
    pub bot_token: String,
    #[serde(default)]
    pub allowed_senders: Vec<i64>,
    #[serde(default = "default_debounce_ms")]
    pub debounce_ms: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DiscordConfig {
    pub bot_token: String,
    #[serde(default)]
    pub allowed_guilds: Vec<u64>,
    #[serde(default)]
    pub allowed_users: Vec<u64>,
    #[serde(default = "default_debounce_ms")]
    pub debounce_ms: u64,
    /// Channel name → worker routing rules
    #[serde(default)]
    pub routing: HashMap<String, ChannelRoute>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ChannelRoute {
    pub worker: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SlackConfig {
    /// Bot token (xoxb-...)
    pub bot_token: String,
    /// App-level token for Socket Mode (xapp-...)
    pub app_token: String,
    #[serde(default)]
    pub allowed_channels: Vec<String>,
    #[serde(default)]
    pub allowed_users: Vec<String>,
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
// Context
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct ContextConfig {
    pub max_context_tokens: Option<u64>,
    pub keep_recent: Option<usize>,
    pub tool_output_max_lines: Option<usize>,
}

// ---------------------------------------------------------------------------
// Web UI
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct WebConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_web_port")]
    pub port: u16,
    #[serde(default = "default_web_bind")]
    pub bind: String,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: default_web_port(),
            bind: default_web_bind(),
        }
    }
}

// ---------------------------------------------------------------------------
// Scheduler
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SchedulerConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_tick_interval")]
    pub tick_interval_secs: u64,
    #[serde(default)]
    pub cortex: CortexConfig,
    #[serde(default)]
    pub cron: CronConfig,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            tick_interval_secs: default_tick_interval(),
            cortex: CortexConfig::default(),
            cron: CronConfig::default(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CortexConfig {
    #[serde(default = "default_cortex_interval")]
    pub interval_hours: u64,
    #[serde(default = "default_cortex_model")]
    pub model: String,
}

impl Default for CortexConfig {
    fn default() -> Self {
        Self {
            interval_hours: default_cortex_interval(),
            model: default_cortex_model(),
        }
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct CronConfig {
    #[serde(default)]
    pub jobs: Vec<CronJobConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CronJobConfig {
    pub name: String,
    pub schedule: String,
    pub prompt: String,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default = "default_session_mode")]
    pub session: String,
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

fn default_web_port() -> u16 {
    19898
}

fn default_web_bind() -> String {
    "127.0.0.1".to_string()
}

fn default_tick_interval() -> u64 {
    60
}

fn default_cortex_interval() -> u64 {
    6
}

fn default_cortex_model() -> String {
    "claude-haiku-4-5-20251001".to_string()
}

fn default_session_mode() -> String {
    "isolated".to_string()
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
        let value =
            std::env::var(&var_name).map_err(|_| ConfigError::MissingEnvVar(var_name.clone()))?;
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
            self.agent
                .skills_dirs
                .iter()
                .map(|s| expand_tilde(s))
                .collect()
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

    #[test]
    fn test_parse_discord_config() {
        let toml = r#"
[agent]
model = "test"
api_key = "key"

[channels.discord]
bot_token = "discord-token-123"
allowed_guilds = [111, 222]
allowed_users = [333]
debounce_ms = 1000

[channels.discord.routing.coding-help]
worker = "coding"

[channels.discord.routing.research]
worker = "research"
"#;
        let config = parse_config(toml).unwrap();
        let dc = config.channels.discord.unwrap();
        assert_eq!(dc.bot_token, "discord-token-123");
        assert_eq!(dc.allowed_guilds, vec![111, 222]);
        assert_eq!(dc.allowed_users, vec![333]);
        assert_eq!(dc.debounce_ms, 1000);
        assert_eq!(dc.routing.len(), 2);
        assert_eq!(dc.routing["coding-help"].worker, "coding");
        assert_eq!(dc.routing["research"].worker, "research");
    }

    #[test]
    fn test_parse_slack_config() {
        let toml = r#"
[agent]
model = "test"
api_key = "key"

[channels.slack]
bot_token = "xoxb-test"
app_token = "xapp-test"
allowed_channels = ["general", "random"]
allowed_users = ["U123"]
debounce_ms = 1500
"#;
        let config = parse_config(toml).unwrap();
        let sl = config.channels.slack.unwrap();
        assert_eq!(sl.bot_token, "xoxb-test");
        assert_eq!(sl.app_token, "xapp-test");
        assert_eq!(sl.allowed_channels, vec!["general", "random"]);
        assert_eq!(sl.allowed_users, vec!["U123"]);
        assert_eq!(sl.debounce_ms, 1500);
    }

    #[test]
    fn test_parse_web_config() {
        let toml = r#"
[agent]
model = "test"
api_key = "key"

[web]
enabled = true
port = 8080
bind = "0.0.0.0"
"#;
        let config = parse_config(toml).unwrap();
        assert!(config.web.enabled);
        assert_eq!(config.web.port, 8080);
        assert_eq!(config.web.bind, "0.0.0.0");
    }

    #[test]
    fn test_web_config_defaults() {
        let toml = r#"
[agent]
model = "test"
api_key = "key"
"#;
        let config = parse_config(toml).unwrap();
        assert!(!config.web.enabled);
        assert_eq!(config.web.port, 19898);
        assert_eq!(config.web.bind, "127.0.0.1");
    }

    #[test]
    fn test_parse_context_config() {
        let toml = r#"
[agent]
model = "test"
api_key = "key"

[agent.context]
max_context_tokens = 180000
keep_recent = 4
tool_output_max_lines = 50
"#;
        let config = parse_config(toml).unwrap();
        assert_eq!(config.agent.context.max_context_tokens, Some(180000));
        assert_eq!(config.agent.context.keep_recent, Some(4));
        assert_eq!(config.agent.context.tool_output_max_lines, Some(50));
    }

    #[test]
    fn test_parse_scheduler_config() {
        let toml = r#"
[agent]
model = "test"
api_key = "key"

[scheduler]
enabled = true
tick_interval_secs = 30

[scheduler.cortex]
interval_hours = 12
model = "claude-haiku-4-5-20251001"

[[scheduler.cron.jobs]]
name = "morning-briefing"
schedule = "0 9 * * *"
prompt = "Check my calendar"
target = "telegram"
session = "isolated"

[[scheduler.cron.jobs]]
name = "evening-summary"
schedule = "0 18 * * 1-5"
prompt = "Summarize the day"
target = "telegram"
"#;
        let config = parse_config(toml).unwrap();
        assert!(config.scheduler.enabled);
        assert_eq!(config.scheduler.tick_interval_secs, 30);
        assert_eq!(config.scheduler.cortex.interval_hours, 12);
        assert_eq!(config.scheduler.cron.jobs.len(), 2);

        let job1 = &config.scheduler.cron.jobs[0];
        assert_eq!(job1.name, "morning-briefing");
        assert_eq!(job1.schedule, "0 9 * * *");
        assert_eq!(job1.prompt, "Check my calendar");
        assert_eq!(job1.target.as_deref(), Some("telegram"));
        assert_eq!(job1.session, "isolated");

        let job2 = &config.scheduler.cron.jobs[1];
        assert_eq!(job2.name, "evening-summary");
        assert_eq!(job2.session, "isolated"); // default
    }
}
