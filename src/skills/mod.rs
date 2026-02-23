pub mod manifest;

use crate::security::SecurityPolicy;
use manifest::{parse_manifest, SkillManifest};
use std::path::Path;

/// A loaded skill with its manifest (including required tools) and file path.
#[derive(Debug, Clone)]
pub struct LoadedSkill {
    pub manifest: SkillManifest,
    pub dir_name: String,
    pub file_path: std::path::PathBuf,
}

/// Load skills from directories, filtering out any that require disabled tools.
///
/// Returns a prompt fragment (XML) and the list of loaded skill manifests.
/// The prompt fragment can be appended to the system prompt directly.
pub fn load_filtered_skills(
    dirs: &[&Path],
    policy: &SecurityPolicy,
) -> (String, Vec<LoadedSkill>) {
    // Load all skills via yoagent to reuse its directory scanning + frontmatter parsing
    let all_skills = yoagent::SkillSet::load(dirs).unwrap_or_default();

    let mut kept_skills = Vec::new();
    let mut excluded_names = Vec::new();

    for skill in all_skills.skills() {
        let content = match std::fs::read_to_string(&skill.file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let manifest = match parse_manifest(&content) {
            Some(m) => m,
            None => {
                // No parseable manifest — include the skill (no tool filtering)
                kept_skills.push(LoadedSkill {
                    manifest: SkillManifest {
                        name: skill.name.clone(),
                        description: skill.description.clone(),
                        tools: Vec::new(),
                    },
                    dir_name: skill.name.clone(),
                    file_path: skill.file_path.clone(),
                });
                continue;
            }
        };

        // Check if all required tools are enabled
        let all_tools_available = manifest.tools.iter().all(|tool| {
            match policy.tool_permissions.get(tool.as_str()) {
                Some(perm) => perm.enabled,
                None => true, // Unknown tools are allowed by default
            }
        });

        if all_tools_available {
            kept_skills.push(LoadedSkill {
                manifest,
                dir_name: skill.name.clone(),
                file_path: skill.file_path.clone(),
            });
        } else {
            excluded_names.push(skill.name.clone());
        }
    }

    if !excluded_names.is_empty() {
        tracing::info!(
            "Excluded skills (disabled tools): {}",
            excluded_names.join(", ")
        );
    }

    // Build the prompt fragment directly (same XML format as yoagent's SkillSet)
    let prompt = format_skills_for_prompt(&kept_skills);

    (prompt, kept_skills)
}

/// Format kept skills as XML for the system prompt.
/// Matches yoagent's `SkillSet::format_for_prompt()` format.
fn format_skills_for_prompt(skills: &[LoadedSkill]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut out = String::from("<available_skills>\n");
    for skill in skills {
        out.push_str("  <skill>\n");
        out.push_str(&format!(
            "    <name>{}</name>\n",
            xml_escape(&skill.manifest.name)
        ));
        out.push_str(&format!(
            "    <description>{}</description>\n",
            xml_escape(&skill.manifest.description)
        ));
        out.push_str(&format!(
            "    <location>{}</location>\n",
            xml_escape(&skill.file_path.to_string_lossy())
        ));
        out.push_str("  </skill>\n");
    }
    out.push_str("</available_skills>");
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Format loaded skills for display (inspect command).
pub fn format_skills_info(skills: &[LoadedSkill]) -> String {
    if skills.is_empty() {
        return "No skills loaded.".to_string();
    }

    skills
        .iter()
        .map(|s| {
            let tools = if s.manifest.tools.is_empty() {
                "none".to_string()
            } else {
                s.manifest.tools.join(", ")
            };
            format!(
                "  {} — {} (tools: {})",
                s.manifest.name, s.manifest.description, tools
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::{SecurityPolicy, ToolPerm};
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn create_skill(dir: &Path, name: &str, description: &str, tools: &[&str]) {
        let skill_dir = dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        let tools_str = if tools.is_empty() {
            String::new()
        } else {
            format!(
                "tools: [{}]\n",
                tools
                    .iter()
                    .map(|t| t.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!(
                "---\nname: {}\ndescription: {}\n{}---\n\n# {}\nInstructions.\n",
                name, description, tools_str, name
            ),
        )
        .unwrap();
    }

    fn permissive_policy() -> SecurityPolicy {
        SecurityPolicy {
            shell_deny_patterns: vec![],
            tool_permissions: HashMap::new(),
        }
    }

    fn restricted_policy() -> SecurityPolicy {
        SecurityPolicy {
            shell_deny_patterns: vec![],
            tool_permissions: HashMap::from([
                (
                    "shell".to_string(),
                    ToolPerm {
                        enabled: false,
                        allowed_paths: vec![],
                        allowed_hosts: vec![],
                        requires_approval: false,
                    },
                ),
                (
                    "http".to_string(),
                    ToolPerm {
                        enabled: true,
                        allowed_paths: vec![],
                        allowed_hosts: vec![],
                        requires_approval: false,
                    },
                ),
            ]),
        }
    }

    #[test]
    fn test_load_all_skills_permissive() {
        let tmp = TempDir::new().unwrap();
        create_skill(tmp.path(), "weather", "Get weather", &["http"]);
        create_skill(tmp.path(), "coding", "Write code", &["shell"]);

        let (prompt, loaded) = load_filtered_skills(&[tmp.path()], &permissive_policy());
        assert_eq!(loaded.len(), 2);
        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("weather"));
        assert!(prompt.contains("coding"));
    }

    #[test]
    fn test_filter_by_disabled_tool() {
        let tmp = TempDir::new().unwrap();
        create_skill(tmp.path(), "weather", "Get weather", &["http"]);
        create_skill(tmp.path(), "coding", "Write code", &["shell"]);

        let (prompt, loaded) = load_filtered_skills(&[tmp.path()], &restricted_policy());
        // "coding" requires shell which is disabled
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].manifest.name, "weather");
        assert!(prompt.contains("weather"));
        assert!(!prompt.contains("coding"));
    }

    #[test]
    fn test_skill_no_tools_always_included() {
        let tmp = TempDir::new().unwrap();
        create_skill(tmp.path(), "greeting", "Greet users", &[]);

        let (prompt, loaded) = load_filtered_skills(&[tmp.path()], &restricted_policy());
        assert_eq!(loaded.len(), 1);
        assert!(prompt.contains("greeting"));
    }

    #[test]
    fn test_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let (prompt, loaded) = load_filtered_skills(&[tmp.path()], &permissive_policy());
        assert!(loaded.is_empty());
        assert!(prompt.is_empty());
    }

    #[test]
    fn test_format_skills_info() {
        let skills = vec![
            LoadedSkill {
                manifest: SkillManifest {
                    name: "weather".into(),
                    description: "Get weather".into(),
                    tools: vec!["http".into()],
                },
                dir_name: "weather".into(),
                file_path: "/tmp/weather/SKILL.md".into(),
            },
            LoadedSkill {
                manifest: SkillManifest {
                    name: "coding".into(),
                    description: "Write code".into(),
                    tools: vec!["shell".into(), "write_file".into()],
                },
                dir_name: "coding".into(),
                file_path: "/tmp/coding/SKILL.md".into(),
            },
        ];
        let info = format_skills_info(&skills);
        assert!(info.contains("weather"));
        assert!(info.contains("http"));
        assert!(info.contains("coding"));
        assert!(info.contains("shell, write_file"));
    }
}
