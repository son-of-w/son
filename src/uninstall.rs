//! `son uninstall`: take away what `son init` wrote. The skill files are
//! deleted, and the block is cut out of the instruction files; the text a
//! user wrote around the block stays. The database is never touched, and
//! the binary is not removed: that is the package manager's work.

use crate::agents::{self, Scope};
use crate::error::coded;
use crate::instructions;
use crate::project;
use crate::render;
use crate::setup::short_path;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

pub struct Options {
    /// Agent names from `--agent`. Empty means every agent.
    pub agents: Vec<String>,
    /// From `--scope`. `None` means the global scope and this project.
    pub scope: Option<Scope>,
    /// Do not ask.
    pub yes: bool,
    /// List what would be done, and change nothing.
    pub dry_run: bool,
    pub json: bool,
}

/// What is to be done with one file.
enum Plan {
    /// A `son` skill file: delete it.
    DeleteSkill,
    /// An instruction file that holds only the block: delete it.
    DeleteRules,
    /// An instruction file with other text: write it back without the block.
    CutBlock(String),
    /// Leave it, and say why. The run fails.
    Leave(String),
}

struct Found {
    path: PathBuf,
    is_skill: bool,
    plan: Plan,
}

/// `old` without the block at `range`, or `None` when only whitespace is
/// left. The text before and after the block is kept as it is, with one
/// empty line between the two parts.
fn without_block(old: &str, range: std::ops::Range<usize>) -> Option<String> {
    let before = old[..range.start].trim_end();
    let after = old[range.end..].trim_start_matches(['\r', '\n']);
    if before.is_empty() && after.trim().is_empty() {
        return None;
    }
    let mut text = String::new();
    text.push_str(before);
    if !before.is_empty() && !after.trim().is_empty() {
        text.push_str("\n\n");
    }
    if !after.trim().is_empty() {
        text.push_str(after);
    }
    if !text.ends_with('\n') {
        text.push('\n');
    }
    Some(text)
}

/// Look at one file. `None` when it holds nothing of `son`.
fn plan(path: &Path, is_skill: bool) -> Option<Plan> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => return Some(Plan::Leave(format!("it cannot be read: {err}"))),
    };
    if is_skill {
        // The file says it is managed by `son init`, and `init` replaces
        // it without asking; a copy changed by hand is deleted too.
        let ours =
            text.contains(instructions::SKILL_MARK) || text.contains(instructions::TODO_SKILL_MARK);
        return ours.then_some(Plan::DeleteSkill);
    }
    match instructions::find_block(&text) {
        Ok(None) => None,
        Ok(Some(range)) => Some(match without_block(&text, range) {
            None => Plan::DeleteRules,
            Some(rest) => Plan::CutBlock(rest),
        }),
        Err(fault) => Some(Plan::Leave(format!(
            "it has {fault}; to repair it, {}",
            instructions::REPAIR
        ))),
    }
}

/// The files to look at: for the named agents (or all), in the named
/// scope (or both).
fn candidates(opts: &Options, root: &Path) -> Result<Vec<(PathBuf, bool)>> {
    let chosen: Vec<agents::Agent> = if opts.agents.is_empty() {
        agents::all()
    } else {
        opts.agents
            .iter()
            .map(|name| {
                agents::find(name)
                    .ok_or_else(|| anyhow!("unknown agent '{name}' (see `son init --list-agents`)"))
            })
            .collect::<Result<_>>()?
    };
    let scopes: &[Scope] = match opts.scope {
        Some(Scope::Global) => &[Scope::Global],
        Some(Scope::Project) => &[Scope::Project],
        None => &[Scope::Global, Scope::Project],
    };
    let mut files: Vec<(PathBuf, bool)> = Vec::new();
    for agent in &chosen {
        for scope in scopes {
            for (path, is_skill) in [
                (agent.skill_file(*scope, root), true),
                (agent.todo_skill_file(*scope, root), true),
                (agent.rules_file(*scope, root), false),
            ] {
                if let Some(path) = path {
                    if !files.iter().any(|(known, _)| *known == path) {
                        files.push((path, is_skill));
                    }
                }
            }
        }
    }
    Ok(files)
}

/// Do what the plan says. The outcome word, or the reason it failed.
fn apply(found: &Found) -> std::result::Result<&'static str, String> {
    let fail = |err: anyhow::Error| format!("{err:#}");
    match &found.plan {
        Plan::DeleteSkill | Plan::DeleteRules => {
            std::fs::remove_file(&found.path)
                .with_context(|| format!("cannot delete {}", found.path.display()))
                .map_err(fail)?;
            if found.is_skill {
                // The `son` folder of the skill, when nothing else is in it.
                if let Some(dir) = found.path.parent() {
                    let _ = std::fs::remove_dir(dir);
                }
            }
            Ok("removed")
        }
        Plan::CutBlock(rest) => {
            instructions::replace_file(&found.path, rest).map_err(fail)?;
            Ok("block removed")
        }
        Plan::Leave(reason) => Err(reason.clone()),
    }
}

fn would(plan: &Plan) -> std::result::Result<&'static str, String> {
    match plan {
        Plan::DeleteSkill | Plan::DeleteRules => Ok("would remove"),
        Plan::CutBlock(_) => Ok("would remove block"),
        Plan::Leave(reason) => Err(reason.clone()),
    }
}

pub fn run(database: &Path, opts: &Options) -> Result<()> {
    let root = std::env::current_dir()
        .map(|cwd| project::root(&cwd))
        .context("cannot read the current directory")?;
    let found: Vec<Found> = candidates(opts, &root)?
        .into_iter()
        .filter_map(|(path, is_skill)| {
            plan(&path, is_skill).map(|plan| Found {
                path,
                is_skill,
                plan,
            })
        })
        .collect();

    let ask = !opts.yes && !opts.dry_run && !found.is_empty();
    if ask {
        if !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
            bail!(
                "son uninstall asks before it removes files, and this is not a terminal. \
                 Add --yes to remove them, or --dry-run to list them and change nothing"
            );
        }
        eprintln!("son uninstall will change these files:");
        for file in &found {
            let what = match would(&file.plan) {
                Ok(what) => what.replace("would ", ""),
                Err(_) => "leave as it is (see below)".to_string(),
            };
            eprintln!("  {:<13} {}", what, short_path(&file.path));
        }
        let go = dialoguer::Confirm::with_theme(&dialoguer::theme::ColorfulTheme::default())
            .with_prompt("Remove son from these files? The database is not touched")
            .default(false)
            .interact()
            .context("the question was cancelled")?;
        if !go {
            bail!("nothing was changed");
        }
    }

    let results: Vec<(&Found, std::result::Result<&'static str, String>)> = found
        .iter()
        .map(|file| {
            let result = if opts.dry_run {
                would(&file.plan)
            } else {
                apply(file)
            };
            (file, result)
        })
        .collect();
    let failed: Vec<String> = results
        .iter()
        .filter_map(|(file, result)| {
            result
                .as_ref()
                .err()
                .map(|reason| format!("{}: {reason}", short_path(&file.path)))
        })
        .collect();

    let database_note = format!(
        "The database was not touched: {}. It holds your memory. To delete that too, delete the file by hand",
        database.display()
    );
    let binary_note = "The `son` program itself is still installed. Remove it with the tool you \
                       installed it with (for example `brew uninstall son`), or delete the file";
    if opts.json {
        let files: Vec<Value> = results
            .iter()
            .map(|(file, result)| {
                let mut value = json!({
                    "path": file.path.display().to_string(),
                    "kind": if file.is_skill { "skill" } else { "rules" },
                });
                match result {
                    Ok(outcome) => value["result"] = json!(outcome.replace(' ', "_")),
                    Err(reason) => {
                        value["result"] = json!("left");
                        value["reason"] = json!(reason);
                    }
                }
                value
            })
            .collect();
        render::print_json(&json!({
            "dry_run": opts.dry_run,
            "files": files,
            "database": database.display().to_string(),
        }));
    } else {
        if results.is_empty() {
            println!("No son skill file and no son block was found. Nothing was changed.");
        }
        for (file, result) in &results {
            match result {
                Ok(outcome) => println!("  {:<18} {}", outcome, short_path(&file.path)),
                Err(reason) => println!("  {:<18} {} ({reason})", "LEFT", short_path(&file.path)),
            }
        }
        if opts.dry_run {
            println!("\nThis was a dry run: nothing was changed.");
        }
        println!("\n{database_note}.\n{binary_note}.");
    }
    if !failed.is_empty() {
        return Err(coded(
            "uninstall_failed",
            format!(
                "{} file(s) were left as they are: {}",
                failed.len(),
                failed.join("; ")
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instructions::{block, find_block};

    fn cut(text: &str) -> Option<String> {
        let range = find_block(text).unwrap().unwrap();
        without_block(text, range)
    }

    #[test]
    fn the_text_around_the_block_stays() {
        let block = block();
        assert_eq!(cut(&block), None);
        assert_eq!(cut(&format!("\n\n{block}\n  \n")), None);
        assert_eq!(
            cut(&format!("# Mine\n\nBe brief.\n\n{block}")).unwrap(),
            "# Mine\n\nBe brief.\n"
        );
        assert_eq!(cut(&format!("{block}\n## After\n")).unwrap(), "## After\n");
        assert_eq!(
            cut(&format!("# Mine\n\n{block}\n## After\ntext")).unwrap(),
            "# Mine\n\n## After\ntext\n"
        );
    }

    #[test]
    fn init_then_uninstall_gives_the_file_back() {
        // What `son init` makes of a file, cut again, is the file.
        let mine = "# Mine\n\nBe brief.\n";
        let installed = format!("{}\n\n{}", mine.trim_end(), block());
        assert_eq!(cut(&installed).unwrap(), mine);
    }
}
