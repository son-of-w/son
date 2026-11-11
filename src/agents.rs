//! The AI agents `son` can set itself up for, and where each one keeps its
//! skills and its always-on instruction file.
//!
//! The skills folders and the folders used to detect an agent follow the
//! table in vercel-labs/skills (`src/agents.ts`, MIT licence), so that a
//! `son` skill lands where `npx skills add` would put one. Agents whose
//! folders depend on the working directory in other ways are left out.

use std::path::{Path, PathBuf};

/// A place on disk, written so it can be worked out for any home directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// Under the home directory.
    Home(&'static str),
    /// Under `$XDG_CONFIG_HOME`, else `~/.config`.
    Config(&'static str),
    /// Under the directory named by an environment variable, else under a
    /// default below the home directory: `(variable, default, rest)`.
    Env(&'static str, &'static str, &'static str),
    /// A fixed path.
    Abs(&'static str),
}

fn var(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub fn home() -> Option<PathBuf> {
    var("HOME")
}

impl Dir {
    pub fn path(&self) -> Option<PathBuf> {
        Some(match *self {
            Dir::Home(rest) => home()?.join(rest),
            Dir::Config(rest) => match var("XDG_CONFIG_HOME") {
                Some(config) => config.join(rest),
                None => home()?.join(".config").join(rest),
            },
            Dir::Env(variable, default, rest) => {
                let base = match var(variable) {
                    Some(base) => base,
                    None => home()?.join(default),
                };
                if rest.is_empty() {
                    base
                } else {
                    base.join(rest)
                }
            }
            Dir::Abs(path) => PathBuf::from(path),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// For every project on this machine.
    Global,
    /// For one project folder only.
    Project,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Global => "global",
            Scope::Project => "project",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Agent {
    /// The name used on the command line, for example `claude-code`.
    pub name: &'static str,
    pub display: &'static str,
    /// Skills folder inside a project.
    pub project_skills: &'static str,
    /// Skills folder for every project.
    pub global_skills: Dir,
    /// The agent is taken to be installed when any of these exists.
    pub detect: &'static [Dir],
    /// The always-on instruction file inside a project.
    pub project_rules: &'static str,
    /// The always-on instruction file for every project, when the agent has
    /// one that is a plain file. `None` for agents that keep global rules in
    /// their settings or do not have them.
    pub global_rules: Option<Dir>,
}

/// The instruction file most agents read from a project's root.
const AGENTS_MD: &str = "AGENTS.md";
/// Folder name and file name of the skill inside a skills folder.
const SKILL_PATH: &str = "son/SKILL.md";

const fn agent(
    name: &'static str,
    display: &'static str,
    project_skills: &'static str,
    global_skills: Dir,
    detect: &'static [Dir],
) -> Agent {
    Agent {
        name,
        display,
        project_skills,
        global_skills,
        detect,
        project_rules: AGENTS_MD,
        global_rules: None,
    }
}

use Dir::{Abs, Config, Env, Home};

/// Skills in the shared `.agents/skills` folder of a project.
const SHARED: &str = ".agents/skills";

#[rustfmt::skip]
const TABLE: &[Agent] = &[
    agent("adal", "AdaL", ".adal/skills", Home(".adal/skills"), &[Home(".adal")]),
    agent("aider-desk", "AiderDesk", ".aider-desk/skills", Home(".aider-desk/skills"), &[Home(".aider-desk")]),
    agent("amp", "Amp", SHARED, Config("agents/skills"), &[Config("amp")]),
    Agent { global_rules: Some(Home(".gemini/GEMINI.md")),
        ..agent("antigravity", "Antigravity", SHARED, Home(".gemini/config/skills"), &[Home(".gemini/antigravity")]) },
    Agent { global_rules: Some(Home(".gemini/GEMINI.md")),
        ..agent("antigravity-cli", "Antigravity CLI", SHARED, Home(".gemini/config/skills"), &[Home(".gemini/antigravity-cli")]) },
    agent("augment", "Augment", ".augment/skills", Home(".augment/skills"), &[Home(".augment")]),
    agent("autohand-code", "Autohand Code CLI", ".autohand/skills", Env("AUTOHAND_HOME", ".autohand", "skills"), &[Env("AUTOHAND_HOME", ".autohand", "")]),
    agent("bob", "IBM Bob", ".bob/skills", Home(".bob/skills"), &[Home(".bob")]),
    Agent { project_rules: "CLAUDE.md", global_rules: Some(Env("CLAUDE_CONFIG_DIR", ".claude", "CLAUDE.md")),
        ..agent("claude-code", "Claude Code", ".claude/skills", Env("CLAUDE_CONFIG_DIR", ".claude", "skills"), &[Env("CLAUDE_CONFIG_DIR", ".claude", "")]) },
    agent("cline", "Cline", SHARED, Home(".agents/skills"), &[Home(".cline")]),
    agent("codearts-agent", "CodeArts Agent", ".codeartsdoer/skills", Home(".codeartsdoer/skills"), &[Home(".codeartsdoer")]),
    agent("codebuddy", "CodeBuddy", ".codebuddy/skills", Home(".codebuddy/skills"), &[Home(".codebuddy")]),
    agent("codemaker", "Codemaker", ".codemaker/skills", Home(".codemaker/skills"), &[Home(".codemaker")]),
    agent("codestudio", "Code Studio", ".codestudio/skills", Home(".codestudio/skills"), &[Home(".codestudio")]),
    Agent { global_rules: Some(Env("CODEX_HOME", ".codex", "AGENTS.md")),
        ..agent("codex", "Codex", SHARED, Home(".agents/skills"), &[Env("CODEX_HOME", ".codex", ""), Abs("/etc/codex")]) },
    agent("command-code", "Command Code", ".commandcode/skills", Home(".commandcode/skills"), &[Home(".commandcode")]),
    agent("continue", "Continue", ".continue/skills", Home(".continue/skills"), &[Home(".continue")]),
    agent("cortex", "Cortex Code", ".cortex/skills", Home(".snowflake/cortex/skills"), &[Home(".snowflake/cortex")]),
    agent("crush", "Crush", ".crush/skills", Home(".config/crush/skills"), &[Home(".config/crush")]),
    agent("cursor", "Cursor", SHARED, Home(".cursor/skills"), &[Home(".cursor")]),
    agent("deepagents", "Deep Agents", SHARED, Home(".deepagents/agent/skills"), &[Home(".deepagents")]),
    agent("devin", "Devin for Terminal", ".devin/skills", Config("devin/skills"), &[Config("devin")]),
    agent("dexto", "Dexto", SHARED, Home(".agents/skills"), &[Home(".dexto")]),
    agent("droid", "Droid", SHARED, Home(".factory/skills"), &[Home(".factory")]),
    agent("firebender", "Firebender", SHARED, Home(".firebender/skills"), &[Home(".firebender")]),
    agent("forgecode", "ForgeCode", ".forge/skills", Home(".forge/skills"), &[Home(".forge")]),
    agent("fx", "fx", SHARED, Home(".agents/skills"), &[Home(".fx")]),
    Agent { project_rules: "GEMINI.md", global_rules: Some(Home(".gemini/GEMINI.md")),
        ..agent("gemini-cli", "Gemini CLI", SHARED, Home(".gemini/skills"), &[Home(".gemini")]) },
    agent("github-copilot", "GitHub Copilot", SHARED, Home(".copilot/skills"), &[Home(".copilot")]),
    agent("goose", "Goose", ".goose/skills", Config("goose/skills"), &[Config("goose")]),
    agent("grok", "Grok Build", ".grok/skills", Env("GROK_HOME", ".grok", "skills"), &[Env("GROK_HOME", ".grok", "")]),
    agent("hermes-agent", "Hermes Agent", ".hermes/skills", Env("HERMES_HOME", ".hermes", "skills"), &[Env("HERMES_HOME", ".hermes", "")]),
    agent("iflow-cli", "iFlow CLI", ".iflow/skills", Home(".iflow/skills"), &[Home(".iflow")]),
    agent("inference-sh", "inference.sh", ".inferencesh/skills", Home(".inferencesh/skills"), &[Home(".inferencesh")]),
    agent("jazz", "Jazz", ".jazz/skills", Home(".jazz/skills"), &[Home(".jazz")]),
    agent("junie", "Junie", ".junie/skills", Home(".junie/skills"), &[Home(".junie")]),
    agent("kilo", "Kilo Code", SHARED, Home(".kilo/skills"), &[Home(".kilo"), Home(".kilocode")]),
    agent("kimchi", "Kimchi", ".kimchi/skills", Home(".config/kimchi/harness/skills"), &[Home(".config/kimchi")]),
    agent("kimi-code-cli", "Kimi Code CLI", SHARED, Home(".agents/skills"), &[Home(".kimi-code"), Home(".kimi")]),
    agent("kiro-cli", "Kiro CLI", ".kiro/skills", Home(".kiro/skills"), &[Home(".kiro")]),
    agent("kode", "Kode", ".kode/skills", Home(".kode/skills"), &[Home(".kode")]),
    agent("lingma", "Lingma", ".lingma/skills", Home(".lingma/skills"), &[Home(".lingma")]),
    agent("loaf", "Loaf", SHARED, Home(".agents/skills"), &[Home(".loaf")]),
    agent("mcpjam", "MCPJam", ".mcpjam/skills", Home(".mcpjam/skills"), &[Home(".mcpjam")]),
    agent("minimax-code", "MiniMax Code", ".minimax/skills", Home(".minimax/skills"), &[Home(".minimax"), Abs("/Applications/MiniMax Code.app")]),
    agent("mistral-vibe", "Mistral Vibe", ".vibe/skills", Env("VIBE_HOME", ".vibe", "skills"), &[Env("VIBE_HOME", ".vibe", "")]),
    agent("moxby", "Moxby", ".moxby/skills", Home(".moxby/skills"), &[Home(".moxby")]),
    agent("mux", "Mux", ".mux/skills", Home(".mux/skills"), &[Home(".mux")]),
    agent("neovate", "Neovate", ".neovate/skills", Home(".neovate/skills"), &[Home(".neovate")]),
    agent("ona", "Ona", ".ona/skills", Home(".ona/skills"), &[Home(".ona")]),
    Agent { global_rules: Some(Config("opencode/AGENTS.md")),
        ..agent("opencode", "OpenCode", SHARED, Config("opencode/skills"), &[Config("opencode")]) },
    agent("openhands", "OpenHands", ".openhands/skills", Home(".openhands/skills"), &[Home(".openhands")]),
    agent("pi", "Pi", SHARED, Home(".agents/skills"), &[Home(".pi/agent")]),
    agent("pochi", "Pochi", ".pochi/skills", Home(".pochi/skills"), &[Home(".pochi")]),
    agent("posit-assistant", "Posit Assistant", ".posit/assistant/skills", Home(".posit/assistant/skills"), &[Home(".posit/assistant"), Home(".positai")]),
    agent("qoder", "Qoder", ".qoder/skills", Home(".qoder/skills"), &[Home(".qoder")]),
    agent("qoder-cn", "Qoder CN", ".qoder/skills", Home(".qoder-cn/skills"), &[Home(".qoder-cn")]),
    agent("qwen-code", "Qwen Code", ".qwen/skills", Home(".qwen/skills"), &[Home(".qwen")]),
    agent("reasonix", "Reasonix", ".reasonix/skills", Env("REASONIX_HOME", ".reasonix", "skills"), &[Env("REASONIX_HOME", ".reasonix", "")]),
    agent("roo", "Roo Code", ".roo/skills", Home(".roo/skills"), &[Home(".roo")]),
    agent("rovodev", "Rovo Dev", ".rovodev/skills", Home(".rovodev/skills"), &[Home(".rovodev")]),
    agent("sarvam-code", "Sarvam Code", SHARED, Home(".agents/skills"), &[Env("SARVAM_HOME", ".sarvam", "")]),
    agent("tabnine-cli", "Tabnine CLI", ".tabnine/agent/skills", Home(".tabnine/agent/skills"), &[Home(".tabnine")]),
    agent("terramind", "Terramind", ".terramind/skills", Home(".terramind/skills"), &[Home(".terramind")]),
    agent("tinycloud", "Tinycloud", ".tinycloud/skills", Home(".tinycloud/skills"), &[Home(".tinycloud")]),
    agent("trae", "Trae", ".trae/skills", Home(".trae/skills"), &[Home(".trae")]),
    agent("trae-cn", "Trae CN", ".trae/skills", Home(".trae-cn/skills"), &[Home(".trae-cn")]),
    agent("warp", "Warp", SHARED, Home(".agents/skills"), &[Home(".warp")]),
    agent("windsurf", "Windsurf", ".windsurf/skills", Home(".codeium/windsurf/skills"), &[Home(".codeium/windsurf")]),
    agent("zcode", "ZCode", ".zcode/skills", Home(".zcode/skills"), &[Home(".zcode"), Abs("/Applications/ZCode.app")]),
    agent("zed", "Zed", SHARED, Home(".agents/skills"), &[Config("zed")]),
    agent("zencoder", "Zencoder", ".zencoder/skills", Home(".zencoder/skills"), &[Home(".zencoder")]),
    agent("zenflow", "Zenflow", ".zencoder/skills", Home(".zencoder/skills"), &[Home(".zencoder")]),
];

/// Shown first in the list when choosing agents.
const WELL_KNOWN: &[&str] = &[
    "claude-code",
    "codex",
    "cursor",
    "antigravity",
    "gemini-cli",
    "github-copilot",
    "opencode",
    "windsurf",
];

/// Short names people type for an agent.
const ALIASES: &[(&str, &str)] = &[
    ("claude", "claude-code"),
    ("gemini", "gemini-cli"),
    ("copilot", "github-copilot"),
    ("agy", "antigravity-cli"),
    ("vibe", "mistral-vibe"),
    ("kiro", "kiro-cli"),
];

impl Agent {
    pub fn detected(&self) -> bool {
        self.detect
            .iter()
            .any(|dir| dir.path().is_some_and(|path| path.exists()))
    }

    /// Where the `son` skill goes for this agent.
    pub fn skill_file(&self, scope: Scope, project_root: &Path) -> Option<PathBuf> {
        let skills = match scope {
            Scope::Global => self.global_skills.path()?,
            Scope::Project => project_root.join(self.project_skills),
        };
        Some(skills.join(SKILL_PATH))
    }

    /// The always-on instruction file for this agent, if it has one here.
    pub fn rules_file(&self, scope: Scope, project_root: &Path) -> Option<PathBuf> {
        match scope {
            Scope::Global => self.global_rules?.path(),
            Scope::Project => Some(project_root.join(self.project_rules)),
        }
    }
}

/// Every agent, in the order to offer them: detected first, then the
/// well-known ones, then the rest by name.
pub fn all() -> Vec<Agent> {
    let mut agents = TABLE.to_vec();
    agents.sort_by_cached_key(|agent| {
        let known = WELL_KNOWN
            .iter()
            .position(|name| *name == agent.name)
            .unwrap_or(WELL_KNOWN.len());
        (!agent.detected(), known, agent.display.to_lowercase())
    });
    agents
}

/// The agent with this name or alias, in any letter case.
pub fn find(name: &str) -> Option<Agent> {
    let name = name.trim().to_lowercase();
    let name = ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map_or(name.as_str(), |(_, full)| full);
    TABLE.iter().find(|agent| agent.name == name).cloned()
}

/// Every place `son` could have been set up, without repeats: skills and
/// instruction files, global and for the project at `project_root`.
pub fn candidate_files(project_root: &Path) -> Vec<(PathBuf, bool)> {
    let mut files: Vec<(PathBuf, bool)> = Vec::new();
    let mut push = |path: Option<PathBuf>, is_skill: bool| {
        if let Some(path) = path {
            if !files.iter().any(|(known, _)| *known == path) {
                files.push((path, is_skill));
            }
        }
    };
    for agent in TABLE {
        for scope in [Scope::Global, Scope::Project] {
            push(agent.skill_file(scope, project_root), true);
            push(agent.rules_file(scope, project_root), false);
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn names_are_unique_and_aliases_point_at_real_agents() {
        let names: HashSet<&str> = TABLE.iter().map(|agent| agent.name).collect();
        assert_eq!(names.len(), TABLE.len());
        for (alias, full) in ALIASES {
            assert!(names.contains(full), "{alias} points at a missing agent");
            assert!(!names.contains(alias), "{alias} is also an agent name");
        }
        for name in WELL_KNOWN {
            assert!(names.contains(name));
        }
    }

    #[test]
    fn find_takes_names_and_aliases_in_any_case() {
        assert_eq!(find("Claude").unwrap().name, "claude-code");
        assert_eq!(find(" codex ").unwrap().name, "codex");
        assert_eq!(find("CURSOR").unwrap().name, "cursor");
        assert!(find("no-such-agent").is_none());
    }

    #[test]
    fn project_paths_sit_under_the_project_root() {
        let root = Path::new("/work/app");
        let claude = find("claude-code").unwrap();
        assert_eq!(
            claude.skill_file(Scope::Project, root).unwrap(),
            Path::new("/work/app/.claude/skills/son/SKILL.md")
        );
        assert_eq!(
            claude.rules_file(Scope::Project, root).unwrap(),
            Path::new("/work/app/CLAUDE.md")
        );
        let cursor = find("cursor").unwrap();
        assert_eq!(
            cursor.skill_file(Scope::Project, root).unwrap(),
            Path::new("/work/app/.agents/skills/son/SKILL.md")
        );
        assert_eq!(
            cursor.rules_file(Scope::Project, root).unwrap(),
            Path::new("/work/app/AGENTS.md")
        );
    }
}
