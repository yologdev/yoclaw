//! Migration from OpenClaw data directory to yoclaw format.
//!
//! Conversions:
//! - SOUL.md / IDENTITY.md → ~/.yoclaw/persona.md
//! - skills/ directory → ~/.yoclaw/skills/
//! - MEMORY.md or memories/ → import into SQLite memory table
//! - Config files → generate config.toml template

use crate::config::config_dir;
use std::path::Path;

/// Run the migration from an OpenClaw directory.
pub fn run_migrate(openclaw_dir: &Path) -> anyhow::Result<()> {
    if !openclaw_dir.exists() {
        anyhow::bail!("OpenClaw directory not found: {}", openclaw_dir.display());
    }

    let target_dir = config_dir();
    std::fs::create_dir_all(&target_dir)?;
    std::fs::create_dir_all(target_dir.join("skills"))?;

    println!(
        "Migrating from {} → {}",
        openclaw_dir.display(),
        target_dir.display()
    );

    // 1. Persona: SOUL.md or IDENTITY.md → persona.md
    let persona_target = target_dir.join("persona.md");
    let persona_migrated = migrate_persona(openclaw_dir, &persona_target)?;
    if persona_migrated {
        println!("  Persona → {}", persona_target.display());
    }

    // 2. Skills: skills/ → ~/.yoclaw/skills/
    let skills_migrated = migrate_skills(openclaw_dir, &target_dir.join("skills"))?;
    if skills_migrated > 0 {
        println!("  Skills → {} skill(s) copied", skills_migrated);
    }

    // 3. Memories: MEMORY.md or memories/ → SQLite
    let memories_migrated = migrate_memories(openclaw_dir, &target_dir)?;
    if memories_migrated > 0 {
        println!("  Memories → {} entries imported", memories_migrated);
    }

    // 4. Generate config template if it doesn't exist
    let config_path = target_dir.join("config.toml");
    if !config_path.exists() {
        generate_config_template(openclaw_dir, &config_path)?;
        println!("  Config template → {}", config_path.display());
    } else {
        println!(
            "  Config already exists: {} (skipped)",
            config_path.display()
        );
    }

    println!("Migration complete.");
    Ok(())
}

fn migrate_persona(openclaw_dir: &Path, target: &Path) -> anyhow::Result<bool> {
    if target.exists() {
        println!("  Persona already exists (skipped)");
        return Ok(false);
    }

    // Try SOUL.md first, then IDENTITY.md
    for name in &["SOUL.md", "IDENTITY.md", "soul.md", "identity.md"] {
        let src = openclaw_dir.join(name);
        if src.exists() {
            std::fs::copy(&src, target)?;
            return Ok(true);
        }
    }

    println!("  No SOUL.md or IDENTITY.md found (skipped)");
    Ok(false)
}

fn migrate_skills(openclaw_dir: &Path, target_skills_dir: &Path) -> anyhow::Result<usize> {
    let skills_dir = openclaw_dir.join("skills");
    if !skills_dir.exists() {
        return Ok(0);
    }

    let mut count = 0;
    for entry in std::fs::read_dir(&skills_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let dest = target_skills_dir.join(&name);
            if dest.exists() {
                println!("  Skill '{}' already exists (skipped)", name);
                continue;
            }
            copy_dir_recursive(&path, &dest)?;
            count += 1;
        }
    }

    Ok(count)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

fn migrate_memories(openclaw_dir: &Path, target_dir: &Path) -> anyhow::Result<usize> {
    let db_path = target_dir.join("yoclaw.db");
    let db = crate::db::Db::open(&db_path)?;
    let mut count = 0;

    // Try MEMORY.md
    let memory_file = openclaw_dir.join("MEMORY.md");
    if memory_file.exists() {
        let content = std::fs::read_to_string(&memory_file)?;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("---") {
                continue;
            }
            // Strip leading "- " from list items
            let text = line.strip_prefix("- ").unwrap_or(line);
            if !text.is_empty() {
                db.exec_sync(|conn| {
                    let ts = crate::db::now_ms() as i64;
                    conn.execute(
                        "INSERT INTO memory (content, source, category, importance, created_at, updated_at)
                         VALUES (?1, 'migrated', 'fact', 5, ?2, ?2)",
                        rusqlite::params![text, ts],
                    )?;
                    Ok(())
                })?;
                count += 1;
            }
        }
    }

    // Try memories/ directory
    let memories_dir = openclaw_dir.join("memories");
    if memories_dir.exists() {
        for entry in std::fs::read_dir(&memories_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "md") {
                let content = std::fs::read_to_string(&path)?;
                let key = path.file_stem().unwrap().to_string_lossy().to_string();
                db.exec_sync(|conn| {
                    let ts = crate::db::now_ms() as i64;
                    conn.execute(
                        "INSERT INTO memory (key, content, source, category, importance, created_at, updated_at)
                         VALUES (?1, ?2, 'migrated', 'fact', 5, ?3, ?3)",
                        rusqlite::params![key, content, ts],
                    )?;
                    Ok(())
                })?;
                count += 1;
            }
        }
    }

    Ok(count)
}

fn generate_config_template(openclaw_dir: &Path, target: &Path) -> anyhow::Result<()> {
    // Try to detect provider from OpenClaw config
    let mut provider = "anthropic";
    let mut model = "claude-sonnet-4-20250514";

    for config_name in &["config.toml", "config.yaml", "config.json", ".env"] {
        let path = openclaw_dir.join(config_name);
        if path.exists() {
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            if content.contains("openai") || content.contains("gpt-") {
                provider = "openai";
                model = "gpt-4o";
            } else if content.contains("google") || content.contains("gemini") {
                provider = "google";
                model = "gemini-2.0-flash";
            }
            break;
        }
    }

    let template = format!(
        r#"# Generated by yoclaw migrate
[agent]
provider = "{provider}"
model = "{model}"
api_key = "${{ANTHROPIC_API_KEY}}"

[agent.budget]
max_tokens_per_day = 1_000_000
max_turns_per_session = 50

[agent.context]
max_context_tokens = 180000
keep_recent = 4
tool_output_max_lines = 50

[channels.telegram]
bot_token = "${{TELEGRAM_BOT_TOKEN}}"
allowed_senders = []
debounce_ms = 2000

[security]
shell_deny_patterns = ["rm -rf", "sudo", "chmod 777"]
"#
    );

    std::fs::write(target, template)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_migrate_persona() {
        let src = TempDir::new().unwrap();
        let dst = TempDir::new().unwrap();

        // Create SOUL.md
        std::fs::write(src.path().join("SOUL.md"), "I am an AI assistant.").unwrap();

        let target = dst.path().join("persona.md");
        let migrated = migrate_persona(src.path(), &target).unwrap();
        assert!(migrated);
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "I am an AI assistant."
        );
    }

    #[test]
    fn test_migrate_persona_skip_existing() {
        let src = TempDir::new().unwrap();
        let dst = TempDir::new().unwrap();

        std::fs::write(src.path().join("SOUL.md"), "new").unwrap();
        let target = dst.path().join("persona.md");
        std::fs::write(&target, "existing").unwrap();

        let migrated = migrate_persona(src.path(), &target).unwrap();
        assert!(!migrated);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "existing");
    }

    #[test]
    fn test_migrate_skills() {
        let src = TempDir::new().unwrap();
        let dst = TempDir::new().unwrap();

        // Create a skill
        let skill_dir = src.path().join("skills/coding");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "---\nname: coding\n---").unwrap();

        let count = migrate_skills(src.path(), dst.path()).unwrap();
        assert_eq!(count, 1);
        assert!(dst.path().join("coding/SKILL.md").exists());
    }

    #[test]
    fn test_migrate_memories() {
        let src = TempDir::new().unwrap();
        let dst = TempDir::new().unwrap();

        // Create MEMORY.md
        std::fs::write(
            src.path().join("MEMORY.md"),
            "# Memories\n\n- User prefers dark mode\n- Favorite language is Rust\n",
        )
        .unwrap();

        let count = migrate_memories(src.path(), dst.path()).unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_generate_config_template() {
        let src = TempDir::new().unwrap();
        let dst = TempDir::new().unwrap();

        let target = dst.path().join("config.toml");
        generate_config_template(src.path(), &target).unwrap();

        let content = std::fs::read_to_string(&target).unwrap();
        assert!(content.contains("[agent]"));
        assert!(content.contains("anthropic"));
    }
}
