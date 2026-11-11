//! Recall: analyze the query, rank with BM25, adjust for recency, type and
//! superseded state, put the current project first, then walk the graph.

use crate::analyze::Analyzer;
use crate::store::{self, Node, COLS, NCOLS};
use anyhow::{bail, Result};
use rusqlite::{params, Connection};
use std::collections::HashSet;

/// BM25 column weights: title counts most, then tags, then body.
const WEIGHTS: &str = "0.0, 10.0, 5.0, 1.0";
/// A node's score gains up to this share when it was just updated...
const RECENCY_BOOST: f64 = 0.25;
/// ...and the gain halves every this many days.
const RECENCY_HALF_LIFE_DAYS: f64 = 90.0;
const TYPE_HINT_BOOST: f64 = 2.5;
/// Patterns and decisions are the answer to "how do we do this", so they
/// start a little ahead of todos, facts and notes.
const PATTERN_PRIOR: f64 = 1.3;
const DECISION_PRIOR: f64 = 1.15;
/// BM25 favours short text, so a one-line todo can beat a full pattern that
/// names every word of the query in its title. These two put that right: a
/// node gains for each query word its title (or tags) covers.
const TITLE_COVERAGE_BOOST: f64 = 1.0;
const TAG_COVERAGE_BOOST: f64 = 0.5;
const SUPERSEDED_PENALTY: f64 = 0.4;
const CLOSED_TODO_PENALTY: f64 = 0.5;
const MAX_NEIGHBORS: usize = 20;
const MAX_ALSO: usize = 8;

pub struct Options<'a> {
    /// Keep only this node type.
    pub kind: Option<&'a str>,
    /// Keep only this project (id).
    pub only_project: Option<&'a str>,
    /// The project whose matches come first (id).
    pub current_project: Option<&'a str>,
    pub limit: usize,
    /// How far to walk edges from the hits: 0, 1 or 2.
    pub hops: u8,
}

pub struct Hit {
    pub node: Node,
    pub score: f64,
    /// True when the node is in the current project.
    pub current: bool,
    /// Short id of the node that supersedes this one.
    pub superseded_by: Option<String>,
}

pub struct Neighbor {
    pub node: Node,
    /// The relation as read from the node it was reached from.
    pub relation: String,
    /// Short id of the node it was reached from.
    pub of: String,
}

pub struct Recall {
    pub hits: Vec<Hit>,
    pub neighbors: Vec<Neighbor>,
    /// Patterns, decisions and open todos from the hits' projects.
    pub also: Vec<Node>,
}

pub fn recall(conn: &Connection, an: &Analyzer, terms: &str, opts: &Options) -> Result<Recall> {
    let query = an.query(terms, &store::synonyms(conn)?);
    let Some(fts) = query.fts() else {
        bail!("nothing to search for in '{terms}'");
    };

    let sql = format!(
        "SELECT {COLS}, bm25(nodes_fts, {WEIGHTS}), \
                julianday('now') - julianday(n.updated_at), \
                EXISTS (SELECT 1 FROM edges e JOIN nodes s ON s.id = e.from_id \
                        WHERE e.to_id = n.id AND e.relation = 'supersedes' AND s.deleted_at IS NULL) \
         FROM nodes_fts JOIN nodes n ON n.id = nodes_fts.node_id \
         LEFT JOIN nodes p ON p.id = n.project_id \
         WHERE nodes_fts MATCH ?1 AND n.deleted_at IS NULL \
           AND (?2 IS NULL OR n.type = ?2) \
           AND (?3 IS NULL OR n.project_id = ?3 OR n.id = ?3) \
         ORDER BY 17 LIMIT ?4"
    );
    // Take more candidates than asked for: the adjustments below reorder them.
    let candidates = (opts.limit.max(1) * 10).max(50) as i64;
    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<(Node, f64, Option<f64>, bool)> = stmt
        .query_map(
            params![fts, opts.kind, opts.only_project, candidates],
            |r| {
                Ok((
                    store::node_from_row(r)?,
                    r.get(NCOLS)?,
                    r.get(NCOLS + 1)?,
                    r.get(NCOLS + 2)?,
                ))
            },
        )?
        .collect::<rusqlite::Result<_>>()?;

    let mut hits = Vec::with_capacity(rows.len());
    for (node, bm25, age_days, superseded) in rows {
        // FTS5 gives better matches a more negative bm25.
        let mut score = -bm25;
        let age = age_days.unwrap_or(0.0).max(0.0);
        score *= 1.0 + RECENCY_BOOST * 0.5f64.powf(age / RECENCY_HALF_LIFE_DAYS);
        score *= 1.0
            + TITLE_COVERAGE_BOOST * coverage(&query.groups, &an.tokens(&node.title))
            + TAG_COVERAGE_BOOST * coverage(&query.groups, &an.tokens(&node.tags.join(" ")));
        score *= match node.kind.as_str() {
            "pattern" => PATTERN_PRIOR,
            "decision" => DECISION_PRIOR,
            _ => 1.0,
        };
        if opts.kind.is_none() && query.type_hint == Some(node.kind.as_str()) {
            score *= TYPE_HINT_BOOST;
        }
        if superseded {
            score *= SUPERSEDED_PENALTY;
        }
        if node.kind == "todo" && node.status.as_deref() != Some("open") {
            score *= CLOSED_TODO_PENALTY;
        }
        let current = opts.current_project.is_some_and(|current| {
            node.project_id.as_deref() == Some(current) || node.id == current
        });
        let superseded_by = if superseded {
            store::superseded_by(conn, &node.id)?.map(|n| n.short())
        } else {
            None
        };
        hits.push(Hit {
            node,
            score,
            current,
            superseded_by,
        });
    }
    hits.sort_by(|a, b| {
        b.current
            .cmp(&a.current)
            .then(b.score.total_cmp(&a.score))
            .then_with(|| b.node.id.cmp(&a.node.id))
    });
    hits.truncate(opts.limit.max(1));

    let mut seen: HashSet<String> = hits.iter().map(|h| h.node.id.clone()).collect();
    let neighbors = expand(conn, &hits, opts.hops, &mut seen)?;
    let also = if opts.hops > 0 {
        also_in_projects(conn, &hits, &seen)?
    } else {
        Vec::new()
    };
    Ok(Recall {
        hits,
        neighbors,
        also,
    })
}

/// The share of the query's words that appear in `tokens`, from 0 to 1.
fn coverage(groups: &[Vec<String>], tokens: &[String]) -> f64 {
    if groups.is_empty() {
        return 0.0;
    }
    let covered = groups
        .iter()
        .filter(|group| group.iter().any(|term| tokens.contains(term)))
        .count();
    covered as f64 / groups.len() as f64
}

/// Walk edges outward from the hits, one hop at a time.
fn expand(
    conn: &Connection,
    hits: &[Hit],
    hops: u8,
    seen: &mut HashSet<String>,
) -> Result<Vec<Neighbor>> {
    let mut neighbors = Vec::new();
    let mut frontier: Vec<String> = hits.iter().map(|h| h.node.id.clone()).collect();
    for _ in 0..hops {
        let mut next = Vec::new();
        for id in &frontier {
            for edge in store::edges_of(conn, id)? {
                if neighbors.len() >= MAX_NEIGHBORS {
                    return Ok(neighbors);
                }
                if !seen.insert(edge.node.id.clone()) {
                    continue;
                }
                next.push(edge.node.id.clone());
                neighbors.push(Neighbor {
                    relation: edge.label().to_string(),
                    of: store::short_id(id),
                    node: edge.node,
                });
            }
        }
        frontier = next;
    }
    Ok(neighbors)
}

/// What else the hits' projects hold: the patterns, decisions and open todos
/// that a session would want next to a keyword match.
fn also_in_projects(conn: &Connection, hits: &[Hit], seen: &HashSet<String>) -> Result<Vec<Node>> {
    let mut projects: Vec<&str> = Vec::new();
    for hit in hits {
        let project = if hit.node.kind == "project" {
            Some(hit.node.id.as_str())
        } else {
            hit.node.project_id.as_deref()
        };
        if let Some(project) = project {
            if !projects.contains(&project) {
                projects.push(project);
            }
        }
    }
    let mut also = Vec::new();
    for project in projects {
        for node in store::list_nodes(conn, Some(project), None, false)? {
            if also.len() >= MAX_ALSO {
                return Ok(also);
            }
            let wanted = match node.kind.as_str() {
                "pattern" | "decision" => true,
                "todo" => node.status.as_deref() == Some("open"),
                _ => false,
            };
            if !wanted || seen.contains(&node.id) {
                continue;
            }
            if node.kind == "decision" && store::superseded_by(conn, &node.id)?.is_some() {
                continue;
            }
            also.push(node);
        }
    }
    Ok(also)
}
