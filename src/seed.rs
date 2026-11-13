//! `son seed`: find the files a user already wrote for their agents, and
//! tell the agent how to bring what is in them into `son`.
//!
//! This command changes nothing. `son` has no model, so it cannot tell a
//! decision from a remark; the agent reads the files and saves what is
//! worth keeping, node by node, with the usual commands. The files stay
//! as they are.

use crate::agents::{self, Scope};
use crate::instructions;
use crate::project;
use crate::render;
use crate::setup::short_path;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// At most this many files from a folder the user names.
const FOLDER_LIMIT: usize = 200;
/// File endings that count as notes in a folder the user names.
const NOTE_ENDINGS: [&str; 4] = ["md", "mdc", "markdown", "txt"];
/// Folders that are never notes.
const SKIP_DIRS: [&str; 6] = ["node_modules", "target", "dist", "build", "vendor", "venv"];

pub struct File {
    pub path: PathBuf,
    pub bytes: u64,
    /// What kind of file it is, in a few words.
    pub kind: &'static str,
    /// "global", "project", or "folder" for a folder the user named.
    pub scope: &'static str,
    /// The file holds a `son` block, which is not to be brought in.
    pub son_block: bool,
}

/// True when the file holds text of the user's own: not empty, not a
/// `son` skill, and more than the `son` block.
fn own_text(text: &str) -> bool {
    if text.contains(instructions::SKILL_MARK) {
        return false;
    }
    let rest = match instructions::find_block(text) {
        Ok(Some(range)) => format!("{}{}", &text[..range.start], &text[range.end..]),
        _ => text.to_string(),
    };
    !rest.trim().is_empty()
}

fn look(path: &Path, kind: &'static str, scope: &'static str, found: &mut Vec<File>) {
    if found.iter().any(|known| known.path == path) {
        return;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    if !own_text(&text) {
        return;
    }
    found.push(File {
        path: path.to_path_buf(),
        bytes: text.len() as u64,
        kind,
        scope,
        son_block: text.contains(instructions::START),
    });
}

/// The note files in `dir` and below, sorted. Hidden folders and build
/// folders are left out.
fn notes_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    entries.sort();
    for path in entries {
        if out.len() >= FOLDER_LIMIT {
            return;
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if path.is_dir() {
            if !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_str()) {
                notes_under(&path, out);
            }
        } else if path
            .extension()
            .and_then(|ending| ending.to_str())
            .is_some_and(|ending| NOTE_ENDINGS.contains(&ending.to_lowercase().as_str()))
        {
            out.push(path);
        }
    }
}

/// The name Claude Code gives the folder of a project: the path with
/// every character that is not a letter or a digit made a `-`.
/// (Seen on macOS with Claude Code 2.1: `/Users/x/work/app` is
/// `-Users-x-work-app`.)
fn claude_project_name(dir: &Path) -> String {
    dir.display()
        .to_string()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Claude Code's own memory files for the folders `dirs`:
/// `<claude folder>/projects/<project name>/memory/*.md`.
fn claude_memory(dirs: &[&Path], found: &mut Vec<File>) {
    let Some(claude) = agents::find("claude-code") else {
        return;
    };
    // The skills folder is `<claude folder>/skills`.
    let Some(base) = claude
        .global_skills
        .path()
        .and_then(|skills| skills.parent().map(Path::to_path_buf))
    else {
        return;
    };
    for dir in dirs {
        let memory = base
            .join("projects")
            .join(claude_project_name(dir))
            .join("memory");
        let mut files = Vec::new();
        notes_under(&memory, &mut files);
        for file in files {
            look(&file, "Claude Code memory file", "project", found);
        }
    }
}

/// The files to offer for the current folder: the instruction and rule
/// files of every agent `son` knows, global and for this project, and
/// Claude Code's memory files for this project.
pub fn candidates() -> Result<Vec<File>> {
    let cwd = std::env::current_dir().context("cannot read the current directory")?;
    let root = project::root(&cwd);
    let mut found = Vec::new();
    for agent in agents::all() {
        if let Some(path) = agent.rules_file(Scope::Project, &root) {
            look(&path, "instruction file", "project", &mut found);
        }
    }
    // Rule files that some agents read beside their main file.
    for (path, kind) in [
        (".cursorrules", "Cursor rules file"),
        (
            ".github/copilot-instructions.md",
            "Copilot instruction file",
        ),
    ] {
        look(&root.join(path), kind, "project", &mut found);
    }
    let mut rules = Vec::new();
    notes_under(&root.join(".cursor").join("rules"), &mut rules);
    for file in rules {
        look(&file, "Cursor rules file", "project", &mut found);
    }
    claude_memory(&[&root, &cwd], &mut found);
    for agent in agents::all() {
        if let Some(path) = agent.rules_file(Scope::Global, &root) {
            look(&path, "instruction file", "global", &mut found);
        }
    }
    Ok(found)
}

/// The note files in a folder the user named.
fn folder(dir: &Path) -> Result<Vec<File>> {
    if !dir.is_dir() {
        bail!("{} is not a folder", dir.display());
    }
    let mut paths = Vec::new();
    notes_under(dir, &mut paths);
    let mut found = Vec::new();
    for path in paths {
        look(&path, "note file", "folder", &mut found);
    }
    Ok(found)
}

/// What the agent is told to do with the files.
fn steps(collection: &str) -> Vec<String> {
    vec![
        "Ask the user first which of these files to bring in, unless they already said. Nothing is saved until you save it.".to_string(),
        format!(
            "Read each file. Leave out the son block (the text from `{}` to `{}`): it is son's own.",
            instructions::START,
            instructions::END
        ),
        "Pick what holds for a long time: decisions (a choice and its reason), patterns (how something is normally set up), and stable facts. One node for one piece of knowledge. Do not copy a file whole, and do not save what is only about one task that is done.".to_string(),
        "Before each save, run the same command with `--check`. If a stored node says the same thing, do not save it again.".to_string(),
        "Save with where it came from: `son remember <decision|pattern|fact|note> \"<title>\" --body \"<text>\" --evidence observation --ref file:<path of the file>`.".to_string(),
        format!(
            "Knowledge about this project goes into the current collection ({collection}). Knowledge from a global file that is about another subject goes into a collection for that subject: add `--collection <name>`."
        ),
        "Leave out secrets: passwords, tokens, keys. Save where a secret lives, not the secret. If son refuses a write with `possible_secret`, take the secret out; do not add `--allow-secret`.".to_string(),
        "Do not change or delete the files. When you are done, tell the user what you saved and what you left out.".to_string(),
    ]
}

pub fn run(dir: Option<&str>, collection: &str, json: bool) -> Result<()> {
    let files = match dir {
        Some(dir) => folder(Path::new(dir))?,
        None => candidates()?,
    };
    let steps = steps(collection);
    if json {
        let list: Vec<Value> = files
            .iter()
            .map(|file| {
                json!({
                    "path": file.path.display().to_string(),
                    "bytes": file.bytes,
                    "kind": file.kind,
                    "scope": file.scope,
                    "son_block": file.son_block,
                })
            })
            .collect();
        render::print_json(&json!({
            "collection": collection,
            "changed": false,
            "files": list,
            "instructions": steps,
        }));
        return Ok(());
    }
    println!("# Seed: files to bring into son (current collection: {collection})");
    println!("NOTHING WAS SAVED. This only lists files and says how to bring them in.");
    if files.is_empty() {
        println!(
            "\nNo file was found. To look in a folder of notes, name it: `son seed <folder>`."
        );
        return Ok(());
    }
    println!("\n## Files");
    for file in &files {
        println!(
            "- {} · {} · {}, {} bytes{}",
            short_path(&file.path),
            file.kind,
            file.scope,
            file.bytes,
            if file.son_block {
                " · holds a son block: leave that part out"
            } else {
                ""
            }
        );
    }
    if dir.is_some() && files.len() >= FOLDER_LIMIT {
        println!("(only the first {FOLDER_LIMIT} files are listed)");
    }
    println!("\n## How to bring them in");
    for (number, step) in steps.iter().enumerate() {
        println!("{}. {step}", number + 1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_with_only_son_text_is_not_the_users_own() {
        assert!(!own_text(""));
        assert!(!own_text("  \n"));
        assert!(!own_text(instructions::skill()));
        assert!(!own_text(&instructions::block()));
        assert!(!own_text(&format!("\n{}\n", instructions::block())));
        assert!(own_text(&format!("# Mine\n\n{}", instructions::block())));
        assert!(own_text("# Mine\n"));
    }

    #[test]
    fn a_claude_project_folder_is_named_from_the_path() {
        assert_eq!(
            claude_project_name(Path::new("/Users/x/work/son-of-w")),
            "-Users-x-work-son-of-w"
        );
    }
}
