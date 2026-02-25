use crate::channels::coalesce::SharedDebounce;
use crate::conductor::Conductor;
use crate::config::{self, Config};
use crate::security::SecurityPolicy;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// Watches the config file for changes and applies hot-reloadable settings.
pub struct ConfigWatcher {
    config_path: PathBuf,
    last_mtime: Option<SystemTime>,
    last_hash: u64,
}

impl ConfigWatcher {
    pub fn new(config_path: PathBuf) -> Self {
        let (mtime, hash) = Self::read_file_meta(&config_path);
        Self {
            config_path,
            last_mtime: mtime,
            last_hash: hash,
        }
    }

    fn read_file_meta(path: &PathBuf) -> (Option<SystemTime>, u64) {
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let hash = std::fs::read_to_string(path)
            .map(|content| {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                content.hash(&mut hasher);
                hasher.finish()
            })
            .unwrap_or(0);
        (mtime, hash)
    }

    /// Check if the config file has changed. Returns `Some(Config)` if it changed
    /// and parsed successfully, `None` if unchanged or on parse error.
    pub fn check(&mut self) -> Option<Config> {
        // Stage 1: cheap mtime check
        let new_mtime = std::fs::metadata(&self.config_path)
            .and_then(|m| m.modified())
            .ok();
        if new_mtime == self.last_mtime {
            return None;
        }
        self.last_mtime = new_mtime;

        // Stage 2: content hash check (catches `touch` without edit)
        let content = match std::fs::read_to_string(&self.config_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to read config file: {}", e);
                return None;
            }
        };
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        content.hash(&mut hasher);
        let new_hash = hasher.finish();

        if new_hash == self.last_hash {
            return None;
        }
        self.last_hash = new_hash;

        // Stage 3: parse new config
        match config::parse_config(&content) {
            Ok(config) => {
                tracing::info!("Config file changed, reloading...");
                Some(config)
            }
            Err(e) => {
                tracing::warn!("Config file changed but failed to parse: {}", e);
                None
            }
        }
    }
}

/// Describes which config sections changed between old and new configs.
pub struct ConfigDiff {
    pub budget_changed: bool,
    pub security_changed: bool,
    pub debounce_changed: bool,
    pub restart_required: Vec<&'static str>,
}

/// Compare two configs and return a diff of what changed.
pub fn diff_configs(old: &Config, new: &Config) -> ConfigDiff {
    let mut restart_required = Vec::new();

    // Check restart-required fields
    if old.agent.provider != new.agent.provider
        || old.agent.model != new.agent.model
        || old.agent.api_key != new.agent.api_key
    {
        restart_required.push("agent provider/model/api_key");
    }
    if old.agent.max_tokens != new.agent.max_tokens {
        restart_required.push("agent.max_tokens");
    }
    if old.agent.thinking != new.agent.thinking {
        restart_required.push("agent.thinking");
    }
    if old.persistence != new.persistence {
        restart_required.push("persistence.db_path");
    }
    if old.web != new.web {
        restart_required.push("web.*");
    }
    // Channel tokens require reconnection
    if old.channels.telegram.as_ref().map(|t| &t.bot_token)
        != new.channels.telegram.as_ref().map(|t| &t.bot_token)
    {
        restart_required.push("channels.telegram.bot_token");
    }
    if old.channels.discord.as_ref().map(|d| &d.bot_token)
        != new.channels.discord.as_ref().map(|d| &d.bot_token)
    {
        restart_required.push("channels.discord.bot_token");
    }
    // Injection detector is baked into Agent at startup — cannot hot-reload
    if old.security.injection != new.security.injection {
        restart_required.push("security.injection");
    }

    ConfigDiff {
        budget_changed: old.agent.budget != new.agent.budget,
        security_changed: old.security != new.security,
        debounce_changed: debounce_changed(old, new),
        restart_required,
    }
}

fn debounce_changed(old: &Config, new: &Config) -> bool {
    old.channels.telegram.as_ref().map(|t| t.debounce_ms)
        != new.channels.telegram.as_ref().map(|t| t.debounce_ms)
        || old.channels.discord.as_ref().map(|d| d.debounce_ms)
            != new.channels.discord.as_ref().map(|d| d.debounce_ms)
        || old.channels.slack.as_ref().map(|s| s.debounce_ms)
            != new.channels.slack.as_ref().map(|s| s.debounce_ms)
}

/// Apply hot-reloadable config changes to the running system.
pub fn apply_hot_reload(
    diff: &ConfigDiff,
    new_config: &Config,
    conductor: &mut Conductor,
    shared_debounce: &SharedDebounce,
) {
    if diff.budget_changed {
        conductor.update_budget(
            new_config.agent.budget.max_tokens_per_day,
            new_config.agent.budget.max_turns_per_session,
        );
    }

    if diff.security_changed {
        let new_policy = SecurityPolicy::from_config(&new_config.security);
        conductor.update_security(new_policy);
    }

    if diff.debounce_changed {
        let mut debounce = shared_debounce.write().unwrap();
        debounce.per_channel.clear();
        if let Some(ref tg) = new_config.channels.telegram {
            debounce
                .per_channel
                .insert("telegram".into(), Duration::from_millis(tg.debounce_ms));
        }
        if let Some(ref dc) = new_config.channels.discord {
            debounce
                .per_channel
                .insert("discord".into(), Duration::from_millis(dc.debounce_ms));
        }
        if let Some(ref sl) = new_config.channels.slack {
            debounce
                .per_channel
                .insert("slack".into(), Duration::from_millis(sl.debounce_ms));
        }
        tracing::info!("Debounce timings reloaded");
    }

    // Always update group catchup (cheap no-op if unchanged)
    conductor.update_max_group_catchup(new_config.agent.context.max_group_catchup_messages);

    for field in &diff.restart_required {
        tracing::warn!("Config change requires restart: {}", field);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_watcher_detects_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[agent]
model = "test"
api_key = "key"
"#,
        )
        .unwrap();

        let mut watcher = ConfigWatcher::new(path.clone());
        // First check — no change (same as initial read)
        assert!(watcher.check().is_none());

        // Modify the file
        std::thread::sleep(Duration::from_millis(50));
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        write!(
            f,
            r#"
[agent]
model = "test"
api_key = "new-key"
"#
        )
        .unwrap();

        let config = watcher.check();
        assert!(config.is_some());
        assert_eq!(config.unwrap().agent.api_key, "new-key");
    }

    #[test]
    fn test_watcher_ignores_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[agent]
model = "test"
api_key = "key"
"#,
        )
        .unwrap();

        let mut watcher = ConfigWatcher::new(path);
        assert!(watcher.check().is_none());
        assert!(watcher.check().is_none());
    }

    #[test]
    fn test_watcher_handles_invalid_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[agent]
model = "test"
api_key = "key"
"#,
        )
        .unwrap();

        let mut watcher = ConfigWatcher::new(path.clone());

        // Write invalid TOML
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(&path, "this is not valid toml {{{}}}").unwrap();
        // Should return None (parse error)
        assert!(watcher.check().is_none());
    }

    #[test]
    fn test_diff_budget_changed() {
        let old = config::parse_config(
            r#"
[agent]
model = "test"
api_key = "key"
[agent.budget]
max_tokens_per_day = 100000
"#,
        )
        .unwrap();

        let new = config::parse_config(
            r#"
[agent]
model = "test"
api_key = "key"
[agent.budget]
max_tokens_per_day = 200000
"#,
        )
        .unwrap();

        let diff = diff_configs(&old, &new);
        assert!(diff.budget_changed);
        assert!(!diff.security_changed);
        assert!(diff.restart_required.is_empty());
    }

    #[test]
    fn test_diff_security_changed() {
        let old = config::parse_config(
            r#"
[agent]
model = "test"
api_key = "key"
[security]
shell_deny_patterns = ["rm -rf"]
"#,
        )
        .unwrap();

        let new = config::parse_config(
            r#"
[agent]
model = "test"
api_key = "key"
[security]
shell_deny_patterns = ["rm -rf", "sudo"]
"#,
        )
        .unwrap();

        let diff = diff_configs(&old, &new);
        assert!(!diff.budget_changed);
        assert!(diff.security_changed);
    }

    #[test]
    fn test_diff_restart_required() {
        let old = config::parse_config(
            r#"
[agent]
model = "test-model-a"
api_key = "key"
"#,
        )
        .unwrap();

        let new = config::parse_config(
            r#"
[agent]
model = "test-model-b"
api_key = "key"
"#,
        )
        .unwrap();

        let diff = diff_configs(&old, &new);
        assert!(!diff.restart_required.is_empty());
    }

    #[test]
    fn test_diff_no_changes() {
        let cfg = r#"
[agent]
model = "test"
api_key = "key"
[agent.budget]
max_tokens_per_day = 100000
"#;
        let old = config::parse_config(cfg).unwrap();
        let new = config::parse_config(cfg).unwrap();

        let diff = diff_configs(&old, &new);
        assert!(!diff.budget_changed);
        assert!(!diff.security_changed);
        assert!(!diff.debounce_changed);
        assert!(diff.restart_required.is_empty());
    }

    #[test]
    fn test_diff_injection_requires_restart() {
        let old = config::parse_config(
            r#"
[agent]
model = "test"
api_key = "key"
[security.injection]
enabled = false
"#,
        )
        .unwrap();

        let new = config::parse_config(
            r#"
[agent]
model = "test"
api_key = "key"
[security.injection]
enabled = true
action = "block"
"#,
        )
        .unwrap();

        let diff = diff_configs(&old, &new);
        assert!(diff.security_changed);
        assert!(
            diff.restart_required.contains(&"security.injection"),
            "Injection config changes should require restart"
        );
    }
}
