/// Parse extended YAML frontmatter from SKILL.md files.
///
/// yoagent's built-in parser only extracts `name` and `description`.
/// We additionally parse the `tools` field for capability-based filtering.

/// Parsed skill manifest from SKILL.md frontmatter.
#[derive(Debug, Clone)]
pub struct SkillManifest {
    pub name: String,
    pub description: String,
    /// Tools this skill requires (e.g. ["http", "shell"]).
    pub tools: Vec<String>,
}

/// Parse a SKILL.md file's YAML frontmatter, extracting name, description, and tools.
pub fn parse_manifest(content: &str) -> Option<SkillManifest> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }

    let after_open = &trimmed[3..];
    let end = after_open.find("\n---")?;
    let yaml_block = &after_open[..end];

    let mut name = None;
    let mut description = None;
    let mut tools = Vec::new();

    for line in yaml_block.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("name:") {
            name = Some(unquote(rest.trim()));
        } else if let Some(rest) = line.strip_prefix("description:") {
            description = Some(unquote(rest.trim()));
        } else if let Some(rest) = line.strip_prefix("tools:") {
            tools = parse_tools_value(rest.trim());
        }
    }

    Some(SkillManifest {
        name: name?,
        description: description?,
        tools,
    })
}

/// Parse a YAML inline list like `[http, shell]` or `[http]`.
fn parse_tools_value(s: &str) -> Vec<String> {
    let s = s.trim();
    if s.starts_with('[') && s.ends_with(']') {
        s[1..s.len() - 1]
            .split(',')
            .map(|t| unquote(t.trim()))
            .filter(|t| !t.is_empty())
            .collect()
    } else if !s.is_empty() {
        // Single value without brackets
        vec![unquote(s)]
    } else {
        Vec::new()
    }
}

fn unquote(s: &str) -> String {
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_manifest_with_tools() {
        let content = r#"---
name: home-automation
description: Control smart home devices via Home Assistant API
tools: [http]
---

# Home Automation
Instructions here.
"#;
        let manifest = parse_manifest(content).unwrap();
        assert_eq!(manifest.name, "home-automation");
        assert_eq!(
            manifest.description,
            "Control smart home devices via Home Assistant API"
        );
        assert_eq!(manifest.tools, vec!["http"]);
    }

    #[test]
    fn test_parse_manifest_multiple_tools() {
        let content = "---\nname: coding\ndescription: Write code\ntools: [shell, read_file, write_file]\n---\n";
        let manifest = parse_manifest(content).unwrap();
        assert_eq!(manifest.tools, vec!["shell", "read_file", "write_file"]);
    }

    #[test]
    fn test_parse_manifest_no_tools() {
        let content = "---\nname: greeting\ndescription: Greet users\n---\n";
        let manifest = parse_manifest(content).unwrap();
        assert!(manifest.tools.is_empty());
    }

    #[test]
    fn test_parse_manifest_quoted_tools() {
        let content = "---\nname: test\ndescription: Test\ntools: [\"http\", \"shell\"]\n---\n";
        let manifest = parse_manifest(content).unwrap();
        assert_eq!(manifest.tools, vec!["http", "shell"]);
    }

    #[test]
    fn test_parse_manifest_missing_frontmatter() {
        let content = "# No frontmatter\nJust markdown.";
        assert!(parse_manifest(content).is_none());
    }

    #[test]
    fn test_parse_manifest_missing_name() {
        let content = "---\ndescription: Has desc\n---\n";
        assert!(parse_manifest(content).is_none());
    }

    #[test]
    fn test_parse_single_tool_no_brackets() {
        let content = "---\nname: simple\ndescription: Simple skill\ntools: http\n---\n";
        let manifest = parse_manifest(content).unwrap();
        assert_eq!(manifest.tools, vec!["http"]);
    }
}
