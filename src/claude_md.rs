//! The block of instructions that tells Claude to use `son`, and the code
//! that puts it into `~/.claude/CLAUDE.md`.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub const START: &str = "<!-- son:start -->";
pub const END: &str = "<!-- son:end -->";

const BODY: &str = r#"## Shared memory: `son`

`son` is a local command that keeps decisions, patterns, facts and todos across all sessions and all projects. Use it through the shell. This block is managed by `son init`; changes made inside it are overwritten.

- At the start of a session, run `son context` and treat the output as prior context for this project.
- Before any non-trivial task, run `son recall <terms>` with three to five related terms: tool names, cloud providers, the project name. Recall searches every project and lists the current project's matches first.
- When a decision is made, a way of configuring something is settled, or a todo is created or finished, record it at once:
  - `son remember <decision|pattern|fact|note> "<title>" --body "<text>" --tag a,b`
  - `son todo add "<title>"` and `son todo done <id>`
- Use `pattern` for anything that answers "how do we normally set this up".
- When the user says "remember this" or "I need to do this", save it with `son`.
- Do not duplicate. If `son recall` shows an equivalent node, change it with `son edit <id>`, or connect to it with `son link <from> <relation> <to>`. When a decision replaces an older one, link it with `supersedes`.
- Never store secrets: no passwords, tokens, API keys or private keys. Store where a secret lives, not the secret.
- `son --help` lists every command. Add `--json` for structured output.
"#;

/// The full block, markers included.
pub fn block() -> String {
    format!("{START}\n{BODY}{END}\n")
}

/// `$CLAUDE_CONFIG_DIR/CLAUDE.md`, else `~/.claude/CLAUDE.md`.
pub fn target() -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    let dir = match var("CLAUDE_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(var("HOME")?).join(".claude"),
    };
    Some(dir.join("CLAUDE.md"))
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Created,
    Added,
    Updated,
    Unchanged,
}

impl Outcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Outcome::Created => "created",
            Outcome::Added => "added",
            Outcome::Updated => "updated",
            Outcome::Unchanged => "unchanged",
        }
    }
}

/// Put the block into the file at `path`. A second run replaces the block in
/// place, so it is never added twice. Text outside the markers is kept.
pub fn install(path: &Path) -> Result<Outcome> {
    let block = block();
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(err).with_context(|| format!("cannot read {}", path.display())),
    };

    let (text, outcome) = match existing {
        None => (block, Outcome::Created),
        Some(old) => match merge(&old, &block) {
            Some(new) if new == old => return Ok(Outcome::Unchanged),
            Some(new) => (new, Outcome::Updated),
            None => {
                let mut new = old.trim_end().to_string();
                if !new.is_empty() {
                    new.push_str("\n\n");
                }
                new.push_str(&block);
                (new, Outcome::Added)
            }
        },
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    std::fs::write(path, text).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(outcome)
}

/// Replace an existing block in `old`. `None` when `old` has no block.
fn merge(old: &str, block: &str) -> Option<String> {
    let start = old.find(START)?;
    let end = start + old[start..].find(END)? + END.len();
    let after = old[end..].strip_prefix('\n').unwrap_or(&old[end..]);
    Some(format!("{}{}{}", &old[..start], block, after))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_replaces_only_the_block() {
        let old = format!("# Mine\n\n{START}\nold text\n{END}\n\n## After\n");
        let new = merge(&old, &block()).unwrap();
        assert!(new.starts_with("# Mine\n\n"));
        assert!(new.ends_with("\n## After\n"));
        assert!(!new.contains("old text"));
        assert_eq!(new.matches(START).count(), 1);
    }

    #[test]
    fn merge_is_stable() {
        let once = format!("# Mine\n\n{}", block());
        assert_eq!(merge(&once, &block()).unwrap(), once);
    }

    #[test]
    fn no_block_means_no_merge() {
        assert!(merge("# Mine\n", &block()).is_none());
    }
}
