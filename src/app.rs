//! Runs one command.

use crate::activity::{self, Draft, EventFilter, NodeFilter};
use crate::agents::Scope;
use crate::analyze::Analyzer;
use crate::cli::{
    Cli, Command, Evidence, NodeType, Relation, ScopeArg, SynonymCommand, TodoCommand,
};
use crate::render::{self, Context as ProjectContext};
use crate::search;
use crate::store::{self, Meta, NewNode, Node, Ref, TODO_STATUSES};
use crate::{db, instructions, project, report, setup, transfer};
use anyhow::{anyhow, bail, Context, Result};
use clap::ValueEnum;
use rusqlite::Connection;
use serde_json::json;
use std::cell::RefCell;
use std::io::Read;
use std::path::Path;
use std::time::Instant;

const CONTEXT_SECTION_LIMIT: usize = 10;
const CONTEXT_NOTES_LIMIT: usize = 5;

struct App {
    conn: Connection,
    an: Analyzer,
    meta: Meta,
    json: bool,
    /// What the running command reports about itself for the activity log.
    draft: RefCell<Draft>,
}

/// The name a command is logged under, and whether it changes the store.
/// `None` for commands that are not logged: looking at activity is not
/// activity.
fn log_name(command: &Command) -> Option<(&'static str, bool)> {
    Some(match command {
        Command::Init { .. }
        | Command::List { .. }
        | Command::Log { .. }
        | Command::Stats { .. } => return None,
        Command::Remember { .. } => ("remember", true),
        Command::Recall { .. } => ("recall", false),
        Command::Show { .. } => ("show", false),
        Command::Edit { .. } => ("edit", true),
        Command::Verify { .. } => ("verify", true),
        Command::Drop { .. } => ("drop", true),
        Command::Restore { .. } => ("restore", true),
        Command::Todo { command } => match command {
            TodoCommand::Add { .. } => ("todo add", true),
            TodoCommand::List { .. } => ("todo list", false),
            TodoCommand::Done { .. } => ("todo done", true),
        },
        Command::Link { .. } => ("link", true),
        Command::Unlink { .. } => ("unlink", true),
        Command::Collections => ("collections", false),
        Command::Synonym { command } => match command {
            SynonymCommand::Add { .. } => ("synonym add", true),
            SynonymCommand::List => ("synonym list", false),
            SynonymCommand::Remove { .. } => ("synonym remove", true),
        },
        Command::Export { .. } => ("export", false),
        Command::Import { .. } => ("import", true),
        Command::Context { .. } => ("context", false),
        Command::Reindex => ("reindex", true),
    })
}

pub fn run(cli: Cli) -> Result<()> {
    // The parts of `son init` that only print need no database.
    if let Command::Init {
        list_agents,
        print_block,
        print_skill,
        ..
    } = &cli.command
    {
        if *list_agents {
            setup::list_agents(cli.json);
            return Ok(());
        }
        if *print_block {
            print!("{}", instructions::block());
            return Ok(());
        }
        if *print_skill {
            print!("{}", instructions::skill());
            return Ok(());
        }
    }
    let started = Instant::now();
    let path = db::path()?;
    let app = App {
        conn: db::open(&path)?,
        an: Analyzer::new(),
        meta: Meta::from_env(),
        json: cli.json,
        draft: RefCell::new(Draft::default()),
    };

    let logged = log_name(&cli.command).filter(|_| activity::enabled());
    let result = dispatch(&app, &path, cli.command);
    if let Some((command, write)) = logged {
        // The log must never make a command fail or slow: ignore its errors.
        let _ = activity::record(
            &app.conn,
            &activity::Event {
                command,
                write,
                source: app.meta.source.as_deref(),
                ms: started.elapsed().as_millis() as i64,
                error: result.as_ref().err().map(|err| format!("{err:#}")),
                draft: &app.draft.borrow(),
            },
        );
    }
    result
}

fn dispatch(app: &App, path: &Path, command: Command) -> Result<()> {
    match command {
        Command::Init {
            agent,
            scope,
            global,
            yes,
            skill_only,
            db_only,
            ..
        } => {
            if db_only {
                if app.json {
                    render::print_json(&json!({ "database": path.display().to_string() }));
                } else {
                    println!("database ready: {}", path.display());
                }
                return Ok(());
            }
            let scope = match (scope, global) {
                (Some(ScopeArg::Global), _) | (None, true) => Some(Scope::Global),
                (Some(ScopeArg::Project), _) => Some(Scope::Project),
                (None, false) => None,
            };
            setup::run(
                path,
                &setup::Options {
                    agents: agent,
                    scope,
                    yes,
                    skill_only,
                    json: app.json,
                },
            )
        }
        Command::Remember {
            kind,
            title,
            body,
            collection: project,
            tag,
            link,
            evidence,
            refs,
        } => {
            let body = read_body(body)?;
            app.remember(&Remember {
                kind,
                title: &title,
                body: &body,
                project: project.as_deref(),
                tags: &tag,
                links: &link,
                evidence,
                refs: &refs,
            })
        }
        Command::Recall {
            query,
            kind,
            collection: project,
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
            collection: project,
            tag,
            evidence,
            refs,
            unref,
        } => {
            let body = body.map(|b| read_body(Some(b))).transpose()?;
            app.edit(
                &id,
                Edit {
                    title,
                    body,
                    kind,
                    status,
                    project,
                    tags: tag,
                    evidence,
                    refs,
                    unref,
                },
            )
        }
        Command::Verify {
            id,
            checked,
            refs,
            clear,
        } => app.verify(&id, checked.as_deref(), &refs, clear),
        Command::Drop { id, purge } => app.drop_node(&id, purge),
        Command::Restore { id } => app.restore(&id),
        Command::Todo { command } => match command {
            TodoCommand::Add {
                title,
                body,
                collection: project,
                tag,
                evidence,
                refs,
            } => {
                let body = read_body(body)?;
                app.remember(&Remember {
                    kind: NodeType::Todo,
                    title: &title,
                    body: &body,
                    project: project.as_deref(),
                    tags: &tag,
                    links: &[],
                    evidence,
                    refs: &refs,
                })
            }
            TodoCommand::List {
                collection: project,
                all,
                done,
            } => app.todo_list(project.as_deref(), all, done),
            TodoCommand::Done { id } => app.todo_done(&id),
        },
        Command::Link { from, relation, to } => app.link(&from, relation, &to, true),
        Command::Unlink { from, relation, to } => app.link(&from, relation, &to, false),
        Command::Collections => {
            let rows = store::list_projects(&app.conn)?;
            render::projects(&rows, &current_project_name()?, app.json);
            Ok(())
        }
        Command::Synonym { command } => app.synonym(command),
        Command::Export {
            collection: project,
        } => app.export(project.as_deref()),
        Command::Import { file } => app.import(&file),
        Command::Context {
            collection: project,
        } => app.context(project.as_deref()),
        Command::List {
            collection: project,
            kind,
            dropped,
            limit,
            page,
        } => app.list(
            project.as_deref(),
            kind,
            dropped,
            limit as usize,
            page as usize,
        ),
        Command::Log {
            collection: project,
            command,
            errors,
            limit,
            page,
        } => app.log(
            project.as_deref(),
            command.as_deref(),
            errors,
            limit as usize,
            page as usize,
        ),
        Command::Stats { days, all } => {
            let stats = activity::stats(&app.conn, (!all).then_some(days))?;
            report::stats(&stats, path, &setup::installed_files(), app.json);
            Ok(())
        }
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

/// The collection named by `--collection`, else the current one.
fn project_name(flag: Option<&str>) -> Result<String> {
    match flag.map(str::trim) {
        Some("") => bail!("--collection needs a name"),
        Some(name) => Ok(name.to_string()),
        None => current_project_name(),
    }
}

fn no_collection(name: &str) -> anyhow::Error {
    anyhow!(
        "no collection named '{}' (see `son collections`)",
        name.trim()
    )
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

/// What `son remember` and `son todo add` were given.
struct Remember<'a> {
    kind: NodeType,
    title: &'a str,
    body: &'a str,
    project: Option<&'a str>,
    tags: &'a [String],
    links: &'a [String],
    evidence: Option<Evidence>,
    refs: &'a [String],
}

/// What `son edit` was asked to change.
struct Edit {
    title: Option<String>,
    body: Option<String>,
    kind: Option<NodeType>,
    status: Option<String>,
    project: Option<String>,
    tags: Option<Vec<String>>,
    evidence: Option<Evidence>,
    refs: Vec<String>,
    unref: Vec<String>,
}

fn parse_refs(texts: &[String]) -> Result<Vec<Ref>> {
    texts.iter().map(|text| store::parse_ref(text)).collect()
}

impl App {
    /// Add to what the activity log will say about this command.
    fn note(&self, fill: impl FnOnce(&mut Draft)) {
        fill(&mut self.draft.borrow_mut());
    }

    /// Log the node a command acted on.
    fn note_node(&self, node: &Node) {
        self.note(|draft| {
            draft.node_id = Some(node.id.clone());
            draft.detail = Some(format!("[{}] {}", node.kind, node.title));
            draft.project = Some(node.project_name().to_string());
        });
    }

    fn list(
        &self,
        project: Option<&str>,
        kind: Option<NodeType>,
        dropped: bool,
        limit: usize,
        page: usize,
    ) -> Result<()> {
        let (scope, project_id) = match project {
            Some(name) => {
                let id = store::find_project(&self.conn, name.trim())?
                    .ok_or_else(|| no_collection(name))?;
                (name.trim().to_string(), Some(id))
            }
            None => ("all collections".to_string(), None),
        };
        let nodes = activity::nodes_page(
            &self.conn,
            &NodeFilter {
                project_id: project_id.as_deref(),
                kind: kind.map(NodeType::as_str),
                include_dropped: dropped,
            },
            limit,
            page,
        )?;
        // The command that gives the next page: the same filters again.
        let mut again = "son list".to_string();
        if let Some(name) = project {
            again.push_str(&format!(" --collection {}", name.trim()));
        }
        if let Some(kind) = kind {
            again.push_str(&format!(" --type {}", kind.as_str()));
        }
        if dropped {
            again.push_str(" --dropped");
        }
        if limit != 20 {
            again.push_str(&format!(" --limit {limit}"));
        }
        report::nodes(&nodes, &scope, &again, self.json);
        Ok(())
    }

    fn log(
        &self,
        project: Option<&str>,
        command: Option<&str>,
        errors: bool,
        limit: usize,
        page: usize,
    ) -> Result<()> {
        let events = activity::events_page(
            &self.conn,
            &EventFilter {
                project: project.map(str::trim),
                command: command.map(str::trim),
                errors_only: errors,
            },
            limit,
            page,
        )?;
        let mut again = "son log".to_string();
        if let Some(name) = project {
            again.push_str(&format!(" --collection {}", name.trim()));
        }
        if let Some(command) = command {
            again.push_str(&format!(" --command \"{}\"", command.trim()));
        }
        if errors {
            again.push_str(" --errors");
        }
        if limit != 20 {
            again.push_str(&format!(" --limit {limit}"));
        }
        report::events(&events, &again, self.json);
        Ok(())
    }

    fn remember(&self, new: &Remember) -> Result<()> {
        let Remember {
            kind,
            body,
            project,
            tags,
            links,
            ..
        } = *new;
        let title = new.title.trim();
        if title.is_empty() {
            bail!("the title is empty");
        }
        let evidence = new.evidence.and_then(Evidence::stored);
        let refs = parse_refs(new.refs)?;
        let id = store::write(&self.conn, || {
            let id = if kind == NodeType::Collection {
                if project.is_some() {
                    bail!("a collection cannot be inside another collection");
                }
                if let Some(existing) = store::find_project(&self.conn, title)? {
                    bail!(
                        "collection '{title}' already exists (`{}`); change it with `son edit`",
                        store::short_id(&existing)
                    );
                }
                self.insert(&NewNode {
                    kind: kind.as_str(),
                    title,
                    body,
                    status: None,
                    project_id: None,
                    tags,
                    evidence,
                    refs: &refs,
                })?
            } else {
                let project_id = store::ensure_project(
                    &self.conn,
                    &self.an,
                    &self.meta,
                    &project_name(project)?,
                )?;
                self.insert(&NewNode {
                    kind: kind.as_str(),
                    title,
                    body,
                    status: (kind == NodeType::Todo).then_some("open"),
                    project_id: Some(&project_id),
                    tags,
                    evidence,
                    refs: &refs,
                })?
            };
            for link in links {
                let (target, relation) = parse_link(link)?;
                let to = store::resolve_id(&self.conn, target)?;
                store::link(&self.conn, &id, relation.as_str(), &to)?;
            }
            Ok(id)
        })?;
        let node = store::get_node(&self.conn, &id)?;
        self.note_node(&node);
        render::saved("saved", &node, self.json);
        Ok(())
    }

    fn insert(&self, new: &NewNode) -> Result<String> {
        store::insert_node(&self.conn, &self.an, &self.meta, new)
    }

    fn verify(&self, id: &str, checked: Option<&str>, refs: &[String], clear: bool) -> Result<()> {
        let id = store::resolve_id(&self.conn, id)?;
        let added = parse_refs(refs)?;
        let checked = checked.map(str::trim).unwrap_or("");
        if !clear && checked.is_empty() {
            bail!("say what you checked: --checked \"<what you looked at and what it showed>\"");
        }
        let node = store::get_node(&self.conn, &id)?;
        self.note_node(&node);
        if node.kind == "collection" {
            bail!("a collection is a container; verify the nodes in it");
        }
        store::write(&self.conn, || {
            if clear {
                return store::unverify(&self.conn, &id);
            }
            if !added.is_empty() {
                let mut refs = node.refs.clone();
                refs.extend(added.iter().cloned());
                store::set_refs(&self.conn, &id, &refs)?;
            }
            let by = self.meta.source.as_deref().or(self.meta.author.as_deref());
            store::verify(&self.conn, &id, by, checked)
        })?;
        let verb = if clear {
            "verification removed from"
        } else {
            "verified"
        };
        render::saved(verb, &store::get_node(&self.conn, &id)?, self.json);
        Ok(())
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
        self.note(|draft| {
            draft.detail = Some(terms.to_string());
            draft.project = Some(name.clone());
        });
        let found = store::find_project(&self.conn, &name)?;
        if project.is_some() && found.is_none() {
            return Err(no_collection(&name));
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
        self.note(|draft| {
            draft.results = Some(result.hits.len() as i64);
            draft.close = Some(result.hits.iter().filter(|hit| hit.close).count() as i64);
        });
        render::recall(terms, &name, &result, self.json);
        Ok(())
    }

    fn show(&self, id: &str) -> Result<()> {
        let id = store::resolve_id(&self.conn, id)?;
        let node = store::get_node(&self.conn, &id)?;
        self.note_node(&node);
        let edges = store::edges_of(&self.conn, &id)?;
        let newer = store::superseded_by(&self.conn, &id)?;
        render::show(&node, &edges, newer.as_ref(), self.json);
        Ok(())
    }

    fn edit(&self, id: &str, edit: Edit) -> Result<()> {
        let Edit {
            title,
            body,
            kind,
            status,
            project,
            tags,
            evidence,
            refs,
            unref,
        } = edit;
        if title.is_none()
            && body.is_none()
            && kind.is_none()
            && status.is_none()
            && project.is_none()
            && tags.is_none()
            && evidence.is_none()
            && refs.is_empty()
            && unref.is_empty()
        {
            bail!(
                "nothing to change; give --title, --body, --type, --status, --collection, --tag, \
                 --evidence, --ref or --unref"
            );
        }
        let (refs, unref) = (parse_refs(&refs)?, parse_refs(&unref)?);
        let id = store::resolve_id(&self.conn, id)?;
        store::write(&self.conn, || {
            let mut node = store::get_node(&self.conn, &id)?;
            let is_project = node.kind == "collection";

            if let Some(title) = title {
                let title = title.trim().to_string();
                if title.is_empty() {
                    bail!("the title is empty");
                }
                if is_project {
                    if let Some(other) = store::find_project(&self.conn, &title)? {
                        if other != node.id {
                            bail!("another collection is already called '{title}'");
                        }
                    }
                }
                node.title = title;
            }
            if let Some(body) = body {
                node.body = body;
            }
            if let Some(kind) = kind {
                if (kind == NodeType::Collection) != is_project {
                    bail!(
                        "a collection cannot become another type, and other types cannot become collections"
                    );
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
                    bail!("a collection cannot be inside another collection");
                }
                let name = project_name(Some(&project))?;
                node.project_id = Some(store::ensure_project(
                    &self.conn, &self.an, &self.meta, &name,
                )?);
            }
            if let Some(tags) = tags {
                node.tags = tags;
            }
            if let Some(evidence) = evidence {
                node.evidence = evidence.stored().map(str::to_string);
            }
            for gone in &unref {
                if !node.refs.contains(gone) {
                    bail!(
                        "`{}` has no reference {}:{}",
                        node.short(),
                        gone.kind,
                        gone.value
                    );
                }
            }
            node.refs.retain(|r| !unref.contains(r));
            node.refs.extend(refs.iter().cloned());
            store::update_node(&self.conn, &self.an, &node)
        })?;
        let node = store::get_node(&self.conn, &id)?;
        self.note_node(&node);
        render::saved("updated", &node, self.json);
        Ok(())
    }

    fn drop_node(&self, id: &str, purge: bool) -> Result<()> {
        let id = store::resolve_id(&self.conn, id)?;
        let node = store::get_node(&self.conn, &id)?;
        self.note_node(&node);
        store::write(&self.conn, || {
            if purge {
                return store::purge(&self.conn, &id);
            }
            if node.deleted_at.is_some() {
                bail!("`{}` is already dropped", node.short());
            }
            if node.kind == "collection" {
                let children = store::live_children(&self.conn, &id)?;
                if children > 0 {
                    bail!(
                        "collection '{}' still holds {children} node(s); drop or move them first",
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
        self.note_node(&node);
        if node.deleted_at.is_none() {
            bail!("`{}` is not dropped", node.short());
        }
        store::write(&self.conn, || {
            if node.kind == "collection" && store::find_project(&self.conn, &node.title)?.is_some()
            {
                bail!("another collection is now called '{}'", node.title);
            }
            store::restore(&self.conn, &self.an, &id)
        })?;
        render::saved("restored", &store::get_node(&self.conn, &id)?, self.json);
        Ok(())
    }

    fn todo_list(&self, project: Option<&str>, all: bool, done: bool) -> Result<()> {
        let (scope, project_id) = if all && project.is_none() {
            ("all collections".to_string(), None)
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
        self.note(|draft| {
            draft.project = project_id.is_some().then(|| scope.clone());
            draft.results = Some(todos.len() as i64);
        });
        render::todos(&todos, &scope, self.json);
        Ok(())
    }

    fn todo_done(&self, id: &str) -> Result<()> {
        let id = store::resolve_id(&self.conn, id)?;
        let node = store::get_node(&self.conn, &id)?;
        self.note_node(&node);
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
        self.note(|draft| {
            draft.node_id = Some(from.id.clone());
            draft.detail = Some(format!("{} {} {}", from.title, relation.as_str(), to.title));
            draft.project = Some(from.project_name().to_string());
        });
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
                self.note(|draft| draft.detail = Some(format!("{term} -> {canonical}")));
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
                self.note(|draft| draft.detail = Some(term.clone()));
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
                store::find_project(&self.conn, name.trim())?.ok_or_else(|| no_collection(name))?,
            ),
            None => None,
        };
        self.note(|draft| draft.project = project.map(|name| name.trim().to_string()));
        let dump = transfer::export(&self.conn, project_id.as_deref())?;
        self.note(|draft| draft.results = Some(dump.nodes.len() as i64));
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
        let mut dump: transfer::Dump = serde_json::from_str(&text)
            .context("this is not a son JSON export (make one with `son export --json`)")?;
        dump.upgrade()?;
        self.note(|draft| draft.detail = Some(file.to_string()));
        let stats = transfer::import(&self.conn, &self.an, &dump)?;
        self.note(|draft| draft.results = Some((stats.added + stats.updated) as i64));
        if self.json {
            render::print_json(&serde_json::to_value(&stats)?);
        } else {
            println!(
                "imported: {} added, {} updated, {} unchanged, {} collections merged by name, {} links, {} synonyms",
                stats.added, stats.updated, stats.unchanged, stats.merged_collections, stats.edges, stats.synonyms
            );
        }
        Ok(())
    }

    fn context(&self, project: Option<&str>) -> Result<()> {
        let name = project_name(project)?;
        self.note(|draft| draft.project = Some(name.clone()));
        let Some(project_id) = store::find_project(&self.conn, &name)? else {
            if self.json {
                render::print_json(&json!({ "collection": null, "name": name }));
            } else {
                println!("# Context: {name}\n\nNothing is stored for this collection yet.");
                println!("`son recall <terms>` still searches every other collection.");
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
