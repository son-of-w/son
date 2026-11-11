//! Reading and writing nodes, tags, edges and synonyms.

use crate::analyze::Analyzer;
use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The current time in UTC, to the millisecond, so that two changes in the
/// same second still sort in the order they happened.
pub const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%fZ','now')";
pub const TODO_STATUSES: [&str; 3] = ["open", "done", "dropped"];
pub const RELATIONS: [&str; 5] = [
    "belongs_to",
    "relates_to",
    "supersedes",
    "depends_on",
    "derived_from",
];

/// Column list for a node row. Use with [`FROM`]; read with [`node_from_row`].
pub const COLS: &str = "n.id, n.type, n.title, n.body, n.status, n.project_id, p.title, \
    n.created_at, n.updated_at, n.deleted_at, \
    (SELECT group_concat(tag, ',') FROM tags WHERE node_id = n.id), \
    n.source, n.author, n.origin, n.visibility, n.rev";
/// Number of columns in [`COLS`]; extra columns in a query start here.
pub const NCOLS: usize = 16;
pub const FROM: &str = "FROM nodes n LEFT JOIN nodes p ON p.id = n.project_id";

fn default_origin() -> String {
    "local".to_string()
}
fn default_visibility() -> String {
    "private".to_string()
}
fn default_rev() -> i64 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    /// The project's name. Filled in on read; ignored on import.
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub deleted_at: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default = "default_origin")]
    pub origin: String,
    #[serde(default = "default_visibility")]
    pub visibility: String,
    #[serde(default = "default_rev")]
    pub rev: i64,
}

impl Node {
    pub fn short(&self) -> String {
        short_id(&self.id)
    }

    /// The name of the project this node is in. A project node is its own.
    pub fn project_name(&self) -> &str {
        if self.kind == "project" {
            &self.title
        } else {
            self.project.as_deref().unwrap_or("-")
        }
    }
}

/// The short form shown to users: the last 8 characters of the ULID, in
/// lowercase. The end is used, not the start, because the start of a ULID is
/// a timestamp: every node made in the same second shares its first 8
/// characters, while the last 8 are random.
pub fn short_id(id: &str) -> String {
    id[id.len().saturating_sub(8)..].to_lowercase()
}

pub fn node_from_row(row: &Row) -> rusqlite::Result<Node> {
    let tags: Option<String> = row.get(10)?;
    let mut tags: Vec<String> = tags
        .map(|t| t.split(',').map(str::to_string).collect())
        .unwrap_or_default();
    tags.sort();
    Ok(Node {
        id: row.get(0)?,
        kind: row.get(1)?,
        title: row.get(2)?,
        body: row.get(3)?,
        status: row.get(4)?,
        project_id: row.get(5)?,
        project: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        deleted_at: row.get(9)?,
        tags,
        source: row.get(11)?,
        author: row.get(12)?,
        origin: row.get(13)?,
        visibility: row.get(14)?,
        rev: row.get(15)?,
    })
}

/// Who is writing: the session and the person.
pub struct Meta {
    pub source: Option<String>,
    pub author: Option<String>,
}

impl Meta {
    pub fn from_env() -> Self {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        Self {
            source: var("SON_SOURCE"),
            author: var("SON_AUTHOR").or_else(|| var("USER")),
        }
    }
}

pub struct NewNode<'a> {
    pub kind: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub status: Option<&'a str>,
    pub project_id: Option<&'a str>,
    pub tags: &'a [String],
}

/// Run `f` inside one write transaction. The lock is taken at the start, so
/// parallel sessions queue up (see the busy timeout) and never deadlock.
pub fn write<T>(conn: &Connection, f: impl FnOnce() -> Result<T>) -> Result<T> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    match f() {
        Ok(value) => {
            conn.execute_batch("COMMIT")?;
            Ok(value)
        }
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(err)
        }
    }
}

/// Turn what the user typed into a full id. Accepts the full ULID or any
/// unique run of 4 or more characters from its end, in either case.
pub fn resolve_id(conn: &Connection, input: &str) -> Result<String> {
    let needle = input.trim().trim_matches('`').to_uppercase();
    if needle.len() < 4 || !needle.chars().all(|c| c.is_ascii_alphanumeric()) {
        bail!("'{input}' is not an id (use the 8-character id that son prints)");
    }
    let mut stmt =
        conn.prepare("SELECT id, title FROM nodes WHERE id LIKE '%' || ?1 ORDER BY id LIMIT 6")?;
    let hits: Vec<(String, String)> = stmt
        .query_map([&needle], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    match hits.as_slice() {
        [] => bail!("no node has an id ending in '{input}'"),
        [(id, _)] => Ok(id.clone()),
        many => {
            let list: Vec<String> = many
                .iter()
                .map(|(id, title)| format!("  {}  {title}", id.to_lowercase()))
                .collect();
            bail!(
                "'{input}' matches more than one node; use more characters:\n{}",
                list.join("\n")
            )
        }
    }
}

pub fn get_node(conn: &Connection, id: &str) -> Result<Node> {
    let sql = format!("SELECT {COLS} {FROM} WHERE n.id = ?1");
    Ok(conn.query_row(&sql, [id], node_from_row)?)
}

pub fn find_project(conn: &Connection, name: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT id FROM nodes WHERE type = 'project' AND deleted_at IS NULL \
             AND title = ?1 COLLATE NOCASE ORDER BY id LIMIT 1",
            [name],
            |r| r.get(0),
        )
        .optional()?)
}

/// The id of the project called `name`, creating the project node if needed.
pub fn ensure_project(conn: &Connection, an: &Analyzer, meta: &Meta, name: &str) -> Result<String> {
    if let Some(id) = find_project(conn, name)? {
        return Ok(id);
    }
    insert_node(
        conn,
        an,
        meta,
        &NewNode {
            kind: "project",
            title: name,
            body: "",
            status: None,
            project_id: None,
            tags: &[],
        },
    )
}

pub fn insert_node(conn: &Connection, an: &Analyzer, meta: &Meta, new: &NewNode) -> Result<String> {
    let id = ulid::Ulid::generate().to_string();
    conn.execute(
        &format!(
            "INSERT INTO nodes (id, type, title, body, status, project_id, created_at, updated_at, source, author) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, {NOW}, {NOW}, ?7, ?8)"
        ),
        params![
            id,
            new.kind,
            new.title,
            new.body,
            new.status,
            new.project_id,
            meta.source,
            meta.author
        ],
    )?;
    set_tags(conn, &id, new.tags)?;
    reindex(conn, an, &id)?;
    Ok(id)
}

/// Save every editable field of `node`, and raise its revision.
pub fn update_node(conn: &Connection, an: &Analyzer, node: &Node) -> Result<()> {
    conn.execute(
        &format!(
            "UPDATE nodes SET type = ?2, title = ?3, body = ?4, status = ?5, project_id = ?6, \
             updated_at = {NOW}, rev = rev + 1 WHERE id = ?1"
        ),
        params![
            node.id,
            node.kind,
            node.title,
            node.body,
            node.status,
            node.project_id
        ],
    )?;
    set_tags(conn, &node.id, &node.tags)?;
    reindex(conn, an, &node.id)
}

pub fn set_status(conn: &Connection, id: &str, status: &str) -> Result<()> {
    conn.execute(
        &format!("UPDATE nodes SET status = ?2, updated_at = {NOW}, rev = rev + 1 WHERE id = ?1"),
        params![id, status],
    )?;
    Ok(())
}

/// Lowercase, trimmed, without blanks or repeats.
pub fn clean_tags(tags: &[String]) -> Vec<String> {
    let mut clean: Vec<String> = tags
        .iter()
        .map(|t| t.trim().trim_start_matches('#').to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    clean.sort();
    clean.dedup();
    clean
}

pub fn set_tags(conn: &Connection, id: &str, tags: &[String]) -> Result<()> {
    conn.execute("DELETE FROM tags WHERE node_id = ?1", [id])?;
    for tag in clean_tags(tags) {
        conn.execute(
            "INSERT OR IGNORE INTO tags (node_id, tag) VALUES (?1, ?2)",
            params![id, tag],
        )?;
    }
    Ok(())
}

/// Rebuild the search row for one node. A dropped node has no search row.
pub fn reindex(conn: &Connection, an: &Analyzer, id: &str) -> Result<()> {
    conn.execute("DELETE FROM nodes_fts WHERE node_id = ?1", [id])?;
    let row: Option<(String, String, Option<String>)> = conn
        .query_row(
            "SELECT title, body, (SELECT group_concat(tag, ' ') FROM tags WHERE node_id = n.id) \
             FROM nodes n WHERE id = ?1 AND deleted_at IS NULL",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((title, body, tags)) = row {
        conn.execute(
            "INSERT INTO nodes_fts (node_id, title, tags, body) VALUES (?1, ?2, ?3, ?4)",
            params![
                id,
                an.index_text(&title),
                an.index_text(&tags.unwrap_or_default()),
                an.index_text(&body)
            ],
        )?;
    }
    Ok(())
}

pub fn reindex_all(conn: &Connection, an: &Analyzer) -> Result<usize> {
    let ids: Vec<String> = conn
        .prepare("SELECT id FROM nodes")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    conn.execute("DELETE FROM nodes_fts", [])?;
    for id in &ids {
        reindex(conn, an, id)?;
    }
    Ok(ids.len())
}

pub fn live_children(conn: &Connection, project_id: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT count(*) FROM nodes WHERE project_id = ?1 AND deleted_at IS NULL",
        [project_id],
        |r| r.get(0),
    )?)
}

pub fn soft_drop(conn: &Connection, an: &Analyzer, id: &str) -> Result<()> {
    conn.execute(
        &format!(
            "UPDATE nodes SET deleted_at = {NOW}, updated_at = {NOW}, rev = rev + 1 \
             WHERE id = ?1 AND deleted_at IS NULL"
        ),
        [id],
    )?;
    reindex(conn, an, id)
}

pub fn restore(conn: &Connection, an: &Analyzer, id: &str) -> Result<()> {
    conn.execute(
        &format!(
            "UPDATE nodes SET deleted_at = NULL, updated_at = {NOW}, rev = rev + 1 WHERE id = ?1"
        ),
        [id],
    )?;
    reindex(conn, an, id)
}

/// Remove a node for good, with its tags and edges.
pub fn purge(conn: &Connection, id: &str) -> Result<()> {
    let children: i64 = conn.query_row(
        "SELECT count(*) FROM nodes WHERE project_id = ?1",
        [id],
        |r| r.get(0),
    )?;
    if children > 0 {
        bail!("this project still holds {children} node(s); purge or move them first");
    }
    conn.execute("DELETE FROM edges WHERE from_id = ?1 OR to_id = ?1", [id])?;
    conn.execute("DELETE FROM tags WHERE node_id = ?1", [id])?;
    conn.execute("DELETE FROM nodes_fts WHERE node_id = ?1", [id])?;
    conn.execute("DELETE FROM nodes WHERE id = ?1", [id])?;
    Ok(())
}

/// Add an edge. Returns false when it was already there.
pub fn link(conn: &Connection, from: &str, relation: &str, to: &str) -> Result<bool> {
    if !RELATIONS.contains(&relation) {
        bail!(
            "unknown relation '{relation}' (use one of: {})",
            RELATIONS.join(", ")
        );
    }
    if from == to {
        bail!("a node cannot link to itself");
    }
    let added = conn.execute(
        &format!(
            "INSERT OR IGNORE INTO edges (from_id, to_id, relation, created_at) VALUES (?1, ?2, ?3, {NOW})"
        ),
        params![from, to, relation],
    )?;
    Ok(added > 0)
}

/// Remove an edge. Returns false when there was none.
pub fn unlink(conn: &Connection, from: &str, relation: &str, to: &str) -> Result<bool> {
    let removed = conn.execute(
        "DELETE FROM edges WHERE from_id = ?1 AND to_id = ?2 AND relation = ?3",
        params![from, to, relation],
    )?;
    Ok(removed > 0)
}

/// One edge seen from a node: the node at the other end and the direction.
pub struct Edge {
    pub relation: String,
    pub outgoing: bool,
    pub node: Node,
}

impl Edge {
    /// The relation as read from this side: an incoming `supersedes` reads
    /// as `superseded_by`.
    pub fn label(&self) -> &str {
        if self.outgoing {
            return &self.relation;
        }
        match self.relation.as_str() {
            "supersedes" => "superseded_by",
            "depends_on" => "needed_by",
            "derived_from" => "source_of",
            "belongs_to" => "has",
            other => other,
        }
    }
}

/// Edges in both directions whose other end is not dropped.
pub fn edges_of(conn: &Connection, id: &str) -> Result<Vec<Edge>> {
    let sql = format!(
        "SELECT {COLS}, e.relation, e.outgoing FROM ( \
             SELECT to_id AS other, relation, 1 AS outgoing, created_at FROM edges WHERE from_id = ?1 \
             UNION ALL \
             SELECT from_id, relation, 0, created_at FROM edges WHERE to_id = ?1 \
         ) e JOIN nodes n ON n.id = e.other LEFT JOIN nodes p ON p.id = n.project_id \
         WHERE n.deleted_at IS NULL ORDER BY e.created_at, n.id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let edges = stmt
        .query_map([id], |r| {
            Ok(Edge {
                node: node_from_row(r)?,
                relation: r.get(NCOLS)?,
                outgoing: r.get(NCOLS + 1)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(edges)
}

/// The live node that supersedes `id`, if any.
pub fn superseded_by(conn: &Connection, id: &str) -> Result<Option<Node>> {
    let sql = format!(
        "SELECT {COLS} FROM edges e JOIN nodes n ON n.id = e.from_id \
         LEFT JOIN nodes p ON p.id = n.project_id \
         WHERE e.to_id = ?1 AND e.relation = 'supersedes' AND n.deleted_at IS NULL \
         ORDER BY e.created_at DESC LIMIT 1"
    );
    Ok(conn.query_row(&sql, [id], node_from_row).optional()?)
}

/// Nodes, newest first. `project_id` keeps one project (and the project node
/// itself); `kind` keeps one type.
pub fn list_nodes(
    conn: &Connection,
    project_id: Option<&str>,
    kind: Option<&str>,
    include_dropped: bool,
) -> Result<Vec<Node>> {
    let sql = format!(
        "SELECT {COLS} {FROM} \
         WHERE (?1 IS NULL OR n.project_id = ?1 OR n.id = ?1) \
           AND (?2 IS NULL OR n.type = ?2) \
           AND (?3 OR n.deleted_at IS NULL) \
         ORDER BY n.updated_at DESC, n.id DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let nodes = stmt
        .query_map(params![project_id, kind, include_dropped], node_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(nodes)
}

#[derive(Serialize)]
pub struct ProjectRow {
    #[serde(flatten)]
    pub node: Node,
    pub nodes: i64,
    pub open_todos: i64,
}

pub fn list_projects(conn: &Connection) -> Result<Vec<ProjectRow>> {
    let sql = format!(
        "SELECT {COLS}, \
           (SELECT count(*) FROM nodes c WHERE c.project_id = n.id AND c.deleted_at IS NULL), \
           (SELECT count(*) FROM nodes c WHERE c.project_id = n.id AND c.deleted_at IS NULL \
              AND c.type = 'todo' AND c.status = 'open') \
         {FROM} WHERE n.type = 'project' AND n.deleted_at IS NULL \
         ORDER BY n.title COLLATE NOCASE"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map([], |r| {
            Ok(ProjectRow {
                node: node_from_row(r)?,
                nodes: r.get(NCOLS)?,
                open_todos: r.get(NCOLS + 1)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// The synonym table as term -> canonical.
pub fn synonyms(conn: &Connection) -> Result<HashMap<String, String>> {
    let mut stmt = conn.prepare("SELECT term, canonical FROM synonyms")?;
    let map = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(map)
}

pub fn add_synonym(conn: &Connection, term: &str, canonical: &str) -> Result<()> {
    let term = term.trim().to_lowercase();
    let canonical = canonical.trim().to_lowercase();
    if term.is_empty() || canonical.is_empty() {
        bail!("a synonym needs a term and a canonical word");
    }
    if term == canonical {
        bail!("'{term}' is already its own canonical word");
    }
    conn.execute(
        "INSERT INTO synonyms (term, canonical) VALUES (?1, ?2) \
         ON CONFLICT(term) DO UPDATE SET canonical = excluded.canonical",
        params![term, canonical],
    )?;
    Ok(())
}

pub fn remove_synonym(conn: &Connection, term: &str) -> Result<bool> {
    let removed = conn.execute(
        "DELETE FROM synonyms WHERE term = ?1",
        [term.trim().to_lowercase()],
    )?;
    Ok(removed > 0)
}
