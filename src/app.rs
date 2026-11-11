//! Runs one command.

use crate::activity::{self, Draft, EventFilter, NodeFilter};
use crate::agents::Scope;
use crate::analyze::Analyzer;
use crate::cli::{
    Cli, Command, Evidence, NodeType, Relation, ScopeArg, SynonymCommand, TodoCommand,
};
use crate::error::coded;
use crate::history::{self, Recorder};
use crate::render::{self, Context as ProjectContext};
use crate::search;
use crate::store::{self, Meta, NewNode, Node, Ref, TODO_STATUSES};
use crate::{db, instructions, project, report, setup, transfer};
use anyhow::{anyhow, bail, Context, Result};
use clap::ValueEnum;
use rusqlite::{Connection, OptionalExtension};
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
        Command::Stale { .. } => ("stale", true),
        Command::Review { .. } => ("review", false),
        Command::Merge { .. } => ("merge", true),
        Command::History { .. } => ("history", false),
        Command::Undo { .. } => ("undo", true),
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
    // A check promises to change nothing: no database is made or upgraded,
    // and nothing goes into the activity log.
    match &cli.command {
        Command::Remember {
            check: true,
            kind,
            title,
            body,
            collection,
            ..
        } => {
            let body = read_body(body.clone())?;
            return check(*kind, title, &body, collection.as_deref(), cli.json);
        }
        Command::Todo {
            command:
                TodoCommand::Add {
                    check: true,
                    title,
                    body,
                    collection,
                    ..
                },
        } => {
            let body = read_body(body.clone())?;
            return check(
                NodeType::Todo,
                title,
                &body,
                collection.as_deref(),
                cli.json,
            );
        }
        _ => {}
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
            check: _,
            key,
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
                key: key.as_deref(),
            })
        }
        Command::Recall {
            query,
            kind,
            collection: project,
            limit,
            hops,
            max_chars,
            explain,
        } => app.recall(
            &query.join(" "),
            kind,
            project.as_deref(),
            limit,
            hops,
            &render::Budget { max_chars, explain },
        ),
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
        Command::Stale { id, reason, clear } => app.stale(&id, reason.as_deref(), clear),
        Command::Review {
            collection,
            kind,
            fact_days,
            decision_days,
            todo_days,
            limit,
            page,
        } => {
            let mut limits = crate::review::Limits::from_env();
            limits.fact_days = fact_days.unwrap_or(limits.fact_days);
            limits.decision_days = decision_days.unwrap_or(limits.decision_days);
            limits.todo_days = todo_days.unwrap_or(limits.todo_days);
            app.review(
                collection.as_deref(),
                kind,
                &limits,
                limit as usize,
                page as usize,
            )
        }
        Command::Merge {
            source,
            survivor,
            body,
            title,
            cross_collection,
        } => {
            let body = body.map(|b| read_body(Some(b))).transpose()?;
            app.merge(&source, &survivor, body, title, cross_collection)
        }
        Command::History {
            id,
            op,
            limit,
            page,
        } => app.history(&id, op, limit as usize, page as usize),
        Command::Undo { id, op } => app.undo(&id, op),
        Command::Todo { command } => match command {
            TodoCommand::Add {
                title,
                body,
                collection: project,
                tag,
                evidence,
                refs,
                check: _,
                key,
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
                    key: key.as_deref(),
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
            max_chars,
        } => app.context(project.as_deref(), max_chars),
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

/// `son remember --check`: list what a new node may duplicate. Reads only.
fn check(
    kind: NodeType,
    title: &str,
    body: &str,
    collection: Option<&str>,
    json: bool,
) -> Result<()> {
    let title = title.trim();
    if title.is_empty() {
        bail!("the title is empty");
    }
    let collection = project_name(collection)?;
    let candidates = match db::open_read_only(&db::path()?)? {
        Some(conn) if kind != NodeType::Collection => {
            search::duplicates(&conn, &Analyzer::new(), title, body, kind.as_str(), None, 5)?
        }
        _ => Vec::new(),
    };
    render::check(kind.as_str(), title, &collection, &candidates, json);
    Ok(())
}

fn no_collection(name: &str) -> anyhow::Error {
    coded(
        "unknown_collection",
        format!(
            "no collection named '{}' (see `son collections`)",
            name.trim()
        ),
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
    /// The idempotency key, if the caller gave one.
    key: Option<&'a str>,
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
        let key = new.key.map(str::trim);
        if key.is_some_and(|key| key.is_empty() || key.chars().count() > 200) {
            return Err(coded("invalid_argument", "--key needs 1 to 200 characters"));
        }
        // What was asked, in one fixed form: the same request gives the
        // same text, whatever the order of tags or the spaces around them.
        let request_collection = if kind == NodeType::Collection {
            None
        } else {
            Some(project_name(project)?.to_lowercase())
        };
        let mut request_refs = refs.clone();
        request_refs.sort();
        request_refs.dedup();
        let request = json!({
            "command": "remember",
            "type": kind.as_str(),
            "title": title,
            "body": body,
            "collection": request_collection,
            "tags": store::clean_tags(tags),
            "links": links,
            "evidence": evidence,
            "refs": request_refs,
        })
        .to_string();
        let saved = store::write(&self.conn, || {
            if let Some(key) = key {
                let earlier: Option<(String, String)> = self
                    .conn
                    .query_row(
                        "SELECT request, result FROM idempotency WHERE key = ?1",
                        [key],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                match earlier {
                    Some((asked, result)) if asked == request => return Ok(Err(result)),
                    Some(_) => {
                        return Err(coded(
                            "idempotency_conflict",
                            format!(
                                "the key '{key}' was already used for a different write. Nothing was saved; \
                                 use a new key for a new write"
                            ),
                        ))
                    }
                    None => {}
                }
            }
            let mut rec = Recorder::new(&self.conn, "create");
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
                let project_id = history::ensure_collection(
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
            rec.created(&id);
            for link in links {
                let (target, relation) = parse_link(link)?;
                let to = store::resolve_id(&self.conn, target)?;
                rec.link(&id, relation.as_str(), &to)?;
            }
            rec.finish(&self.meta)?;
            if let Some(key) = key {
                // The answer is kept with the write, in the same transaction.
                let node = store::get_node(&self.conn, &id)?;
                self.conn.execute(
                    &format!(
                        "INSERT INTO idempotency (key, command, request, node_id, result, created_at) \
                         VALUES (?1, 'remember', ?2, ?3, ?4, {})",
                        store::NOW
                    ),
                    rusqlite::params![key, request, id, render::node_json(&node).to_string()],
                )?;
            }
            Ok(Ok(id))
        })?;
        match saved {
            Ok(id) => {
                let node = store::get_node(&self.conn, &id)?;
                self.note_node(&node);
                render::saved("saved", &node, self.json);
            }
            // The same key and the same request: give the first answer
            // again, exactly, and save nothing.
            Err(result) => {
                let node: Node = serde_json::from_str(&result)
                    .context("the stored answer for this key cannot be read")?;
                self.note_node(&node);
                self.note(|draft| {
                    draft.detail = Some(format!(
                        "replay of key {}: {}",
                        key.unwrap_or_default(),
                        draft.detail.clone().unwrap_or_default()
                    ));
                });
                if self.json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::from_str::<serde_json::Value>(
                            &result
                        )?)?
                    );
                } else {
                    println!(
                        "saved {} (already saved with this key; nothing new was written)",
                        render::line(&node)
                    );
                }
            }
        }
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
            let mut rec = Recorder::new(&self.conn, if clear { "unverify" } else { "verify" });
            rec.touch(&id)?;
            if clear {
                if node.verified_at.is_some() {
                    store::unverify(&self.conn, &id)?;
                }
            } else {
                if !added.is_empty() {
                    let mut refs = node.refs.clone();
                    refs.extend(added.iter().cloned());
                    store::set_refs(&self.conn, &id, &refs)?;
                }
                let by = self.meta.source.as_deref().or(self.meta.author.as_deref());
                store::verify(&self.conn, &id, by, checked)?;
            }
            rec.finish(&self.meta)?;
            Ok(())
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
        budget: &render::Budget,
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
        print!(
            "{}",
            render::recall(terms, &name, &result, self.json, budget)?
        );
        Ok(())
    }

    fn show(&self, id: &str) -> Result<()> {
        let id = store::resolve_id(&self.conn, id)?;
        let node = store::get_node(&self.conn, &id)?;
        self.note_node(&node);
        let edges = store::edges_of(&self.conn, &id)?;
        let newer = store::superseded_by(&self.conn, &id)?;
        let merged_from = store::merged_from(&self.conn, &id)?;
        render::show(&node, &edges, newer.as_ref(), &merged_from, self.json);
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
        let changed = store::write(&self.conn, || {
            let mut rec = Recorder::new(&self.conn, "edit");
            rec.touch(&id)?;
            let mut node = store::get_node(&self.conn, &id)?;
            let original = node.clone();
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
                node.project_id = Some(history::ensure_collection(
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
            node.refs.sort();
            node.refs.dedup();
            node.tags = store::clean_tags(&node.tags);
            // An edit that changes nothing is not a revision.
            if history::same_content(&original, &node) {
                return Ok(false);
            }
            store::update_node(&self.conn, &self.an, &node)?;
            rec.finish(&self.meta)?;
            Ok(true)
        })?;
        let node = store::get_node(&self.conn, &id)?;
        self.note_node(&node);
        render::saved(
            if changed { "updated" } else { "unchanged" },
            &node,
            self.json,
        );
        Ok(())
    }

    fn drop_node(&self, id: &str, purge: bool) -> Result<()> {
        let id = store::resolve_id(&self.conn, id)?;
        let node = store::get_node(&self.conn, &id)?;
        self.note_node(&node);
        store::write(&self.conn, || {
            if purge {
                return history::purge(&self.conn, &id);
            }
            let mut rec = Recorder::new(&self.conn, "drop");
            rec.touch(&id)?;
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
            store::soft_drop(&self.conn, &self.an, &id)?;
            rec.finish(&self.meta)?;
            Ok(())
        })?;
        if purge {
            // A purged node leaves no trace, in the activity log either.
            self.note(|draft| {
                draft.node_id = None;
                draft.detail = None;
            });
        }
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
                "dropped {} (bring it back with `son restore {}`)",
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
            let mut rec = Recorder::new(&self.conn, "restore");
            rec.touch(&id)?;
            store::restore(&self.conn, &self.an, &id)?;
            rec.finish(&self.meta)?;
            Ok(())
        })?;
        render::saved("restored", &store::get_node(&self.conn, &id)?, self.json);
        Ok(())
    }

    fn stale(&self, id: &str, reason: Option<&str>, clear: bool) -> Result<()> {
        let id = store::resolve_id(&self.conn, id)?;
        let node = store::get_node(&self.conn, &id)?;
        self.note_node(&node);
        let reason = reason.map(str::trim).unwrap_or("");
        if !clear && reason.is_empty() {
            bail!("say why it is out of date: --reason \"<text>\"");
        }
        if node.kind == "collection" {
            bail!("a collection is a container; mark the nodes in it");
        }
        store::write(&self.conn, || {
            let mut rec = Recorder::new(&self.conn, if clear { "unstale" } else { "stale" });
            rec.touch(&id)?;
            if !clear {
                store::mark_stale(&self.conn, &id, reason)?;
            } else if node.stale_at.is_some() {
                store::clear_stale(&self.conn, &id)?;
            }
            rec.finish(&self.meta)?;
            Ok(())
        })?;
        let verb = if clear {
            "no longer stale:"
        } else {
            "marked stale:"
        };
        render::saved(verb, &store::get_node(&self.conn, &id)?, self.json);
        Ok(())
    }

    fn review(
        &self,
        collection: Option<&str>,
        kind: Option<NodeType>,
        limits: &crate::review::Limits,
        limit: usize,
        page: usize,
    ) -> Result<()> {
        let (scope, collection_id) = match collection {
            Some(name) => {
                let id = store::find_project(&self.conn, name.trim())?
                    .ok_or_else(|| no_collection(name))?;
                (name.trim().to_string(), Some(id))
            }
            None => ("all collections".to_string(), None),
        };
        let all = crate::review::items(
            &self.conn,
            &self.an,
            limits,
            &crate::review::Filter {
                collection_id: collection_id.as_deref(),
                kind: kind.map(NodeType::as_str),
            },
        )?;
        let total = all.len() as i64;
        let items: Vec<_> = all
            .into_iter()
            .skip(limit * (page - 1))
            .take(limit)
            .collect();
        self.note(|draft| {
            draft.project = collection.map(|name| name.trim().to_string());
            draft.results = Some(total);
        });
        let mut again = "son review".to_string();
        if let Some(name) = collection {
            again.push_str(&format!(" --collection {}", name.trim()));
        }
        if let Some(kind) = kind {
            again.push_str(&format!(" --type {}", kind.as_str()));
        }
        let default = crate::review::Limits::from_env();
        for (flag, value, usual) in [
            ("--fact-days", limits.fact_days, default.fact_days),
            (
                "--decision-days",
                limits.decision_days,
                default.decision_days,
            ),
            ("--todo-days", limits.todo_days, default.todo_days),
        ] {
            if value != usual {
                again.push_str(&format!(" {flag} {value}"));
            }
        }
        if limit != 20 {
            again.push_str(&format!(" --limit {limit}"));
        }
        report::review(
            &activity::Page::new(items, total, limit, page),
            &scope,
            &again,
            self.json,
        );
        Ok(())
    }

    fn merge(
        &self,
        source: &str,
        survivor: &str,
        body: Option<String>,
        title: Option<String>,
        cross_collection: bool,
    ) -> Result<()> {
        let source_id = store::resolve_id(&self.conn, source)?;
        let survivor_id = store::resolve_id(&self.conn, survivor)?;
        if source_id == survivor_id {
            bail!("a node cannot be merged into itself");
        }
        let title = title.map(|t| t.trim().to_string());
        if title.as_deref() == Some("") {
            bail!("the title is empty");
        }
        let moved = store::write(&self.conn, || {
            let source = store::get_node(&self.conn, &source_id)?;
            let mut survivor = store::get_node(&self.conn, &survivor_id)?;
            self.note_node(&survivor);
            for node in [&source, &survivor] {
                if node.kind == "collection" {
                    bail!("a collection cannot be merged; move its nodes with `son edit <id> --collection`");
                }
                if let Some(into) = &node.merged_into {
                    bail!(
                        "`{}` was already merged into `{}`",
                        node.short(),
                        store::short_id(into)
                    );
                }
                if node.deleted_at.is_some() {
                    bail!(
                        "`{}` is dropped; bring it back with `son restore {}` before a merge",
                        node.short(),
                        node.short()
                    );
                }
            }
            if source.kind != survivor.kind {
                bail!(
                    "`{}` is a {} and `{}` is a {}; a merge needs the same type (change one with `son edit <id> --type`)",
                    source.short(),
                    source.kind,
                    survivor.short(),
                    survivor.kind
                );
            }
            if source.project_id != survivor.project_id && !cross_collection {
                bail!(
                    "`{}` is in collection '{}' and `{}` is in '{}'; add --cross-collection to merge them (the survivor stays in '{}')",
                    source.short(),
                    source.project_name(),
                    survivor.short(),
                    survivor.project_name(),
                    survivor.project_name()
                );
            }
            // The body: the caller's text, else the one body there is. Two
            // different bodies are a conflict that only the caller can settle.
            let (old, new) = (source.body.trim(), survivor.body.trim());
            survivor.body = match body {
                Some(text) => text,
                None if old.is_empty() || old == new => survivor.body.clone(),
                None if new.is_empty() => source.body.clone(),
                None => bail!(
                    "the two bodies differ; give the merged text with --body (\"-\" reads stdin). \
                     See both with `son show {}` and `son show {}`. Nothing was changed",
                    source.short(),
                    survivor.short()
                ),
            };
            if let Some(title) = title {
                survivor.title = title;
            }
            survivor.tags.extend(source.tags.iter().cloned());
            survivor.tags = store::clean_tags(&survivor.tags);
            survivor.refs.extend(source.refs.iter().cloned());
            survivor.refs.sort();
            survivor.refs.dedup();
            if survivor.evidence.is_none() {
                survivor.evidence = source.evidence.clone();
            }

            let mut rec = Recorder::new(&self.conn, "merge");
            rec.touch(&source_id)?;
            rec.touch_always(&survivor_id)?;
            store::update_node(&self.conn, &self.an, &survivor)?;
            // Every link of the source moves to the survivor. A link between
            // the two would point from the survivor to itself: it goes.
            let mut moved = 0;
            for (from, relation, to) in store::raw_edges(&self.conn, &source_id)? {
                rec.unlink(&from, &relation, &to)?;
                let from = if from == source_id {
                    &survivor_id
                } else {
                    &from
                };
                let to = if to == source_id { &survivor_id } else { &to };
                if from != to && rec.link(from, &relation, to)? {
                    moved += 1;
                }
            }
            let mut gone = source.clone();
            gone.merged_into = Some(survivor_id.clone());
            store::put_snapshot(&self.conn, &self.an, &gone)?;
            store::soft_drop(&self.conn, &self.an, &source_id)?;
            rec.finish(&self.meta)?;
            Ok(moved)
        })?;
        let survivor = store::get_node(&self.conn, &survivor_id)?;
        let source = store::get_node(&self.conn, &source_id)?;
        if self.json {
            let mut value = render::node_json(&survivor);
            value["merged_from"] = json!([source.id]);
            value["links_moved"] = json!(moved);
            render::print_json(&value);
        } else {
            println!(
                "merged `{}` into {}",
                source.short(),
                render::line(&survivor)
            );
            println!(
                "{moved} link(s) moved. `{}` is dropped and points here. Reverse this with `son undo {}`.",
                source.short(),
                survivor.short()
            );
        }
        Ok(())
    }

    fn history(&self, id: &str, op: Option<i64>, limit: usize, page: usize) -> Result<()> {
        let id = store::resolve_id(&self.conn, id)?;
        let node = store::get_node(&self.conn, &id)?;
        self.note_node(&node);
        if let Some(op) = op {
            let entry = history::entry(&self.conn, &id, op)?;
            report::history_entry(&node, &entry, self.json);
            return Ok(());
        }
        let entries = history::entries_page(&self.conn, &id, limit, page)?;
        self.note(|draft| draft.results = Some(entries.items.len() as i64));
        let mut again = format!("son history {}", node.short());
        if limit != 20 {
            again.push_str(&format!(" --limit {limit}"));
        }
        report::history(&node, &entries, &again, self.json);
        Ok(())
    }

    fn undo(&self, id: &str, op: Option<i64>) -> Result<()> {
        let id = store::resolve_id(&self.conn, id)?;
        self.note_node(&store::get_node(&self.conn, &id)?);
        let undone = store::write(&self.conn, || {
            history::undo(&self.conn, &self.an, &self.meta, &id, op)
        })?;
        if self.json {
            render::print_json(&json!({
                "op": undone.op,
                "undid": undone.undid,
                "kind": undone.kind,
                "nodes": undone.nodes.iter().map(render::node_json).collect::<Vec<_>>(),
                "links": undone.links,
            }));
        } else {
            println!(
                "undid #{} ({}) as #{}; undo that with `son undo {} --op {}`",
                undone.undid,
                undone.kind,
                undone.op,
                store::short_id(&id),
                undone.op
            );
            for node in &undone.nodes {
                let state = if node.deleted_at.is_some() {
                    " (dropped)"
                } else {
                    ""
                };
                println!("- {}{state}", render::line(node));
            }
            if undone.links > 0 {
                println!("- {} link change(s) reversed", undone.links);
            }
        }
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
        if node.status.as_deref() != Some("done") {
            store::write(&self.conn, || {
                let mut rec = Recorder::new(&self.conn, "done");
                rec.touch(&id)?;
                store::set_status(&self.conn, &id, "done")?;
                rec.finish(&self.meta)?;
                Ok(())
            })?;
        }
        render::saved("done", &store::get_node(&self.conn, &id)?, self.json);
        Ok(())
    }

    fn link(&self, from: &str, relation: Relation, to: &str, add: bool) -> Result<()> {
        let from = store::resolve_id(&self.conn, from)?;
        let to = store::resolve_id(&self.conn, to)?;
        let changed = store::write(&self.conn, || {
            let mut rec = Recorder::new(&self.conn, if add { "link" } else { "unlink" });
            let changed = if add {
                rec.link(&from, relation.as_str(), &to)?
            } else {
                rec.unlink(&from, relation.as_str(), &to)?
            };
            rec.finish(&self.meta)?;
            Ok(changed)
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
        let mut dump: transfer::Dump = serde_json::from_str(&text).map_err(|err| {
            coded(
                "import_failed",
                format!("this is not a son JSON export (make one with `son export --json`): {err}"),
            )
        })?;
        dump.upgrade()?;
        self.note(|draft| draft.detail = Some(file.to_string()));
        let stats = transfer::import(&self.conn, &self.an, &self.meta, &dump)?;
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

    fn context(&self, project: Option<&str>, max_chars: Option<usize>) -> Result<()> {
        let name = project_name(project)?;
        self.note(|draft| draft.project = Some(name.clone()));
        let Some(project_id) = store::find_project(&self.conn, &name)? else {
            let text = if self.json {
                render::json_text(
                    &json!({ "collection": null, "name": name }),
                    max_chars.is_some(),
                )
            } else {
                format!(
                    "# Context: {name}\n\nNothing is stored for this collection yet.\n\
                     `son recall <terms>` still searches every other collection.\n"
                )
            };
            print!("{}", render::within(text, max_chars)?);
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

        let text = render::context(
            &ProjectContext {
                project: &project,
                patterns: of_kind("pattern", CONTEXT_SECTION_LIMIT)?,
                decisions,
                facts: of_kind("fact", CONTEXT_SECTION_LIMIT)?,
                todos,
                notes: of_kind("note", CONTEXT_NOTES_LIMIT)?,
            },
            self.json,
            max_chars,
        )?;
        print!("{text}");
        Ok(())
    }
}
