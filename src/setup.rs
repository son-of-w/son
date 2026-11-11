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
}

/// What was done for one agent.
struct Installed {
    agent: Agent,
    skill: Step,
    rules: Step,
}

enum Step {
    Done(PathBuf, Outcome),
    Failed(PathBuf, String),
    /// Nothing to write, and why.
    Skipped(&'static str),
}

/// `~/x` for a path under the home directory.
fn short(path: &Path) -> String {
    if let Some(home) = agents::home() {
        if let Ok(rest) = path.strip_prefix(&home) {
            return format!("~/{}", rest.display());
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
    let picked = MultiSelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Which agents should use son? (space to select, enter to confirm)")
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

fn install(agent: Agent, scope: Scope, root: &Path, skill_only: bool) -> Installed {
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
        Step::Skipped(reason) => json!({ "result": "skipped", "reason": reason }),
    }
}

fn step_line(label: &str, step: &Step) -> String {
    match step {
        Step::Done(path, outcome) => {
            format!("  {label:<6} {:<9} {}", outcome.as_str(), short(path))
        }
        Step::Failed(path, reason) => format!("  {label:<6} FAILED    {} ({reason})", short(path)),
        Step::Skipped(reason) => format!("  {label:<6} none      {reason}"),
    }
}

pub fn run(database: &Path, opts: &Options) -> Result<()> {
    if !opts.json {
        println!("database ready: {}", database.display());
    }
    let root = project_root()?;
    let chosen = choose_agents(opts)?;
    let scope = choose_scope(opts, &root)?;

    let done: Vec<Installed> = chosen
        .into_iter()
        .map(|agent| install(agent, scope, &root, opts.skill_only))
        .collect();
    let failed = done
        .iter()
        .filter(|i| matches!(i.skill, Step::Failed(..)) || matches!(i.rules, Step::Failed(..)))
        .count();

    if opts.json {
        let installs: Vec<Value> = done
            .iter()
            .map(|i| {
                json!({
                    "agent": i.agent.name,
                    "skill": step_json(&i.skill),
                    "rules": step_json(&i.rules),
                })
            })
            .collect();
        render::print_json(&json!({
            "database": database.display().to_string(),
            "scope": scope.as_str(),
            "project_root": (scope == Scope::Project).then(|| root.display().to_string()),
            "installs": installs,
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
        }
        println!(
            "\nRun `son init` again at any time to add agents or bring these files up to date."
        );
    }
    if failed > 0 {
        bail!("{failed} agent(s) could not be set up fully; see above");
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
