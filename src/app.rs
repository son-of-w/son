//! Runs one command.

use crate::analyze::Analyzer;
use crate::cli::{Cli, Command, NodeType, Relation, SynonymCommand, TodoCommand};
use crate::render::{self, Context as ProjectContext};
use crate::search;
use crate::store::{self, Meta, NewNode, Node, TODO_STATUSES};
use crate::{claude_md, db, project, transfer};
use anyhow::{anyhow, bail, Context, Result};
use clap::ValueEnum;
use rusqlite::Connection;
use serde_json::json;
use std::io::Read;

const CONTEXT_SECTION_LIMIT: usize = 10;
const CONTEXT_NOTES_LIMIT: usize = 5;

struct App {
    conn: Connection,
    an: Analyzer,
    meta: Meta,
    json: bool,
}

pub fn run(cli: Cli) -> Result<()> {
    if let Command::Init {
        print_block: true, ..
    } = cli.command
    {
        print!("{}", claude_md::block());
        return Ok(());
    }
    let path = db::path()?;
    let app = App {
        conn: db::open(&path)?,
        an: Analyzer::new(),
        meta: Meta::from_env(),
        json: cli.json,
    };

    match cli.command {
        Command::Init { no_claude_md, .. } => app.init(&path, no_claude_md),
        Command::Remember {
            kind,
            title,
            body,
            project,
            tag,
            link,
        } => {
            let body = read_body(body)?;
            app.remember(kind, &title, &body, project.as_deref(), &tag, &link)
        }
        Command::Recall {
            query,
            kind,
            project,
            limit,
            hops,
        } => app.recall(&query.join(" "), kind, project.as_deref(), limit, hops),
        Command::Show { id } => app.show(&id),
        Command::Edit {
            id,
            title,
            body,
            kind,
            status,
            project,
            tag,
        } => {
            let body = body.map(|b| read_body(Some(b))).transpose()?;
            app.edit(&id, title, body, kind, status, project, tag)
        }
        Command::Drop { id, purge } => app.drop_node(&id, purge),
        Command::Restore { id } => app.restore(&id),
        Command::Todo { command } => match command {
            TodoCommand::Add {
                title,
                body,
                project,
                tag,
            } => {
                let body = read_body(body)?;
                app.remember(NodeType::Todo, &title, &body, project.as_deref(), &tag, &[])
            }
            TodoCommand::List { project, all, done } => {
                app.todo_list(project.as_deref(), all, done)
            }
            TodoCommand::Done { id } => app.todo_done(&id),
        },
        Command::Link { from, relation, to } => app.link(&from, relation, &to, true),
        Command::Unlink { from, relation, to } => app.link(&from, relation, &to, false),
        Command::Projects => {
            let rows = store::list_projects(&app.conn)?;
            render::projects(&rows, &current_project_name()?, app.json);
            Ok(())
        }
        Command::Synonym { command } => app.synonym(command),
        Command::Export { project } => app.export(project.as_deref()),
        Command::Import { file } => app.import(&file),
        Command::Context { project } => app.context(project.as_deref()),
        Command::Reindex => {
            let count = store::write(&app.conn, || store::reindex_all(&app.conn, &app.an))?;
            if app.json {
                render::print_json(&json!({ "reindexed": count }));
            } else {
                println!("reindexed {count} nodes");
            }
            Ok(())
        }
    }
}

/// `--body` as given, `--body -` from stdin, no `--body` as empty.
fn read_body(arg: Option<String>) -> Result<String> {
    match arg.as_deref() {
        None => Ok(String::new()),
        Some("-") => {
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .context("cannot read the body from stdin")?;
            Ok(text.trim_end().to_string())
        }
        Some(text) => Ok(text.to_string()),
    }
}

fn current_project_name() -> Result<String> {
    let cwd = std::env::current_dir().context("cannot read the current directory")?;
    Ok(project::detect(&cwd))
}

/// The project named by `--project`, else the detected one.
fn project_name(flag: Option<&str>) -> Result<String> {
    match flag.map(str::trim) {
        Some("") => bail!("--project needs a name"),
        Some(name) => Ok(name.to_string()),
        None => current_project_name(),
    }
}

/// `<id>:<relation>` from `--link`.
fn parse_link(text: &str) -> Result<(&str, Relation)> {
    let (id, relation) = text
        .rsplit_once(':')
        .ok_or_else(|| anyhow!("--link takes <id>:<relation>, for example a1b2c3d4:supersedes"))?;
    let relation = Relation::from_str(relation.trim(), true).map_err(|_| {
        anyhow!(
            "unknown relation '{relation}' (use one of: {})",
            store::RELATIONS.join(", ")
        )
    })?;
    Ok((id, relation))
}

impl App {
    fn init(&self, path: &std::path::Path, no_claude_md: bool) -> Result<()> {
        let mut report = json!({ "database": path.display().to_string() });
        if !self.json {
            println!("database ready: {}", path.display());
        }
        if no_claude_md {
            if self.json {
                render::print_json(&report);
            }
            return Ok(());
        }

        let installed = claude_md::target()
            .ok_or_else(|| anyhow!("HOME is not set"))
            .and_then(|target| claude_md::install(&target).map(|outcome| (target, outcome)));
        match installed {
            Ok((target, outcome)) => {
                report["claude_md"] = json!({
                    "path": target.display().to_string(),
                    "result": outcome.as_str(),
                });
                if !self.json {
                    println!("CLAUDE.md block {}: {}", outcome.as_str(), target.display());
                }
            }
            Err(err) => {
                report["claude_md"] = json!({
                    "result": "not written",
                    "reason": format!("{err:#}"),
                    "block": claude_md::block(),
                });
                if !self.json {
                    println!("could not write CLAUDE.md ({err:#}).");
                    println!("Add this block to ~/.claude/CLAUDE.md yourself:\n");
                    print!("{}", claude_md::block());
                }
            }
        }
        if self.json {
            render::print_json(&report);
        }
        Ok(())
    }

    fn remember(
        &self,
        kind: NodeType,
        title: &str,
        body: &str,
        project: Option<&str>,
        tags: &[String],
        links: &[String],
    ) -> Result<()> {
        let title = title.trim();
        if title.is_empty() {
            bail!("the title is empty");
        }
        let id = store::write(&self.conn, || {
            let id = if kind == NodeType::Project {
                if project.is_some() {
                    bail!("a project cannot be inside another project");
                }
                if let Some(existing) = store::find_project(&self.conn, title)? {
                    bail!(
                        "project '{title}' already exists (`{}`); change it with `son edit`",
                        store::short_id(&existing)
                    );
                }
                self.insert(kind, title, body, None, None, tags)?
            } else {
                let project_id = store::ensure_project(
                    &self.conn,
                    &self.an,
                    &self.meta,
                    &project_name(project)?,
                )?;
                let status = (kind == NodeType::Todo).then_some("open");
                self.insert(kind, title, body, status, Some(&project_id), tags)?
            };
            for link in links {
                let (target, relation) = parse_link(link)?;
                let to = store::resolve_id(&self.conn, target)?;
                store::link(&self.conn, &id, relation.as_str(), &to)?;
            }
            Ok(id)
        })?;
        render::saved("saved", &store::get_node(&self.conn, &id)?, self.json);
        Ok(())
    }

    fn insert(
        &self,
        kind: NodeType,
        title: &str,
        body: &str,
        status: Option<&str>,
        project_id: Option<&str>,
        tags: &[String],
    ) -> Result<String> {
        store::insert_node(
            &self.conn,
            &self.an,
            &self.meta,
            &NewNode {
                kind: kind.as_str(),
                title,
                body,
                status,
                project_id,
                tags,
            },
        )
    }

    fn recall(
        &self,
        terms: &str,
        kind: Option<NodeType>,
        project: Option<&str>,
        limit: usize,
        hops: u8,
    ) -> Result<()> {
        let name = project_name(project)?;
        let found = store::find_project(&self.conn, &name)?;
        if project.is_some() && found.is_none() {
            bail!("no project named '{name}' (see `son projects`)");
        }
        let result = search::recall(
            &self.conn,
            &self.an,
            terms,
            &search::Options {
                kind: kind.map(NodeType::as_str),
                only_project: if project.is_some() {
                    found.as_deref()
                } else {
                    None
                },
                current_project: found.as_deref(),
                limit,
                hops,
            },
        )?;
        render::recall(terms, &name, &result, self.json);
        Ok(())
    }

    fn show(&self, id: &str) -> Result<()> {
        let id = store::resolve_id(&self.conn, id)?;
        let node = store::get_node(&self.conn, &id)?;
        let edges = store::edges_of(&self.conn, &id)?;
        let newer = store::superseded_by(&self.conn, &id)?;
        render::show(&node, &edges, newer.as_ref(), self.json);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn edit(
        &self,
        id: &str,
        title: Option<String>,
        body: Option<String>,
        kind: Option<NodeType>,
        status: Option<String>,
        project: Option<String>,
        tags: Option<Vec<String>>,
    ) -> Result<()> {
        if title.is_none()
            && body.is_none()
            && kind.is_none()
            && status.is_none()
            && project.is_none()
            && tags.is_none()
        {
            bail!("nothing to change; give --title, --body, --type, --status, --project or --tag");
        }
        let id = store::resolve_id(&self.conn, id)?;
        store::write(&self.conn, || {
            let mut node = store::get_node(&self.conn, &id)?;
            let is_project = node.kind == "project";

            if let Some(title) = title {
                let title = title.trim().to_string();
                if title.is_empty() {
                    bail!("the title is empty");
                }
                if is_project {
                    if let Some(other) = store::find_project(&self.conn, &title)? {
                        if other != node.id {
                            bail!("another project is already called '{title}'");
                        }
                    }
                }
                node.title = title;
            }
            if let Some(body) = body {
                node.body = body;
            }
            if let Some(kind) = kind {
                if (kind == NodeType::Project) != is_project {
                    bail!("a project cannot become another type, and other types cannot become projects");
                }
                node.kind = kind.as_str().to_string();
                if kind == NodeType::Todo {
                    node.status.get_or_insert_with(|| "open".to_string());
                } else if status.is_none() {
                    node.status = None;
                }
            }
            if let Some(status) = status {
                let status = status.trim().to_lowercase();
                node.status = (!status.is_empty()).then_some(status);
            }
            if node.kind == "todo" {
                let status = node.status.as_deref().unwrap_or("");
                if !TODO_STATUSES.contains(&status) {
                    bail!("a todo's status is one of: {}", TODO_STATUSES.join(", "));
                }
            }
            if let Some(project) = project {
                if is_project {
                    bail!("a project cannot be inside another project");
                }
                let name = project_name(Some(&project))?;
                node.project_id = Some(store::ensure_project(
                    &self.conn, &self.an, &self.meta, &name,
                )?);
            }
            if let Some(tags) = tags {
                node.tags = tags;
            }
            store::update_node(&self.conn, &self.an, &node)
        })?;
        render::saved("updated", &store::get_node(&self.conn, &id)?, self.json);
        Ok(())
    }

    fn drop_node(&self, id: &str, purge: bool) -> Result<()> {
        let id = store::resolve_id(&self.conn, id)?;
        let node = store::get_node(&self.conn, &id)?;
        store::write(&self.conn, || {
            if purge {
                return store::purge(&self.conn, &id);
            }
            if node.deleted_at.is_some() {
                bail!("`{}` is already dropped", node.short());
            }
            if node.kind == "project" {
                let children = store::live_children(&self.conn, &id)?;
                if children > 0 {
                    bail!(
                        "project '{}' still holds {children} node(s); drop or move them first",
                        node.title
                    );
                }
            }
            store::soft_drop(&self.conn, &self.an, &id)
        })?;
        if self.json {
            render::print_json(&json!({
                "id": node.id,
                "short_id": node.short(),
                "result": if purge { "purged" } else { "dropped" },
            }));
        } else if purge {
            println!("purged {}", render::line(&node));
        } else {
            println!(
                "dropped {} (undo with `son restore {}`)",
                render::line(&node),
                node.short()
            );
        }
        Ok(())
    }

    fn restore(&self, id: &str) -> Result<()> {
        let id = store::resolve_id(&self.conn, id)?;
        let node = store::get_node(&self.conn, &id)?;
        if node.deleted_at.is_none() {
            bail!("`{}` is not dropped", node.short());
        }
        store::write(&self.conn, || {
            if node.kind == "project" && store::find_project(&self.conn, &node.title)?.is_some() {
                bail!("another project is now called '{}'", node.title);
            }
            store::restore(&self.conn, &self.an, &id)
        })?;
        render::saved("restored", &store::get_node(&self.conn, &id)?, self.json);
        Ok(())
    }

    fn todo_list(&self, project: Option<&str>, all: bool, done: bool) -> Result<()> {
        let (scope, project_id) = if all && project.is_none() {
            ("all projects".to_string(), None)
        } else {
            let name = project_name(project)?;
            match store::find_project(&self.conn, &name)? {
                Some(id) => (name, Some(id)),
                None => {
                    render::todos(&[], &name, self.json);
                    return Ok(());
                }
            }
        };
        let mut todos = store::list_nodes(&self.conn, project_id.as_deref(), Some("todo"), false)?;
        if !done {
            todos.retain(|t| t.status.as_deref() == Some("open"));
        }
        // Oldest first: a todo list reads top down.
        todos.sort_by(|a, b| a.id.cmp(&b.id));
        render::todos(&todos, &scope, self.json);
        Ok(())
    }

    fn todo_done(&self, id: &str) -> Result<()> {
        let id = store::resolve_id(&self.conn, id)?;
        let node = store::get_node(&self.conn, &id)?;
        if node.kind != "todo" {
            bail!("`{}` is a {}, not a todo", node.short(), node.kind);
        }
        store::write(&self.conn, || store::set_status(&self.conn, &id, "done"))?;
        render::saved("done", &store::get_node(&self.conn, &id)?, self.json);
        Ok(())
    }

    fn link(&self, from: &str, relation: Relation, to: &str, add: bool) -> Result<()> {
        let from = store::resolve_id(&self.conn, from)?;
        let to = store::resolve_id(&self.conn, to)?;
        let changed = store::write(&self.conn, || {
            if add {
                store::link(&self.conn, &from, relation.as_str(), &to)
            } else {
                store::unlink(&self.conn, &from, relation.as_str(), &to)
            }
        })?;
        let (from, to) = (
            store::get_node(&self.conn, &from)?,
            store::get_node(&self.conn, &to)?,
        );
        let result = match (add, changed) {
            (true, true) => "linked",
            (true, false) => "already linked",
            (false, true) => "unlinked",
            (false, false) => "was not linked",
        };
        if self.json {
            render::print_json(&json!({
                "result": result,
                "from": from.short(),
                "relation": relation.as_str(),
                "to": to.short(),
            }));
        } else {
            println!(
                "{result}: `{}` {} {} `{}` {}",
                from.short(),
                from.title,
                relation.as_str(),
                to.short(),
                to.title
            );
        }
        Ok(())
    }

    fn synonym(&self, command: SynonymCommand) -> Result<()> {
        match command {
            SynonymCommand::Add { term, canonical } => {
                store::write(&self.conn, || {
                    store::add_synonym(&self.conn, &term, &canonical)
                })?;
                let (term, canonical) =
                    (term.trim().to_lowercase(), canonical.trim().to_lowercase());
                if self.json {
                    render::print_json(&json!({ "term": term, "canonical": canonical }));
                } else {
                    println!("synonym saved: {term} -> {canonical}");
                }
            }
            SynonymCommand::Remove { term } => {
                let removed =
                    store::write(&self.conn, || store::remove_synonym(&self.conn, &term))?;
                if self.json {
                    render::print_json(&json!({ "term": term, "removed": removed }));
                } else if removed {
                    println!("synonym removed: {term}");
                } else {
                    println!("no synonym for '{term}'");
                }
            }
            SynonymCommand::List => {
                let mut pairs: Vec<(String, String)> =
                    store::synonyms(&self.conn)?.into_iter().collect();
                pairs.sort();
                if self.json {
                    let list: Vec<_> = pairs
                        .iter()
                        .map(|(term, canonical)| json!({ "term": term, "canonical": canonical }))
                        .collect();
                    render::print_json(&json!(list));
                } else {
                    for (term, canonical) in pairs {
                        println!("{term} -> {canonical}");
                    }
                }
            }
        }
        Ok(())
    }

    fn export(&self, project: Option<&str>) -> Result<()> {
        let project_id = match project {
            Some(name) => Some(
                store::find_project(&self.conn, name.trim())?
                    .ok_or_else(|| anyhow!("no project named '{name}' (see `son projects`)"))?,
            ),
            None => None,
        };
        let dump = transfer::export(&self.conn, project_id.as_deref())?;
        if self.json {
            println!("{}", serde_json::to_string_pretty(&dump)?);
        } else {
            print!("{}", transfer::markdown(&dump));
        }
        Ok(())
    }

    fn import(&self, file: &str) -> Result<()> {
        let text = if file == "-" {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text)?;
            text
        } else {
            std::fs::read_to_string(file).with_context(|| format!("cannot read {file}"))?
        };
        let dump: transfer::Dump = serde_json::from_str(&text)
            .context("this is not a son JSON export (make one with `son export --json`)")?;
        let stats = transfer::import(&self.conn, &self.an, &dump)?;
        if self.json {
            render::print_json(&serde_json::to_value(&stats)?);
        } else {
            println!(
                "imported: {} added, {} updated, {} unchanged, {} projects merged by name, {} links, {} synonyms",
                stats.added, stats.updated, stats.unchanged, stats.merged_projects, stats.edges, stats.synonyms
            );
        }
        Ok(())
    }

    fn context(&self, project: Option<&str>) -> Result<()> {
        let name = project_name(project)?;
        let Some(project_id) = store::find_project(&self.conn, &name)? else {
            if self.json {
                render::print_json(&json!({ "project": null, "name": name }));
            } else {
                println!("# Context: {name}\n\nNothing is stored for this project yet.");
                println!("`son recall <terms>` still searches every other project.");
            }
            return Ok(());
        };
        let project = store::get_node(&self.conn, &project_id)?;
        let of_kind = |kind: &str, limit: usize| -> Result<Vec<Node>> {
            let mut nodes = store::list_nodes(&self.conn, Some(&project_id), Some(kind), false)?;
            nodes.truncate(limit);
            Ok(nodes)
        };

        let mut decisions = Vec::new();
        for node in store::list_nodes(&self.conn, Some(&project_id), Some("decision"), false)? {
            if decisions.len() == CONTEXT_SECTION_LIMIT {
                break;
            }
            if store::superseded_by(&self.conn, &node.id)?.is_none() {
                decisions.push(node);
            }
        }
        let mut todos = of_kind("todo", usize::MAX)?;
        todos.retain(|t| t.status.as_deref() == Some("open"));
        todos.sort_by(|a, b| a.id.cmp(&b.id));

        render::context(
            &ProjectContext {
                project: &project,
                patterns: of_kind("pattern", CONTEXT_SECTION_LIMIT)?,
                decisions,
                facts: of_kind("fact", CONTEXT_SECTION_LIMIT)?,
                todos,
                notes: of_kind("note", CONTEXT_NOTES_LIMIT)?,
            },
            self.json,
        );
        Ok(())
    }
}
