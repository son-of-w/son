//! `son init`: choose the agents and the scope, then write the skill and
//! the instruction block for each agent.

use crate::agents::{self, Agent, Scope};
use crate::instructions::{self, Outcome};
use crate::project;
use crate::render;
use anyhow::{anyhow, bail, Context, Result};
use dialoguer::theme::ColorfulTheme;
use dialoguer::{MultiSelect, Select};
use serde_json::{json, Value};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

pub struct Options {
    /// Agent names from `--agent`. Empty means ask, or take the detected ones.
    pub agents: Vec<String>,
    /// From `--scope` or `--global`. `None` means ask, or global with `--yes`.
    pub scope: Option<Scope>,
    /// Do not ask anything.
    pub yes: bool,
    /// Leave instruction files alone; write the skill only.
    pub skill_only: bool,
    pub json: bool,
    /// Install the todo skill too. A plain init only brings one that is
    /// there up to date.
    pub todo_skill: bool,
    /// From `--todo-skill-name`.
    pub todo_skill_name: Option<String>,
    /// From `--overwrite-todo-skill`: replace a skill of that name that
    /// is not son's.
    pub overwrite_todo_skill: bool,
    /// From `--files`: with the project scope, keep the collection of the
    /// project as files in its folder, and do not ask.
    pub files: bool,
}

/// What was done for one agent.
struct Installed {
    agent: Agent,
    skill: Step,
    rules: Step,
    /// The todo skill. `None` when it was not asked for and is not there.
    todo: Option<Step>,
}

/// What to do when a skill of the wanted name is there and is not son's.
#[derive(Clone)]
enum Taken {
    /// Leave it, and say so.
    Leave,
    /// Replace it.
    Overwrite,
    /// Use this name for the todo skill.
    Rename(String),
}

/// Ask what to do about a skill named `name` that is not son's.
fn ask_taken(name: &str, path: &Path) -> Result<Taken> {
    let items = [
        "Give the son todo skill another name".to_string(),
        format!("Overwrite {}", short(path)),
        "Do not install the todo skill".to_string(),
    ];
    let picked = Select::with_theme(&ColorfulTheme::default())
        .with_prompt(format!(
            "A skill named '{name}' is there already and is not son's. What now?"
        ))
        .items(&items)
        .default(0)
        .interact()
        .context("the choice was cancelled")?;
    Ok(match picked {
        0 => {
            let new: String = dialoguer::Input::with_theme(&ColorfulTheme::default())
                .with_prompt("Name for the son todo skill")
                .default("son-todo".to_string())
                .validate_with(|text: &String| {
                    if instructions::is_skill_name(text.trim()) {
                        Ok(())
                    } else {
                        Err("use lowercase letters, digits and -")
                    }
                })
                .interact_text()
                .context("the name was cancelled")?;
            Taken::Rename(new.trim().to_string())
        }
        1 => Taken::Overwrite,
        _ => Taken::Leave,
    })
}

/// Install or update the todo skill for one agent. `taken` is the answer
/// to "a skill of that name is there", once it is known; it is asked for
/// one time and then used for every agent.
fn todo_step(
    agent: &Agent,
    scope: Scope,
    root: &Path,
    opts: &Options,
    taken: &mut Option<Taken>,
) -> Option<Step> {
    let skills = agent.skills_dir(scope, root)?;
    let existing = instructions::find_todo_skill(&skills);
    let wanted = opts.todo_skill_name.clone();
    // One that son wrote is there: bring it up to date where it is,
    // unless another name is asked for.
    if let Some(path) = &existing {
        let name = instructions::skill_name_of(path);
        if wanted.as_deref().is_none_or(|new| new == name) {
            return Some(match instructions::install_todo_skill(path, &name) {
                Ok(outcome) => Step::Done(path.clone(), outcome),
                Err(err) => Step::Failed(path.clone(), format!("{err:#}")),
            });
        }
    } else if !opts.todo_skill {
        return None;
    }
    let mut name = wanted.unwrap_or_else(|| instructions::TODO_SKILL_NAME.to_string());
    let mut path = skills.join(&name).join("SKILL.md");
    let is_ours = |path: &Path| {
        std::fs::read_to_string(path).is_ok_and(|text| text.contains(instructions::TODO_SKILL_MARK))
    };
    if path.exists() && !is_ours(&path) && !opts.overwrite_todo_skill {
        if taken.is_none() {
            *taken = Some(if !opts.yes && can_ask() {
                match ask_taken(&name, &path) {
                    Ok(answer) => answer,
                    Err(err) => return Some(Step::Failed(path, format!("{err:#}"))),
                }
            } else {
                Taken::Leave
            });
        }
        match taken.clone().unwrap_or(Taken::Leave) {
            Taken::Overwrite => {}
            Taken::Rename(new) => {
                name = new;
                path = skills.join(&name).join("SKILL.md");
                if path.exists() && !is_ours(&path) {
                    return Some(Step::Left(
                        path,
                        "a skill of that name is there too and is not son's",
                    ));
                }
            }
            Taken::Leave => {
                return Some(Step::Left(
                    path,
                    "a skill of that name is there and is not son's; it was left as it is. \
                     Give --todo-skill-name <name> for another name, or --overwrite-todo-skill",
                ));
            }
        }
    }
    let step = match instructions::install_todo_skill(&path, &name) {
        Ok(outcome) => Step::Done(path.clone(), outcome),
        Err(err) => return Some(Step::Failed(path, format!("{err:#}"))),
    };
    // The todo skill has a new name: the one under the old name goes.
    if let Some(old) = existing.filter(|old| *old != path) {
        let _ = std::fs::remove_file(&old);
        if let Some(dir) = old.parent() {
            let _ = std::fs::remove_dir(dir);
        }
    }
    Some(step)
}

enum Step {
    Done(PathBuf, Outcome),
    Failed(PathBuf, String),
    /// A file that is there was left as it is, and why. Not a failure.
    Left(PathBuf, &'static str),
    /// Nothing to write, and why.
    Skipped(&'static str),
}

/// `~/x` for a path under the home directory.
fn short(path: &Path) -> String {
    if let Some(home) = agents::home() {
        if let Ok(rest) = path.strip_prefix(&home) {
            // Forward slashes on every system, so the output reads the same.
            return format!("~/{}", rest.display()).replace('\\', "/");
        }
    }
    path.display().to_string()
}

fn can_ask() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

/// The folder a project-scope install goes into: the git root, else here.
fn project_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("cannot read the current directory")?;
    Ok(project::root(&cwd))
}

/// `son init --list-agents`.
pub fn list_agents(json: bool) {
    let all = agents::all();
    if json {
        let list: Vec<Value> = all
            .iter()
            .map(|agent| {
                json!({
                    "name": agent.name,
                    "display": agent.display,
                    "detected": agent.detected(),
                    "project_skills": agent.project_skills,
                    "global_skills": agent.global_skills.path().map(|p| p.display().to_string()),
                    "project_rules": agent.project_rules,
                    "global_rules": agent.global_rules.and_then(|d| d.path()).map(|p| p.display().to_string()),
                })
            })
            .collect();
        render::print_json(&Value::Array(list));
        return;
    }
    let detected = all.iter().filter(|agent| agent.detected()).count();
    println!(
        "# Agents ({} known, {detected} detected on this machine)",
        all.len()
    );
    for agent in &all {
        println!(
            "- {:<18} {}{}",
            agent.name,
            agent.display,
            if agent.detected() { " (detected)" } else { "" }
        );
    }
}

fn choose_agents(opts: &Options) -> Result<Vec<Agent>> {
    if !opts.agents.is_empty() {
        let mut chosen: Vec<Agent> = Vec::new();
        for name in &opts.agents {
            let agent = agents::find(name)
                .ok_or_else(|| anyhow!("unknown agent '{name}' (see `son init --list-agents`)"))?;
            if !chosen.iter().any(|known| known.name == agent.name) {
                chosen.push(agent);
            }
        }
        return Ok(chosen);
    }

    let all = agents::all();
    let detected: Vec<Agent> = all.iter().filter(|a| a.detected()).cloned().collect();
    if opts.yes {
        if detected.is_empty() {
            bail!("no agent was detected on this machine; name one with --agent (see `son init --list-agents`)");
        }
        return Ok(detected);
    }
    if !can_ask() {
        let names: Vec<&str> = detected.iter().map(|a| a.name).collect();
        bail!(
            "son init asks questions, and this is not a terminal. Say what to set up instead:\n  \
             son init --agent <names> --scope <global|project>\n  \
             son init --yes            (the detected agents, global)\n\
             detected agents: {}",
            if names.is_empty() {
                "none".to_string()
            } else {
                names.join(", ")
            }
        );
    }

    let labels: Vec<String> = all
        .iter()
        .map(|a| {
            format!(
                "{}{}",
                a.display,
                if a.detected() { " (detected)" } else { "" }
            )
        })
        .collect();
    let checked: Vec<bool> = all.iter().map(Agent::detected).collect();
    // The prompt must fit on one line of the terminal, with the page
    // number that is added to it: the list is drawn again at each key
    // press, and a prompt that wraps is then left behind once for each
    // press. So the keys are named on a line of their own, printed once.
    eprintln!("Space selects, Enter confirms, the arrow keys move.");
    let picked = MultiSelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Which agents should use son?")
        .items(&labels)
        .defaults(&checked)
        .max_length(14)
        .interact()
        .context("the agent choice was cancelled")?;
    if picked.is_empty() {
        bail!("no agent was selected; nothing was changed");
    }
    Ok(picked.into_iter().map(|i| all[i].clone()).collect())
}

fn choose_scope(opts: &Options, root: &Path) -> Result<Scope> {
    if let Some(scope) = opts.scope {
        return Ok(scope);
    }
    if opts.yes {
        return Ok(Scope::Global);
    }
    if !can_ask() {
        bail!("say where to install with --scope global or --scope project");
    }
    let items = [
        "Global: every project on this machine".to_string(),
        format!("This project only: {}", short(root)),
    ];
    let picked = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Where should son be set up?")
        .items(&items)
        .default(0)
        .interact()
        .context("the scope choice was cancelled")?;
    Ok(if picked == 0 {
        Scope::Global
    } else {
        Scope::Project
    })
}

fn step(
    path: Option<PathBuf>,
    missing: &'static str,
    write: impl Fn(&Path) -> Result<Outcome>,
) -> Step {
    match path {
        None => Step::Skipped(missing),
        Some(path) => match write(&path) {
            Ok(outcome) => Step::Done(path, outcome),
            Err(err) => Step::Failed(path, format!("{err:#}")),
        },
    }
}

fn install(
    agent: Agent,
    scope: Scope,
    root: &Path,
    opts: &Options,
    taken: &mut Option<Taken>,
) -> Installed {
    let skill_only = opts.skill_only;
    let todo = todo_step(&agent, scope, root, opts, taken);
    let skill = step(
        agent.skill_file(scope, root),
        "the home directory is not known",
        instructions::install_skill,
    );
    let rules = if skill_only {
        Step::Skipped("--skill-only was given")
    } else {
        step(
            agent.rules_file(scope, root),
            "this agent has no global instruction file; it will use son through the skill",
            instructions::install,
        )
    };
    Installed {
        agent,
        skill,
        rules,
        todo,
    }
}

fn step_json(step: &Step) -> Value {
    match step {
        Step::Done(path, outcome) => {
            json!({ "path": path.display().to_string(), "result": outcome.as_str() })
        }
        Step::Failed(path, reason) => {
            json!({ "path": path.display().to_string(), "result": "failed", "reason": reason })
        }
        Step::Left(path, reason) => {
            json!({ "path": path.display().to_string(), "result": "left", "reason": reason })
        }
        Step::Skipped(reason) => json!({ "result": "skipped", "reason": reason }),
    }
}

fn step_line(label: &str, step: &Step) -> String {
    match step {
        Step::Done(path, outcome) => {
            format!("  {label:<6} {:<9} {}", outcome.as_str(), short(path))
        }
        Step::Failed(path, reason) => format!("  {label:<6} FAILED    {} ({reason})", short(path)),
        Step::Left(path, reason) => format!("  {label:<6} LEFT      {} ({reason})", short(path)),
        Step::Skipped(reason) => format!("  {label:<6} none      {reason}"),
    }
}

/// With the project scope: should the collection of the project be kept
/// as files in its folder (see `files.rs`)? `--files` says yes. In a
/// terminal the user is asked, unless it is on already. Without a
/// terminal, and with `--yes`, the answer is no: nothing is shared that
/// was not asked for.
fn choose_files(opts: &Options, scope: Scope, root: &Path) -> Result<bool> {
    if scope != Scope::Project {
        if opts.files {
            bail!("--files needs --scope project: files are kept in a project folder");
        }
        return Ok(false);
    }
    if opts.files {
        return Ok(true);
    }
    if opts.yes || !can_ask() || project::shared(root).is_some() {
        return Ok(false);
    }
    // A short prompt, for the same reason as in `choose_agents`.
    eprintln!(
        "son can keep this project's memory as files in {}, to commit and share through git.",
        short(&root.join(crate::files::DEFAULT_FOLDER))
    );
    dialoguer::Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt("Keep the memory as files?")
        .default(false)
        .interact()
        .context("the question about files was cancelled")
}

pub fn run(
    database: &Path,
    opened: &crate::db::Opened,
    share: &dyn Fn(&Path) -> Result<crate::files::Status>,
    opts: &Options,
) -> Result<()> {
    if !opts.json {
        println!("database ready: {}", database.display());
        println!("{}", opened.line());
    }
    if let Some(name) = &opts.todo_skill_name {
        if !instructions::is_skill_name(name) {
            return Err(crate::error::coded(
                "invalid_argument",
                format!(
                    "'{name}' cannot be the name of the todo skill; use lowercase letters, digits and -, and not 'son'"
                ),
            ));
        }
    }
    let root = project_root()?;
    let chosen = choose_agents(opts)?;
    let scope = choose_scope(opts, &root)?;
    let files = choose_files(opts, scope, &root)?;

    // Look at every instruction file before any file is written. One file
    // with marks that do not make one block stops the whole run.
    if !opts.skill_only {
        let faults: Vec<String> = chosen
            .iter()
            .filter_map(|agent| agent.rules_file(scope, &root))
            .filter_map(|path| {
                instructions::check_file(&path)
                    .err()
                    .map(|fault| format!("  {}: {fault}", short(&path)))
            })
            .collect();
        if !faults.is_empty() {
            return Err(crate::error::coded(
                "broken_block",
                format!(
                    "no file was changed: {} instruction file(s) hold a son block that is not whole
{}
                     son does not repair these, because it cannot tell which text is yours.                      In each file, {}. `son doctor` shows the same check.",
                    faults.len(),
                    faults.join("\n"),
                    instructions::REPAIR
                ),
            ));
        }
    }

    let mut taken: Option<Taken> = None;
    let done: Vec<Installed> = chosen
        .into_iter()
        .map(|agent| install(agent, scope, &root, opts, &mut taken))
        .collect();
    let failed = done
        .iter()
        .filter(|i| {
            matches!(i.skill, Step::Failed(..))
                || matches!(i.rules, Step::Failed(..))
                || matches!(i.todo, Some(Step::Failed(..)))
        })
        .count();

    // After the agents: a file that could not be written is reported
    // below, and the shared folder is still made.
    let shared = if files { Some(share(&root)) } else { None };
    if opts.json {
        let installs: Vec<Value> = done
            .iter()
            .map(|i| {
                let mut value = json!({
                    "agent": i.agent.name,
                    "skill": step_json(&i.skill),
                    "rules": step_json(&i.rules),
                });
                if let Some(todo) = &i.todo {
                    value["todo_skill"] = step_json(todo);
                }
                value
            })
            .collect();
        render::print_json(&json!({
            "database": database.display().to_string(),
            "database_state": opened.json(),
            "scope": scope.as_str(),
            "project_root": (scope == Scope::Project).then(|| root.display().to_string()),
            "installs": installs,
            "files": shared.as_ref().and_then(|shared| shared.as_ref().ok()),
        }));
    } else {
        match scope {
            Scope::Global => println!("scope: global (every project)"),
            Scope::Project => println!("scope: this project ({})", short(&root)),
        }
        for installed in &done {
            println!("{}", installed.agent.display);
            println!("{}", step_line("skill", &installed.skill));
            println!("{}", step_line("rules", &installed.rules));
            if let Some(todo) = &installed.todo {
                println!("{}", step_line("todo", todo));
            }
        }
        if let Some(Ok(status)) = &shared {
            println!(
                "files: on for collection '{}' in {} ({} node file(s)). \
                 Commit the .son file and the folder to share them through git.",
                status.collection.as_deref().unwrap_or("?"),
                status.folder.as_deref().unwrap_or("?"),
                status.nodes
            );
        }
        println!(
            "\nRun `son init` again at any time to add agents or bring these files up to date."
        );
        // The memory starts empty. Say so when there is something to put in.
        let own = crate::seed::candidates().map(|f| f.len()).unwrap_or(0);
        if own > 0 {
            println!(
                "You already have {own} file(s) of instructions or notes for your agents. \
                 To bring what is in them into son, ask your agent to run `son seed`."
            );
        }
    }
    if let Some(Err(err)) = shared {
        return Err(err.context("the agents are set up, but the files were not turned on"));
    }
    if failed > 0 {
        // With --json the report above went to stdout as one valid object;
        // this error goes to stderr and names what failed.
        let reasons: Vec<String> = done
            .iter()
            .flat_map(|i| [Some(&i.skill), Some(&i.rules), i.todo.as_ref()])
            .flatten()
            .filter_map(|step| match step {
                Step::Failed(path, reason) => Some(format!("{}: {reason}", short(path))),
                _ => None,
            })
            .collect();
        return Err(crate::error::coded(
            "install_failed",
            format!(
                "{failed} agent(s) could not be set up fully: {}",
                reasons.join("; ")
            ),
        ));
    }
    Ok(())
}

/// The files that show `son` is set up, global and for the current project.
pub fn installed_files() -> Vec<PathBuf> {
    let Ok(root) = project_root() else {
        return Vec::new();
    };
    agents::candidate_files(&root)
        .into_iter()
        .filter(|(path, _)| {
            std::fs::read_to_string(path).is_ok_and(|text| instructions::is_installed_in(&text))
        })
        .map(|(path, _)| path)
        .collect()
}

/// For display: `~/.claude/CLAUDE.md`.
pub fn short_path(path: &Path) -> String {
    short(path)
}
