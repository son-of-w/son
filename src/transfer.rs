//! Export and import. JSON is the backup format and round-trips; markdown is
//! for reading.

use crate::analyze::Analyzer;
use crate::history::{EdgeChange, Recorder};
use crate::store::{self, Meta, Node, NOW};
use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

/// Version 2 says "collection" where version 1 said "project": the node
/// type, and the fields `collection_id` and `collection`. Version 1 files
/// are still read.
pub const DUMP_VERSION: u32 = 2;

fn local() -> String {
    "local".to_string()
}

#[derive(Serialize, Deserialize)]
pub struct EdgeRow {
    pub from_id: String,
    pub to_id: String,
    pub relation: String,
    pub created_at: String,
    #[serde(default = "local")]
    pub origin: String,
}

#[derive(Serialize, Deserialize)]
pub struct SynonymRow {
    pub term: String,
    pub canonical: String,
}

#[derive(Serialize, Deserialize)]
pub struct Dump {
    pub version: u32,
    pub exported_at: String,
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<EdgeRow>,
    #[serde(default)]
    pub synonyms: Vec<SynonymRow>,
}

impl Dump {
    /// Bring a file of an older version to the current form.
    pub fn upgrade(&mut self) -> Result<()> {
        if self.version == 0 || self.version > DUMP_VERSION {
            return Err(crate::error::coded(
                "import_failed",
                format!(
                    "this file is export version {}, but this son reads versions 1 to {DUMP_VERSION}",
                    self.version
                ),
            ));
        }
        for node in &mut self.nodes {
            if node.kind == "project" {
                node.kind = "collection".to_string();
            }
        }
        self.version = DUMP_VERSION;
        Ok(())
    }
}

/// Everything, or one collection. Dropped nodes are included so a backup can
/// still restore them.
pub fn export(conn: &Connection, project_id: Option<&str>) -> Result<Dump> {
    let mut nodes = store::list_nodes(conn, project_id, None, true)?;
    // Collections first, then oldest first (a ULID sorts by creation time).
    nodes.sort_by(|a, b| (a.kind != "collection", &a.id).cmp(&(b.kind != "collection", &b.id)));
    let ids: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();

    let mut stmt = conn.prepare(
        "SELECT from_id, to_id, relation, created_at, origin FROM edges ORDER BY created_at, from_id",
    )?;
    let edges: Vec<EdgeRow> = stmt
        .query_map([], |r| {
            Ok(EdgeRow {
                from_id: r.get(0)?,
                to_id: r.get(1)?,
                relation: r.get(2)?,
                created_at: r.get(3)?,
                origin: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter(|e| ids.contains(e.from_id.as_str()) && ids.contains(e.to_id.as_str()))
        .collect();

    let mut synonyms: Vec<SynonymRow> = store::synonyms(conn)?
        .into_iter()
        .map(|(term, canonical)| SynonymRow { term, canonical })
        .collect();
    synonyms.sort_by(|a, b| a.term.cmp(&b.term));

    let exported_at: String = conn.query_row(&format!("SELECT {NOW}"), [], |r| r.get(0))?;
    Ok(Dump {
        version: DUMP_VERSION,
        exported_at,
        nodes,
        edges,
        synonyms,
    })
}

/// A readable dump: one section per collection, live nodes only.
pub fn markdown(dump: &Dump) -> String {
    let mut out = String::new();
    let titles: HashMap<&str, &Node> = dump.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let _ = writeln!(out, "# son export ({})", dump.exported_at);

    for project in dump
        .nodes
        .iter()
        .filter(|n| n.kind == "collection" && n.deleted_at.is_none())
    {
        let _ = writeln!(out, "\n## Collection: {}", project.title);
        if !project.body.trim().is_empty() {
            let _ = writeln!(out, "\n{}", project.body.trim());
        }
        for node in dump.nodes.iter().filter(|n| {
            n.deleted_at.is_none() && n.project_id.as_deref() == Some(project.id.as_str())
        }) {
            let _ = writeln!(out, "\n### [{}] {}", node.kind, node.title);
            let mut facts = vec![format!("id: `{}`", node.short())];
            if let Some(status) = &node.status {
                facts.push(format!("status: {status}"));
            }
            if !node.tags.is_empty() {
                facts.push(format!("tags: {}", node.tags.join(", ")));
            }
            facts.push(format!("updated: {}", node.updated_at));
            facts.extend(crate::render::trust(node));
            let _ = writeln!(out, "{}", facts.join(" · "));
            if !node.body.trim().is_empty() {
                let _ = writeln!(out, "\n{}", node.body.trim());
            }
            let links: Vec<String> = dump
                .edges
                .iter()
                .filter(|e| e.from_id == node.id)
                .filter_map(|e| {
                    let to = titles.get(e.to_id.as_str())?;
                    Some(format!("- {} → `{}` {}", e.relation, to.short(), to.title))
                })
                .collect();
            if !links.is_empty() {
                let _ = writeln!(out, "\n{}", links.join("\n"));
            }
            for r in &node.refs {
                let _ = writeln!(out, "- evidence {}: {}", r.kind, r.value);
            }
        }
    }
    out
}

#[derive(Default, Serialize)]
pub struct ImportStats {
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    /// Collections in the file that matched a local collection of the same name.
    pub merged_collections: usize,
    pub edges: usize,
    pub synonyms: usize,
}

/// How many faults an import error lists before it only counts them.
const FAULT_LIMIT: usize = 20;
/// The types a node can have.
pub const NODE_TYPES: [&str; 6] = ["collection", "todo", "decision", "pattern", "fact", "note"];

/// True for an id of the form `son` makes: 26 characters, each a digit or
/// an uppercase letter of the basic alphabet.
pub fn is_id(id: &str) -> bool {
    id.len() == 26
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
}

/// True for a time as `son` writes it: `2026-11-13T09:30:00.123Z`. The
/// part after the seconds can be left out.
pub fn is_time(text: &str) -> bool {
    let bytes = text.as_bytes();
    let number = |from: usize, to: usize| -> Option<u32> {
        let part = bytes.get(from..to)?;
        if !part.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(part).ok()?.parse().ok()
    };
    let mark = |at: usize, want: u8| bytes.get(at) == Some(&want);
    let parts = (
        number(0, 4),
        number(5, 7),
        number(8, 10),
        number(11, 13),
        number(14, 16),
        number(17, 19),
    );
    let (Some(_), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = parts else {
        return false;
    };
    if !(mark(4, b'-') && mark(7, b'-') && mark(10, b'T') && mark(13, b':') && mark(16, b':')) {
        return false;
    }
    if !((1..=12).contains(&month) && (1..=31).contains(&day)) {
        return false;
    }
    if hour > 23 || minute > 59 || second > 59 {
        return false;
    }
    match &bytes[19..] {
        [b'Z'] => true,
        [b'.', digits @ .., b'Z'] => !digits.is_empty() && digits.iter().all(u8::is_ascii_digit),
        _ => false,
    }
}

/// A value for an error message: cut short, and safe to print.
fn shown(text: &str) -> String {
    let mut out: String = text.chars().take(40).flat_map(char::escape_debug).collect();
    if text.chars().count() > 40 {
        out.push('…');
    }
    out
}

/// What is wrong with one node, looked at by itself. Empty when nothing
/// is. `son doctor` uses this for stored rows too.
pub fn node_faults(node: &Node) -> Vec<String> {
    let mut faults = Vec::new();
    if !is_id(&node.id) {
        faults.push(
            "the id is not 26 characters of digits and uppercase letters, the form son makes"
                .to_string(),
        );
    }
    if !NODE_TYPES.contains(&node.kind.as_str()) {
        faults.push(format!("type '{}' is not known", shown(&node.kind)));
    }
    if node.title.trim().is_empty() {
        faults.push("the title is empty".to_string());
    }
    match (&node.status, node.kind.as_str()) {
        (None, "todo") => faults.push("a todo needs a status".to_string()),
        (Some(status), _) if !store::TODO_STATUSES.contains(&status.as_str()) => {
            faults.push(format!("status '{}' is not known", shown(status)));
        }
        _ => {}
    }
    match (&node.project_id, node.kind.as_str()) {
        (Some(_), "collection") => {
            faults.push("a collection cannot be in a collection".to_string())
        }
        (None, kind) if kind != "collection" => {
            faults.push("the node is in no collection".to_string());
        }
        (Some(id), _) if !is_id(id) => faults.push("collection_id is not an id".to_string()),
        _ => {}
    }
    let times = [
        ("created_at", Some(&node.created_at)),
        ("updated_at", Some(&node.updated_at)),
        ("deleted_at", node.deleted_at.as_ref()),
        ("verified_at", node.verified_at.as_ref()),
        ("stale_at", node.stale_at.as_ref()),
    ];
    for (name, value) in times {
        if let Some(value) = value {
            if !is_time(value) {
                faults.push(format!(
                    "{name} '{}' is not a time such as 2026-11-13T09:30:00.000Z",
                    shown(value)
                ));
            }
        }
    }
    if let Some(evidence) = &node.evidence {
        if !store::EVIDENCE.contains(&evidence.as_str()) {
            faults.push(format!("evidence '{}' is not known", shown(evidence)));
        }
    }
    for r in &node.refs {
        if !store::REF_KINDS.contains(&r.kind.as_str()) {
            faults.push(format!("reference kind '{}' is not known", shown(&r.kind)));
        } else if r.value.trim().is_empty() {
            faults.push("a reference has no value".to_string());
        }
    }
    if node.tags.iter().any(|tag| tag.contains(',')) {
        faults.push("a tag holds a comma".to_string());
    }
    if let Some(id) = &node.merged_into {
        if !is_id(id) {
            faults.push("merged_into is not an id".to_string());
        }
    }
    if node.rev < 1 {
        faults.push(format!("rev {} is less than 1", node.rev));
    }
    faults
}

/// Every fault in the file, each with the place of its record. Looks at
/// the records one by one, and at how they point at each other and at the
/// nodes stored here.
fn faults(conn: &Connection, dump: &Dump) -> Result<Vec<String>> {
    let mut found = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    let kinds: HashMap<&str, &str> = dump
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n.kind.as_str()))
        .collect();
    let stored_kind = |id: &str| -> Result<Option<String>> {
        Ok(conn
            .query_row("SELECT type FROM nodes WHERE id = ?1", [id], |r| r.get(0))
            .optional()?)
    };
    for (at, node) in dump.nodes.iter().enumerate() {
        let place = format!("nodes[{at}] (id '{}')", shown(&node.id));
        for fault in node_faults(node) {
            found.push(format!("{place}: {fault}"));
        }
        if !seen.insert(node.id.as_str()) {
            found.push(format!("{place}: the id is in the file more than once"));
        }
        if let Some(stored) = stored_kind(&node.id)? {
            if stored != node.kind && NODE_TYPES.contains(&node.kind.as_str()) {
                found.push(format!(
                    "{place}: the node is stored here as a {stored}, and the file says {}",
                    node.kind
                ));
            }
        }
        if let Some(parent) = node.project_id.as_deref().filter(|id| is_id(id)) {
            let kind = match kinds.get(parent) {
                Some(kind) => Some(kind.to_string()),
                None => stored_kind(parent)?,
            };
            match kind.as_deref() {
                Some("collection") => {}
                Some(other) => found.push(format!(
                    "{place}: collection_id points at a {}, not at a collection",
                    shown(other)
                )),
                None => found.push(format!(
                    "{place}: collection_id points at a node that is not in the file and not stored here"
                )),
            }
        }
    }
    for (at, edge) in dump.edges.iter().enumerate() {
        let place = format!("edges[{at}]");
        if !is_id(&edge.from_id) || !is_id(&edge.to_id) {
            found.push(format!("{place}: from_id or to_id is not an id"));
        } else if edge.from_id == edge.to_id {
            found.push(format!("{place}: a node is linked to itself"));
        }
        if !store::RELATIONS.contains(&edge.relation.as_str()) {
            found.push(format!(
                "{place}: relation '{}' is not known",
                shown(&edge.relation)
            ));
        }
        if !is_time(&edge.created_at) {
            found.push(format!("{place}: created_at is not a time"));
        }
    }
    for (at, synonym) in dump.synonyms.iter().enumerate() {
        if synonym.term.trim().is_empty() || synonym.canonical.trim().is_empty() {
            found.push(format!("synonyms[{at}]: the term or its meaning is empty"));
        }
    }
    Ok(found)
}

/// Stop an import that holds a record this `son` cannot keep safely.
fn refuse_faults(conn: &Connection, dump: &Dump) -> Result<()> {
    let found = faults(conn, dump)?;
    if found.is_empty() {
        return Ok(());
    }
    let mut lines: Vec<String> = found
        .iter()
        .take(FAULT_LIMIT)
        .map(|f| format!("  {f}"))
        .collect();
    if found.len() > FAULT_LIMIT {
        lines.push(format!("  and {} more", found.len() - FAULT_LIMIT));
    }
    Err(crate::error::coded(
        "import_failed",
        format!(
            "nothing was imported: the file holds {} fault(s)
{}
             A value that is not known can mean the file is from a newer son.",
            found.len(),
            lines.join("\n")
        ),
    ))
}

/// Load a dump. Every record is checked first; one fault refuses the whole
/// file and nothing is changed. A node that is already here is replaced only when the
/// incoming copy is newer. A collection in the file with the same name as a
/// local collection is merged into the local one. Call [`Dump::upgrade`]
/// first.
pub fn import(conn: &Connection, an: &Analyzer, meta: &Meta, dump: &Dump) -> Result<ImportStats> {
    if dump.version != DUMP_VERSION {
        bail!("upgrade the export before importing it");
    }
    store::write(conn, || {
        refuse_faults(conn, dump)?;
        // Rows may arrive before the rows they point at.
        conn.execute_batch("PRAGMA defer_foreign_keys = ON")?;
        let mut stats = ImportStats::default();
        // The whole import is one operation in the history.
        let mut rec = Recorder::new(conn, "import");

        let mut remap: HashMap<&str, String> = HashMap::new();
        for node in dump
            .nodes
            .iter()
            .filter(|n| n.kind == "collection" && n.deleted_at.is_none())
        {
            if stored_updated_at(conn, &node.id)?.is_some() {
                continue;
            }
            if let Some(local) = store::find_project(conn, &node.title)? {
                remap.insert(node.id.as_str(), local);
            }
        }
        let mapped = |id: &str| remap.get(id).cloned().unwrap_or_else(|| id.to_string());

        for node in &dump.nodes {
            if remap.contains_key(node.id.as_str()) {
                stats.merged_collections += 1;
                continue;
            }
            rec.touch(&node.id)?;
            match stored_updated_at(conn, &node.id)? {
                Some(stored) if stored >= node.updated_at => {
                    // The content here is as new or newer. A verification
                    // is apart from the content: take it when it is newer,
                    // and keep every reference to evidence from both.
                    let newer_check = conn.execute(
                        "UPDATE nodes SET verified_at = ?2, verified_by = ?3, verified_note = ?4 \
                         WHERE id = ?1 AND ?2 IS NOT NULL AND (verified_at IS NULL OR verified_at < ?2)",
                        params![node.id, node.verified_at, node.verified_by, node.verified_note],
                    )?;
                    let mut new_refs = 0;
                    for r in &node.refs {
                        new_refs += conn.execute(
                            "INSERT OR IGNORE INTO refs (node_id, kind, value, created_at) VALUES (?1, ?2, ?3, ?4)",
                            params![node.id, r.kind, r.value, node.updated_at],
                        )?;
                    }
                    if newer_check + new_refs > 0 {
                        stats.updated += 1;
                    } else {
                        stats.unchanged += 1;
                    }
                    continue;
                }
                Some(_) => stats.updated += 1,
                None => stats.added += 1,
            }
            let project_id = node.project_id.as_deref().map(mapped);
            conn.execute(
                "INSERT INTO nodes (id, type, title, body, status, project_id, created_at, updated_at, \
                                    deleted_at, source, origin, author, visibility, rev, \
                                    evidence, verified_at, verified_by, verified_note, merged_into, \
                                    stale_at, stale_note) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21) \
                 ON CONFLICT(id) DO UPDATE SET \
                    type = excluded.type, title = excluded.title, body = excluded.body, \
                    status = excluded.status, project_id = excluded.project_id, \
                    created_at = excluded.created_at, updated_at = excluded.updated_at, \
                    deleted_at = excluded.deleted_at, source = excluded.source, \
                    origin = excluded.origin, author = excluded.author, \
                    visibility = excluded.visibility, rev = excluded.rev, \
                    evidence = excluded.evidence, verified_at = excluded.verified_at, \
                    verified_by = excluded.verified_by, verified_note = excluded.verified_note, \
                    merged_into = excluded.merged_into, stale_at = excluded.stale_at, \
                    stale_note = excluded.stale_note",
                params![
                    node.id,
                    node.kind,
                    node.title,
                    node.body,
                    node.status,
                    project_id,
                    node.created_at,
                    node.updated_at,
                    node.deleted_at,
                    node.source,
                    node.origin,
                    node.author,
                    node.visibility,
                    node.rev,
                    node.evidence,
                    node.verified_at,
                    node.verified_by,
                    node.verified_note,
                    node.merged_into,
                    node.stale_at,
                    node.stale_note
                ],
            )?;
            store::set_tags(conn, &node.id, &node.tags)?;
            store::set_refs(conn, &node.id, &node.refs)?;
            store::reindex(conn, an, &node.id)?;
        }

        for edge in &dump.edges {
            let (from, to) = (mapped(&edge.from_id), mapped(&edge.to_id));
            if stored_updated_at(conn, &from)?.is_none() || stored_updated_at(conn, &to)?.is_none()
            {
                continue;
            }
            let added = conn.execute(
                "INSERT OR IGNORE INTO edges (from_id, to_id, relation, created_at, origin) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![from, to, edge.relation, edge.created_at, edge.origin],
            )?;
            if added > 0 {
                stats.edges += 1;
                rec.linked(EdgeChange {
                    from_id: from,
                    to_id: to,
                    relation: edge.relation.clone(),
                    added: true,
                    created_at: edge.created_at.clone(),
                    origin: edge.origin.clone(),
                });
            }
        }

        for synonym in &dump.synonyms {
            stats.synonyms += conn.execute(
                "INSERT OR IGNORE INTO synonyms (term, canonical) VALUES (?1, ?2)",
                params![synonym.term, synonym.canonical],
            )?;
        }
        rec.finish(meta)?;
        Ok(stats)
    })
}

fn stored_updated_at(conn: &Connection, id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT updated_at FROM nodes WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .optional()?)
}
