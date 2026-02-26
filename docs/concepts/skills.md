# Skills

Skills are markdown files that extend your agent's knowledge and capabilities. They're injected into the system prompt at startup.

## Creating a skill

Create a directory under `~/.yoclaw/skills/` with a `SKILL.md` file:

```bash
mkdir -p ~/.yoclaw/skills/weather
```

```markdown
---
name: weather
description: Get current weather information
tools: [http]
---

# Weather Skill

When the user asks about weather, use the HTTP tool to query the OpenWeatherMap API:

- Base URL: https://api.openweathermap.org/data/2.5/weather
- Parameters: q={city}&appid={API_KEY}&units=metric
- The API key is stored in the environment as WEATHER_API_KEY

Always include:
- Current temperature in Celsius
- Weather description
- Humidity percentage
```

## SKILL.md format

Each skill file has YAML frontmatter followed by markdown content:

### Frontmatter fields

| Field | Required | Description |
|-------|----------|------------|
| `name` | Yes | Skill identifier |
| `description` | Yes | Short description shown in skill listings |
| `tools` | No | List of tools this skill requires |

### Tool filtering

The `tools` field is the key feature. If a skill requires a tool that's disabled in the security policy, the entire skill is excluded from the agent's prompt.

For example, if `shell` is disabled:

```yaml
---
name: deploy
description: Deploy applications
tools: [shell]     # Requires shell
---
```

This skill won't be loaded. But a skill with no tool requirements is always included:

```yaml
---
name: greetings
description: Multilingual greetings
tools: []          # No tools needed — always loaded
---
```

## Skill directory structure

```
~/.yoclaw/skills/
├── weather/
│   └── SKILL.md
├── coding/
│   └── SKILL.md
├── deployment/
│   └── SKILL.md
└── knowledge-base/
    └── SKILL.md
```

Each skill lives in its own directory. The directory name is used as the skill identifier if `name` isn't specified in the frontmatter.

## Custom skill directories

By default, yoclaw looks for skills in `~/.yoclaw/skills/`. You can specify additional directories:

```toml
[agent]
skills_dirs = ["~/.yoclaw/skills", "~/my-project/skills"]
```

## Viewing loaded skills

```bash
yoclaw inspect --skills
```

Output:

```
=== Skills (3) ===
  weather — Get current weather information (tools: http)
  coding — Write and review code (tools: shell, write_file)
  greetings — Multilingual greetings (tools: none)
```

## How skills work internally

Skills are loaded at startup by yoagent's `SkillSet::load()`, then filtered by yoclaw's security policy. The surviving skills are formatted as XML and appended to the system prompt:

```xml
<available_skills>
  <skill>
    <name>weather</name>
    <description>Get current weather information</description>
    <location>/home/user/.yoclaw/skills/weather/SKILL.md</location>
  </skill>
</available_skills>
```

The agent sees this in its system prompt and knows what skills are available and what they can do.

## Skill configuration requires restart

Skills are loaded once at startup. Adding, removing, or modifying skills requires restarting yoclaw.
