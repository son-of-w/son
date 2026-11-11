//! Change history and undo.
//!
//! Every command that changes nodes or links is one **operation**. An
//! operation keeps, for each node it changed, the node as it was before and
//! as it was after, and each link it added or removed. Nothing is pruned.
//!
//! `son undo` reverses one operation. It first checks that every node and
//! link the operation touched is still as the operation left it. If
//! something changed since, it reports a conflict and changes nothing. The
//! reversal is itself a new operation, so history only ever grows.

use crate::activity::Page;
use crate::analyze::Analyzer;
use crate::error::coded;
use crate::store::{self, Meta, Node, NOW};
use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

/// A link that an operation added or removed.
#[derive(Debug, Clone, Serialize)]
pub struct EdgeChange {
    #[serde(rename = "from")]
    pub from_id: String,
    #[serde(rename = "to")]
    pub to_id: String,
    pub relation: String,
    pub added: bool,
    #[serde(skip)]
    pub created_at: String,
    #[serde(skip)]
    pub origin: String,
}

/// True when two states of a node hold the same content. The revision
/// number, the time of the last change and the collection's name (which is
/// looked up, not stored) are not content.
pub fn same_content(a: &Node, b: &Node) -> bool {
    a.kind == b.kind
        && a.title == b.title
        && a.body == b.body
        && a.status == b.status
        && a.project_id == b.project_id
        && a.tags == b.tags
        && a.deleted_at == b.deleted_at
        && a.evidence == b.evidence
        && a.verified_at == b.verified_at
        && a.verified_by == b.verified_by
        && a.verified_note == b.verified_note
        && a.refs == b.refs
        && a.merged_into == b.merged_into
        && a.stale_at == b.stale_at
        && a.stale_note == b.stale_note
}

/// Collects what one operation changes, then writes it down. Use inside
/// [`store::write`], so that the record and the change commit together or
/// not at all.
pub struct Recorder<'a> {
    conn: &'a Connection,
    kind: &'a str,
    nodes: Vec<(String, Option<Node>)>,
    edges: Vec<EdgeChange>,
    undoes: Option<i64>,
    /// Nodes that belong in the record even when their content is the same
    /// after as before.
    always: Vec<String>,
}

impl<'a> Recorder<'a> {
    pub fn new(conn: &'a Connection, kind: &'a str) -> Self {
        Self {
            conn,
            kind,
            nodes: Vec::new(),
            edges: Vec::new(),
            undoes: None,
            always: Vec::new(),
        }
    }

    /// Like [`Recorder::touch`], and the node is part of the record even
    /// if the operation leaves its content the same. A merge uses this for
    /// the survivor, so that the merge is always in the survivor's history
    /// and can be undone from it.
    pub fn touch_always(&mut self, id: &str) -> Result<()> {
        self.always.push(id.to_string());
        self.touch(id)
    }

    /// Call before changing the node `id`: keeps the node as it is now. For
    /// a node that does not exist yet, call it before or after the insert.
    pub fn touch(&mut self, id: &str) -> Result<()> {
        if !self.nodes.iter().any(|(seen, _)| seen == id) {
            self.nodes
                .push((id.to_string(), store::find_node(self.conn, id)?));
        }
        Ok(())
    }

    /// The node `id` was made by this operation.
    pub fn created(&mut self, id: &str) {
        self.nodes.push((id.to_string(), None));
    }

    /// Add a link and note it. False when the link was already there.
    pub fn link(&mut self, from: &str, relation: &str, to: &str) -> Result<bool> {
        let added = store::link(self.conn, from, relation, to)?;
        if added {
            let (created_at, origin) = edge_row(self.conn, from, relation, to)?
                .ok_or_else(|| anyhow!("the new link is missing"))?;
            self.edges.push(EdgeChange {
                from_id: from.to_string(),
                to_id: to.to_string(),
                relation: relation.to_string(),
                added: true,
                created_at,
                origin,
            });
        }
        Ok(added)
    }

    /// Remove a link and note it. False when there was no such link.
    pub fn unlink(&mut self, from: &str, relation: &str, to: &str) -> Result<bool> {
        let Some((created_at, origin)) = edge_row(self.conn, from, relation, to)? else {
            return Ok(false);
        };
        store::unlink(self.conn, from, relation, to)?;
        self.edges.push(EdgeChange {
            from_id: from.to_string(),
            to_id: to.to_string(),
            relation: relation.to_string(),
            added: false,
            created_at,
            origin,
        });
        Ok(true)
    }

    /// Put back a link exactly as it was, with its first creation time.
    fn relink(&mut self, change: &EdgeChange) -> Result<()> {
        self.conn.execute(
            "INSERT INTO edges (from_id, to_id, relation, created_at, origin) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                change.from_id,
                change.to_id,
                change.relation,
                change.created_at,
                change.origin
            ],
        )?;
        self.edges.push(EdgeChange {
            added: true,
            ..change.clone()
        });
        Ok(())
    }

    /// A link was added by other code (an import); note it.
    pub fn linked(&mut self, change: EdgeChange) {
        self.edges.push(change);
    }

    /// Write the operation down. `None` when nothing changed: an operation
    /// that changed nothing leaves no record.
    pub fn finish(self, meta: &Meta) -> Result<Option<i64>> {
        let mut changed: Vec<(String, Option<Node>, Node)> = Vec::new();
        for (id, before) in self.nodes {
            let Some(after) = store::find_node(self.conn, &id)? else {
                continue;
            };
            if before.as_ref().is_some_and(|b| same_content(b, &after))
                && !self.always.contains(&id)
            {
                continue;
            }
            changed.push((id, before, after));
        }
        // A link added and removed again by the same operation is no change.
        let mut edges: Vec<EdgeChange> = Vec::new();
        for change in self.edges {
            let same = |e: &EdgeChange| {
                e.from_id == change.from_id
                    && e.to_id == change.to_id
                    && e.relation == change.relation
            };
            match edges.iter().position(same) {
                Some(at) if edges[at].added != change.added => {
                    edges.remove(at);
                }
                Some(at) => edges[at] = change,
                None => edges.push(change),
            }
        }
        if changed.is_empty() && edges.is_empty() {
            return Ok(None);
        }
        self.conn.execute(
            &format!(
                "INSERT INTO ops (at, kind, source, author, undoes) VALUES ({NOW}, ?1, ?2, ?3, ?4)"
            ),
            params![self.kind, meta.source, meta.author, self.undoes],
        )?;
        let op = self.conn.last_insert_rowid();
        for (id, before, after) in &changed {
            let before = before.as_ref().map(serde_json::to_string).transpose()?;
            self.conn.execute(
                "INSERT INTO revisions (op_id, node_id, before, after) VALUES (?1, ?2, ?3, ?4)",
                params![op, id, before, serde_json::to_string(after)?],
            )?;
        }
        for edge in &edges {
            self.conn.execute(
                "INSERT INTO edge_changes (op_id, from_id, to_id, relation, added, created_at, origin) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    op,
                    edge.from_id,
                    edge.to_id,
                    edge.relation,
                    edge.added,
                    edge.created_at,
                    edge.origin
                ],
            )?;
        }
        Ok(Some(op))
    }
}

fn edge_row(
    conn: &Connection,
    from: &str,
    relation: &str,
    to: &str,
) -> Result<Option<(String, String)>> {
    Ok(conn
        .query_row(
            "SELECT created_at, origin FROM edges WHERE from_id = ?1 AND to_id = ?2 AND relation = ?3",
            params![from, to, relation],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

/// The id of the collection called `name`. When it has to be made, that is
/// an operation of its own, so that undoing the command that needed it does
/// not take the collection away from other nodes.
pub fn ensure_collection(
    conn: &Connection,
    an: &Analyzer,
    meta: &Meta,
    name: &str,
) -> Result<String> {
    if let Some(id) = store::find_project(conn, name)? {
        return Ok(id);
    }
    let id = store::ensure_project(conn, an, meta, name)?;
    let mut rec = Recorder::new(conn, "create");
    rec.created(&id);
    rec.finish(meta)?;
    Ok(id)
}

/// Remove a node for good, with its history. An operation that also
/// changed other nodes (a merge, an import) keeps its record of those, but
/// is marked: it can no longer be undone, because part of it is gone.
pub fn purge(conn: &Connection, id: &str) -> Result<()> {
    store::purge(conn, id)?;
    let ops: Vec<i64> = conn
        .prepare(
            "SELECT op_id FROM revisions WHERE node_id = ?1 \
             UNION SELECT op_id FROM edge_changes WHERE from_id = ?1 OR to_id = ?1",
        )?
        .query_map([id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    conn.execute("DELETE FROM revisions WHERE node_id = ?1", [id])?;
    conn.execute(
        "DELETE FROM edge_changes WHERE from_id = ?1 OR to_id = ?1",
        [id],
    )?;
    for op in ops {
        let left: i64 = conn.query_row(
            "SELECT (SELECT count(*) FROM revisions WHERE op_id = ?1) \
                  + (SELECT count(*) FROM edge_changes WHERE op_id = ?1)",
            [op],
            |r| r.get(0),
        )?;
        if left == 0 {
            conn.execute("DELETE FROM ops WHERE id = ?1", [op])?;
        } else {
            conn.execute("UPDATE ops SET purged = 1 WHERE id = ?1", [op])?;
        }
    }
    // The activity log names what a command acted on: forget that too.
    conn.execute(
        "UPDATE events SET detail = NULL, node_id = NULL WHERE node_id = ?1",
        [id],
    )?;
    Ok(())
}

/// One operation as seen from one node.
#[derive(Serialize)]
pub struct Entry {
    pub op: i64,
    /// Local time.
    pub at: String,
    pub kind: String,
    pub source: Option<String>,
    pub author: Option<String>,
    /// For an undo: the operation it reversed.
    pub undoes: Option<i64>,
    /// The undo that reversed this operation, if any.
    pub undone_by: Option<i64>,
    /// True when part of this operation's record was purged.
    pub purged: bool,
    /// The node before the operation; `None` when the operation made it,
    /// or when the operation changed only the node's links.
    pub before: Option<Node>,
    pub after: Option<Node>,
    pub links: Vec<EdgeChange>,
    /// How many other nodes the operation changed.
    pub other_nodes: i64,
}

const ENTRY: &str =
    "SELECT o.id, strftime('%Y-%m-%d %H:%M:%S', o.at, 'localtime'), o.kind, o.source, \
    o.author, o.undoes, o.undone_by, o.purged, r.before, r.after, \
    (SELECT count(*) FROM revisions x WHERE x.op_id = o.id AND x.node_id != ?1) \
    FROM ops o LEFT JOIN revisions r ON r.op_id = o.id AND r.node_id = ?1";
const TOUCHES: &str = "o.id IN (SELECT op_id FROM revisions WHERE node_id = ?1 \
    UNION SELECT op_id FROM edge_changes WHERE from_id = ?1 OR to_id = ?1)";

fn parse_node(json: Option<String>) -> Option<Node> {
    json.and_then(|json| serde_json::from_str(&json).ok())
}

fn entry_from_row(r: &rusqlite::Row) -> rusqlite::Result<Entry> {
    Ok(Entry {
        op: r.get(0)?,
        at: r.get(1)?,
        kind: r.get(2)?,
        source: r.get(3)?,
        author: r.get(4)?,
        undoes: r.get(5)?,
        undone_by: r.get(6)?,
        purged: r.get(7)?,
        before: parse_node(r.get(8)?),
        after: parse_node(r.get(9)?),
        links: Vec::new(),
        other_nodes: r.get(10)?,
    })
}

fn edge_changes(conn: &Connection, op: i64, node: Option<&str>) -> Result<Vec<EdgeChange>> {
    let mut stmt = conn.prepare(
        "SELECT from_id, to_id, relation, added, created_at, origin FROM edge_changes \
         WHERE op_id = ?1 AND (?2 IS NULL OR from_id = ?2 OR to_id = ?2) ORDER BY id",
    )?;
    let rows = stmt
        .query_map(params![op, node], |r| {
            Ok(EdgeChange {
                from_id: r.get(0)?,
                to_id: r.get(1)?,
                relation: r.get(2)?,
                added: r.get(3)?,
                created_at: r.get(4)?,
                origin: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// The operations that changed a node or its links, newest first.
pub fn entries_page(
    conn: &Connection,
    node_id: &str,
    limit: usize,
    page: usize,
) -> Result<Page<Entry>> {
    let total: i64 = conn.query_row(
        &format!("SELECT count(*) FROM ops o WHERE {TOUCHES}"),
        [node_id],
        |r| r.get(0),
    )?;
    let mut stmt = conn.prepare(&format!(
        "{ENTRY} WHERE {TOUCHES} ORDER BY o.id DESC LIMIT ?2 OFFSET ?3"
    ))?;
    let mut items: Vec<Entry> = stmt
        .query_map(
            params![node_id, limit as i64, (limit * (page - 1)) as i64],
            entry_from_row,
        )?
        .collect::<rusqlite::Result<_>>()?;
    for entry in &mut items {
        entry.links = edge_changes(conn, entry.op, Some(node_id))?;
    }
    Ok(Page::new(items, total, limit, page))
}

/// One operation as seen from one node.
pub fn entry(conn: &Connection, node_id: &str, op: i64) -> Result<Entry> {
    let mut entry = conn
        .query_row(
            &format!("{ENTRY} WHERE o.id = ?2 AND {TOUCHES}"),
            params![node_id, op],
            entry_from_row,
        )
        .optional()?
        .ok_or_else(|| {
            anyhow!(
                "operation #{op} did not change `{}`",
                store::short_id(node_id)
            )
        })?;
    entry.links = edge_changes(conn, op, Some(node_id))?;
    Ok(entry)
}

/// What an undo did.
pub struct Undone {
    /// The new operation: the undo itself.
    pub op: i64,
    /// The operation it reversed.
    pub undid: i64,
    pub kind: String,
    /// The nodes it changed, as they are now.
    pub nodes: Vec<Node>,
    pub links: usize,
}

fn describe(node: &Node) -> String {
    format!("`{}` {}", node.short(), node.title)
}

/// Reverse one operation that changed `node_id`: the one numbered `op`, or
/// else the newest one that is not an undo and was not undone. All of the
/// operation is reversed (every node and link it changed), or none of it.
pub fn undo(
    conn: &Connection,
    an: &Analyzer,
    meta: &Meta,
    node_id: &str,
    op: Option<i64>,
) -> Result<Undone> {
    let short = store::short_id(node_id);
    let target: i64 = match op {
        Some(op) => entry(conn, node_id, op)?.op,
        None => conn
            .query_row(
                &format!(
                    "SELECT o.id FROM ops o WHERE {TOUCHES} AND o.undone_by IS NULL \
                     AND o.kind != 'undo' ORDER BY o.id DESC LIMIT 1"
                ),
                [node_id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| anyhow!("nothing to undo for `{short}` (see `son history {short}`)"))?,
    };
    let (kind, undoes, undone_by, purged): (String, Option<i64>, Option<i64>, bool) = conn
        .query_row(
            "SELECT kind, undoes, undone_by, purged FROM ops WHERE id = ?1",
            [target],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
    let name = format!("#{target} ({kind})");
    if let Some(by) = undone_by {
        return Err(coded(
            "conflict",
            format!("{name} was already undone by #{by}"),
        ));
    }
    if purged {
        return Err(coded(
            "unavailable",
            format!(
                "cannot undo {name}: a node it changed was purged, so restoration is unavailable. \
                 Nothing was changed"
            ),
        ));
    }

    let revisions: Vec<(String, Option<Node>, Node)> = conn
        .prepare("SELECT node_id, before, after FROM revisions WHERE op_id = ?1 ORDER BY id")?
        .query_map([target], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(|(id, before, after)| {
            let before = before
                .map(|json| serde_json::from_str(&json))
                .transpose()
                .context("a stored revision cannot be read")?;
            let after = serde_json::from_str(&after).context("a stored revision cannot be read")?;
            Ok((id, before, after))
        })
        .collect::<Result<_>>()?;
    let edges = edge_changes(conn, target, None)?;

    // First check everything. Nothing is written until all of it is safe.
    let conflict = |what: String| {
        coded(
            "conflict",
            format!("cannot undo {name}: {what}. Nothing was changed (see `son history {short}`)"),
        )
    };
    let unavailable = |what: String| {
        coded(
            "unavailable",
            format!(
                "cannot undo {name}: {what}, so restoration is unavailable. Nothing was changed"
            ),
        )
    };
    // The nodes that this undo will leave dropped.
    let will_drop: Vec<&str> = revisions
        .iter()
        .filter(|(_, before, _)| before.as_ref().is_none_or(|b| b.deleted_at.is_some()))
        .map(|(id, _, _)| id.as_str())
        .collect();
    for (id, before, after) in &revisions {
        let Some(now) = store::find_node(conn, id)? else {
            return Err(unavailable(format!("{} was purged", describe(after))));
        };
        if !same_content(&now, after) {
            return Err(conflict(format!("{} was changed after it", describe(&now))));
        }
        // What the node will be once this is undone.
        let dropped = before.as_ref().is_none_or(|b| b.deleted_at.is_some());
        let live_before = now.deleted_at.is_none();
        if now.kind == "collection" && dropped && live_before {
            let children = conn
                .prepare("SELECT id FROM nodes WHERE project_id = ?1 AND deleted_at IS NULL")?
                .query_map([id], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
                .iter()
                .filter(|child| !will_drop.contains(&child.as_str()))
                .count();
            if children > 0 {
                return Err(conflict(format!(
                    "the collection {} now holds {children} node(s)",
                    describe(&now)
                )));
            }
        }
        if let Some(before) = before {
            if let Some(collection) = &before.project_id {
                if store::find_node(conn, collection)?.is_none() {
                    return Err(unavailable(format!(
                        "the collection that {} was in was purged",
                        describe(before)
                    )));
                }
            }
            if before.kind == "collection" && before.deleted_at.is_none() {
                if let Some(other) = store::find_project(conn, &before.title)? {
                    if other != *id {
                        return Err(conflict(format!(
                            "another collection is now called '{}'",
                            before.title
                        )));
                    }
                }
            }
        }
    }
    for edge in &edges {
        for end in [&edge.from_id, &edge.to_id] {
            if store::find_node(conn, end)?.is_none() {
                return Err(unavailable("a node at one end of a link was purged".into()));
            }
        }
        let exists = edge_row(conn, &edge.from_id, &edge.relation, &edge.to_id)?.is_some();
        if exists != edge.added {
            return Err(conflict(format!(
                "the link `{}` {} `{}` was changed after it",
                store::short_id(&edge.from_id),
                edge.relation,
                store::short_id(&edge.to_id)
            )));
        }
    }

    // Then reverse it, as a new operation.
    let mut rec = Recorder::new(conn, "undo");
    rec.undoes = Some(target);
    for (id, before, _) in &revisions {
        rec.touch(id)?;
        match before {
            Some(before) => store::put_snapshot(conn, an, before)?,
            None => store::soft_drop(conn, an, id)?,
        }
    }
    // Newest change first, so that a link moved twice ends where it began.
    for edge in edges.iter().rev() {
        if edge.added {
            rec.unlink(&edge.from_id, &edge.relation, &edge.to_id)?;
        } else {
            rec.relink(edge)?;
        }
    }
    let op = rec
        .finish(meta)?
        .ok_or_else(|| anyhow!("{name} changed nothing, so there is nothing to undo"))?;
    conn.execute(
        "UPDATE ops SET undone_by = ?2 WHERE id = ?1",
        params![target, op],
    )?;
    if let Some(redone) = undoes {
        // Undoing an undo puts the first operation back in force.
        conn.execute("UPDATE ops SET undone_by = NULL WHERE id = ?1", [redone])?;
    }
    let mut nodes = Vec::new();
    for (id, _, _) in &revisions {
        nodes.push(store::get_node(conn, id)?);
    }
    Ok(Undone {
        op,
        undid: target,
        kind,
        nodes,
        links: edges.len(),
    })
}
