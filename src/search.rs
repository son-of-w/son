//! Recall: analyze the query, rank with BM25, adjust for recency, type and
//! superseded state, put the current project first, then walk the graph.
//!
//! Matching has two passes. The first takes the query's words as given
//! (stemmed, with synonyms). If that finds fewer nodes than asked for, the
//! second also takes words that start with a query word (`terra` finds
//! `terraform`) and stored words a typo away (`terrafrom` finds `terraform`).
//! Loose matches are marked and listed after exact ones.

use crate::analyze::{edit_distance, Analyzer, Query};
use crate::store::{self, Node, COLS, NCOLS};
use anyhow::{bail, Result};
use rusqlite::{params, Connection};
use serde::Serialize;
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
/// A query word this long or longer also matches words that start with it.
const MIN_PREFIX_CHARS: usize = 3;
/// How many near-miss words to try for one mistyped query word.
const MAX_FUZZY_TERMS: usize = 3;
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
    /// True when the node matched only loosely: by the start of a word, or
    /// by a word a typo away from the query. False for an exact match.
    pub close: bool,
    /// Short id of the node that supersedes this one.
    pub superseded_by: Option<String>,
    /// Why the node matched and how its score was made. Filled in by the
    /// ranking itself, so it always agrees with the order.
    pub why: Why,
}

/// One query word found in a node.
#[derive(Debug, Clone, Serialize)]
pub struct WordMatch {
    /// The query word, after stemming.
    pub word: String,
    /// The stored word it met.
    pub found: String,
    /// `exact`, `synonym`, `prefix` or `typo`.
    pub how: &'static str,
    /// `title`, `tags` or `body`.
    #[serde(rename = "in")]
    pub field: &'static str,
}

/// One multiplication applied to the text score.
#[derive(Debug, Clone, Serialize)]
pub struct Factor {
    pub name: &'static str,
    pub times: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Why {
    pub matches: Vec<WordMatch>,
    /// The text score (BM25; title counts 10, tags 5, body 1).
    pub text_score: f64,
    /// Each adjustment, in the order it was applied. The final score is
    /// `text_score` times all of them.
    pub factors: Vec<Factor>,
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

/// One word of the query as the search sees it: the terms that stand for
/// it, and for a loose search a prefix as well.
struct Matcher {
    /// The query word itself, stemmed.
    stem: String,
    /// The stem and its synonyms.
    terms: Vec<String>,
    /// Stored words a typo away from the stem (loose search only).
    fuzzy: Vec<String>,
    prefix: Option<String>,
}

impl Matcher {
    /// How this query word meets a stored word, if it does.
    fn how(&self, token: &str) -> Option<&'static str> {
        if token == self.stem {
            Some("exact")
        } else if self.terms.iter().any(|term| term == token) {
            Some("synonym")
        } else if self.fuzzy.iter().any(|term| term == token) {
            Some("typo")
        } else if self.prefix.as_deref().is_some_and(|p| token.starts_with(p)) {
            Some("prefix")
        } else {
            None
        }
    }

    fn matches(&self, token: &str) -> bool {
        self.how(token).is_some()
    }
}

/// Where each query word was found in a node: the first place of title,
/// tags, body.
fn word_matches(matchers: &[Matcher], fields: &[(&'static str, &[String])]) -> Vec<WordMatch> {
    let mut found = Vec::new();
    for matcher in matchers {
        for (field, tokens) in fields {
            let hit = tokens
                .iter()
                .find_map(|token| matcher.how(token).map(|how| (token, how)));
            if let Some((token, how)) = hit {
                found.push(WordMatch {
                    word: matcher.stem.clone(),
                    found: token.clone(),
                    how,
                    field,
                });
                break;
            }
        }
    }
    found
}

/// The FTS5 MATCH expression for a set of matchers. Terms hold only letters
/// and digits, so quoting them is always safe.
fn match_expression(matchers: &[Matcher]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for matcher in matchers {
        for term in matcher.terms.iter().chain(&matcher.fuzzy) {
            parts.push(format!("\"{term}\""));
        }
        if let Some(prefix) = &matcher.prefix {
            parts.push(format!("\"{prefix}\"*"));
        }
    }
    parts.sort();
    parts.dedup();
    parts.join(" OR ")
}

pub fn recall(conn: &Connection, an: &Analyzer, terms: &str, opts: &Options) -> Result<Recall> {
    let query = an.query(terms, &store::synonyms(conn)?);
    if query.groups.is_empty() {
        bail!("nothing to search for in '{terms}'");
    }
    let limit = opts.limit.max(1);

    // First the words as given (after stemming and synonyms).
    let exact: Vec<Matcher> = query
        .groups
        .iter()
        .map(|group| Matcher {
            stem: group.stem.clone(),
            terms: group.terms.clone(),
            fuzzy: Vec::new(),
            prefix: None,
        })
        .collect();
    let mut hits = ranked(conn, an, &query, &exact, opts, false)?;

    // Too few? Then also take words that start with a query word, and
    // stored words that are a typo away from one. These never come before
    // an exact match from the same project band.
    if hits.len() < limit {
        let mut loose = Vec::with_capacity(query.groups.len());
        for group in &query.groups {
            let prefix =
                (group.stem.chars().count() >= MIN_PREFIX_CHARS).then(|| group.stem.clone());
            loose.push(Matcher {
                stem: group.stem.clone(),
                terms: group.terms.clone(),
                fuzzy: fuzzy_terms(conn, &group.stem)?,
                prefix,
            });
        }
        if match_expression(&loose) != match_expression(&exact) {
            let found: HashSet<String> = hits.iter().map(|h| h.node.id.clone()).collect();
            hits.extend(
                ranked(conn, an, &query, &loose, opts, true)?
                    .into_iter()
                    .filter(|hit| !found.contains(&hit.node.id)),
            );
        }
    }

    hits.sort_by(|a, b| {
        b.current
            .cmp(&a.current)
            .then(a.close.cmp(&b.close))
            .then(b.score.total_cmp(&a.score))
            .then_with(|| b.node.id.cmp(&a.node.id))
    });
    hits.truncate(limit);

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

/// Run one search and score its rows. Not yet sorted or cut to the limit.
fn ranked(
    conn: &Connection,
    an: &Analyzer,
    query: &Query,
    matchers: &[Matcher],
    opts: &Options,
    close: bool,
) -> Result<Vec<Hit>> {
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
         ORDER BY {} LIMIT ?4",
        NCOLS + 1
    );
    // Take more candidates than asked for: the adjustments below reorder them.
    let candidates = (opts.limit.max(1) * 10).max(50) as i64;
    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<(Node, f64, Option<f64>, bool)> = stmt
        .query_map(
            params![
                match_expression(matchers),
                opts.kind,
                opts.only_project,
                candidates
            ],
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
        let mut why = Why {
            text_score: score,
            ..Why::default()
        };
        // Every adjustment goes through here, so the explanation cannot
        // differ from what was done.
        let mut apply = |name: &'static str, times: f64| {
            if times != 1.0 {
                score *= times;
                why.factors.push(Factor { name, times });
            }
        };
        let (title, tags, body) = (
            an.tokens(&node.title),
            an.tokens(&node.tags.join(" ")),
            an.tokens(&node.body),
        );
        let age = age_days.unwrap_or(0.0).max(0.0);
        apply(
            "recency",
            1.0 + RECENCY_BOOST * 0.5f64.powf(age / RECENCY_HALF_LIFE_DAYS),
        );
        apply(
            "query words in title and tags",
            1.0 + TITLE_COVERAGE_BOOST * coverage(matchers, &title)
                + TAG_COVERAGE_BOOST * coverage(matchers, &tags),
        );
        apply(
            "type",
            match node.kind.as_str() {
                "pattern" => PATTERN_PRIOR,
                "decision" => DECISION_PRIOR,
                _ => 1.0,
            },
        );
        if opts.kind.is_none() && query.type_hint == Some(node.kind.as_str()) {
            apply("type named in the query", TYPE_HINT_BOOST);
        }
        if superseded {
            apply("superseded", SUPERSEDED_PENALTY);
        }
        if node.kind == "todo" && node.status.as_deref() != Some("open") {
            apply("closed todo", CLOSED_TODO_PENALTY);
        }
        why.matches = word_matches(
            matchers,
            &[("title", &title), ("tags", &tags), ("body", &body)],
        );
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
            close,
            superseded_by,
            why,
        });
    }
    Ok(hits)
}

/// An existing node that may hold the same knowledge as a new one.
pub struct Candidate {
    pub node: Node,
    /// From 0 to 1: how alike the two are.
    pub score: f64,
    /// Why it is listed, in plain words.
    pub reasons: Vec<String>,
}

/// The lowest likeness that is worth showing.
const DUPLICATE_THRESHOLD: f64 = 0.5;

fn word_set(an: &Analyzer, text: &str) -> Vec<String> {
    let mut words = an.tokens(text);
    words.sort();
    words.dedup();
    words
}

fn shared<'a>(a: &'a [String], b: &[String]) -> Vec<&'a str> {
    a.iter()
        .filter(|word| b.contains(word))
        .map(String::as_str)
        .collect()
}

/// How alike a new node (`title`, `body`, `kind`) and a stored one are, with
/// the reasons. Only words are compared: the same words in the title, then
/// in the body. This finds candidates; it does not decide that two nodes
/// say the same thing.
pub fn likeness(
    an: &Analyzer,
    title: &str,
    body: &str,
    kind: &str,
    node: &Node,
) -> (f64, Vec<String>) {
    let mut reasons = Vec::new();
    let (new_title, old_title) = (word_set(an, title), word_set(an, &node.title));
    let common = shared(&new_title, &old_title);
    let union = new_title.len() + old_title.len() - common.len();
    let smaller = new_title.len().min(old_title.len());
    let mut score = if title.trim().eq_ignore_ascii_case(node.title.trim()) {
        reasons.push("same title".to_string());
        1.0
    } else if common.is_empty() || union == 0 {
        0.0
    } else {
        let jaccard = common.len() as f64 / union as f64;
        // One title that holds all the words of the other, when there are
        // at least two such words.
        let contained = if smaller >= 2 && common.len() == smaller {
            0.85
        } else {
            0.0
        };
        reasons.push(format!(
            "title shares {} of {} words: {}",
            common.len(),
            union,
            common.join(", ")
        ));
        jaccard.max(contained)
    };
    let (new_body, old_body) = (word_set(an, body), word_set(an, &node.body));
    if !new_body.is_empty() && !old_body.is_empty() {
        let common = shared(&new_body, &old_body).len();
        let overlap = common as f64 / new_body.len().min(old_body.len()) as f64;
        if body.trim() == node.body.trim() {
            reasons.push("same body".to_string());
            score = score.max(0.9);
        } else if overlap >= 0.6 && score >= 0.3 {
            reasons.push(format!("body shares {common} words"));
            score = score.max(0.3 + 0.5 * overlap);
        }
    }
    if node.kind != kind {
        reasons.push(format!("different type ({})", node.kind));
        score *= 0.8;
    }
    (score.min(1.0), reasons)
}

/// Stored nodes that a new node with this title and body may duplicate,
/// most alike first. `exclude` leaves one node out (the node itself, when
/// a stored node is compared with the rest). Reads only.
pub fn duplicates(
    conn: &Connection,
    an: &Analyzer,
    title: &str,
    body: &str,
    kind: &str,
    exclude: Option<&str>,
    limit: usize,
) -> Result<Vec<Candidate>> {
    let query = an.query(title, &store::synonyms(conn)?);
    let Some(expression) = query.fts() else {
        return Ok(Vec::new());
    };
    let sql = format!(
        "SELECT {COLS} FROM nodes_fts JOIN nodes n ON n.id = nodes_fts.node_id \
         LEFT JOIN nodes p ON p.id = n.project_id \
         WHERE nodes_fts MATCH ?1 AND n.deleted_at IS NULL AND n.type != 'collection' \
           AND (?2 IS NULL OR n.id != ?2) \
         ORDER BY bm25(nodes_fts, {WEIGHTS}) LIMIT 50"
    );
    let nodes: Vec<Node> = conn
        .prepare(&sql)?
        .query_map(params![expression, exclude], store::node_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    let mut found: Vec<Candidate> = nodes
        .into_iter()
        .filter_map(|node| {
            let (score, reasons) = likeness(an, title, body, kind, &node);
            (score >= DUPLICATE_THRESHOLD).then_some(Candidate {
                node,
                score,
                reasons,
            })
        })
        .collect();
    found.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| b.node.id.cmp(&a.node.id))
    });
    found.truncate(limit);
    Ok(found)
}

/// Stored words that are a typo away from `stem`. Only for a word the index
/// does not hold at all: a word that is there is taken to be meant.
fn fuzzy_terms(conn: &Connection, stem: &str) -> Result<Vec<String>> {
    let max = match stem.chars().count() {
        0..=3 => return Ok(Vec::new()),
        4..=7 => 1,
        _ => 2,
    };
    let known: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM nodes_vocab WHERE term = ?1)",
        [stem],
        |r| r.get(0),
    )?;
    if known {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare("SELECT term, doc FROM nodes_vocab")?;
    let mut near: Vec<(usize, i64, String)> = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let term: String = row.get(0)?;
        if let Some(distance) = edit_distance(stem, &term, max) {
            near.push((distance, row.get(1)?, term));
        }
    }
    // Nearest first; among equals, the word more nodes use.
    near.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    near.truncate(MAX_FUZZY_TERMS);
    Ok(near.into_iter().map(|(_, _, term)| term).collect())
}

/// The share of the query's words that appear in `tokens`, from 0 to 1.
fn coverage(matchers: &[Matcher], tokens: &[String]) -> f64 {
    if matchers.is_empty() {
        return 0.0;
    }
    let covered = matchers
        .iter()
        .filter(|matcher| tokens.iter().any(|token| matcher.matches(token)))
        .count();
    covered as f64 / matchers.len() as f64
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
        let project = if hit.node.kind == "collection" {
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
