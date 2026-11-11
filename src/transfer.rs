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
            bail!(
                "this file is export version {}, but this son reads versions 1 to {DUMP_VERSION}",
                self.version
            );
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

/// Load a dump. A node that is already here is replaced only when the
/// incoming copy is newer. A collection in the file with the same name as a
/// local collection is merged into the local one. Call [`Dump::upgrade`]
/// first.
pub fn import(conn: &Connection, an: &Analyzer, meta: &Meta, dump: &Dump) -> Result<ImportStats> {
    if dump.version != DUMP_VERSION {
        bail!("upgrade the export before importing it");
    }
    store::write(conn, || {
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
                                    evidence, verified_at, verified_by, verified_note) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18) \
                 ON CONFLICT(id) DO UPDATE SET \
                    type = excluded.type, title = excluded.title, body = excluded.body, \
                    status = excluded.status, project_id = excluded.project_id, \
                    created_at = excluded.created_at, updated_at = excluded.updated_at, \
                    deleted_at = excluded.deleted_at, source = excluded.source, \
                    origin = excluded.origin, author = excluded.author, \
                    visibility = excluded.visibility, rev = excluded.rev, \
                    evidence = excluded.evidence, verified_at = excluded.verified_at, \
                    verified_by = excluded.verified_by, verified_note = excluded.verified_note",
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
                    node.verified_note
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
