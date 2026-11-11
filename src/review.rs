//! `son review`: the nodes that deserve a second look, and why.
//!
//! Review reads; it never changes a node. A node is listed when:
//!
//! - someone marked it stale (`son stale`);
//! - it is a fact or a decision that was not verified for longer than the
//!   limit (counted from the last `son verify`, or from creation when it
//!   was never verified; an edit does not restart the count);
//! - it is a todo that has been open for longer than the limit;
//! - another node may say the same thing (the same test as
//!   `son remember --check`).
//!
//! The limits only decide what is listed. Nothing becomes stale, expires
//! or is deleted because of its age.

use crate::analyze::Analyzer;
use crate::search;
use crate::store::{self, Node, COLS, FROM, NCOLS};
use anyhow::Result;
use rusqlite::{params, Connection};
use serde::Serialize;

/// Days after which a node is listed.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub fact_days: u32,
    pub decision_days: u32,
    pub todo_days: u32,
}

impl Limits {
    pub const DEFAULT: Limits = Limits {
        fact_days: 90,
        decision_days: 180,
        todo_days: 30,
    };

    /// The defaults, changed by `SON_REVIEW_FACT_DAYS`,
    /// `SON_REVIEW_DECISION_DAYS` and `SON_REVIEW_TODO_DAYS` when set.
    pub fn from_env() -> Limits {
        let days = |name: &str, default: u32| {
            std::env::var(name)
                .ok()
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(default)
        };
        Limits {
            fact_days: days("SON_REVIEW_FACT_DAYS", Self::DEFAULT.fact_days),
            decision_days: days("SON_REVIEW_DECISION_DAYS", Self::DEFAULT.decision_days),
            todo_days: days("SON_REVIEW_TODO_DAYS", Self::DEFAULT.todo_days),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Reason {
    /// `stale`, `not_verified`, `old_todo` or `possible_duplicate`.
    pub code: &'static str,
    pub text: String,
    /// Whole days, for the reasons that count days.
    pub days: Option<i64>,
    /// For a possible duplicate: the short id of the other node.
    pub other: Option<String>,
}

pub struct Item {
    pub node: Node,
    pub reasons: Vec<Reason>,
}

pub struct Filter<'a> {
    pub collection_id: Option<&'a str>,
    pub kind: Option<&'a str>,
}

/// True when the two nodes have a link between them, in either direction.
/// A link says someone already looked at the pair: they are related, or
/// one replaces the other, so they are not listed as duplicates.
fn linked(conn: &Connection, a: &str, b: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM edges WHERE (from_id = ?1 AND to_id = ?2) \
                                               OR (from_id = ?2 AND to_id = ?1))",
        params![a, b],
        |r| r.get(0),
    )?)
}

/// Every node that needs attention, most pressing first: stale, then
/// possible duplicates, then the longest overdue.
pub fn items(
    conn: &Connection,
    an: &Analyzer,
    limits: &Limits,
    filter: &Filter,
) -> Result<Vec<Item>> {
    let sql = format!(
        "SELECT {COLS}, \
                julianday('now') - julianday(coalesce(n.verified_at, n.created_at)), \
                julianday('now') - julianday(n.created_at), \
                EXISTS (SELECT 1 FROM edges e JOIN nodes s ON s.id = e.from_id \
                        WHERE e.to_id = n.id AND e.relation = 'supersedes' AND s.deleted_at IS NULL) \
         {FROM} WHERE n.deleted_at IS NULL AND n.type != 'collection' \
           AND (?1 IS NULL OR n.project_id = ?1) AND (?2 IS NULL OR n.type = ?2) \
         ORDER BY n.id"
    );
    let rows: Vec<(Node, f64, f64, bool)> = conn
        .prepare(&sql)?
        .query_map(params![filter.collection_id, filter.kind], |r| {
            Ok((
                store::node_from_row(r)?,
                r.get::<_, Option<f64>>(NCOLS)?.unwrap_or(0.0),
                r.get::<_, Option<f64>>(NCOLS + 1)?.unwrap_or(0.0),
                r.get(NCOLS + 2)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;

    let mut found: Vec<(u8, f64, Item)> = Vec::new();
    for (node, unverified_days, age_days, superseded) in rows {
        let mut reasons = Vec::new();
        let mut rank = 3;
        let mut overdue = 0.0;
        if node.stale_at.is_some() {
            rank = 0;
            let note = node.stale_note.as_deref().unwrap_or("").trim();
            reasons.push(Reason {
                code: "stale",
                text: if note.is_empty() {
                    "marked stale".to_string()
                } else {
                    format!("marked stale: {note}")
                },
                days: None,
                other: None,
            });
        }
        let limit = match node.kind.as_str() {
            "fact" => Some(limits.fact_days),
            "decision" => Some(limits.decision_days),
            _ => None,
        };
        // A stale node is listed as stale; a superseded decision is history.
        if let Some(limit) = limit.filter(|_| node.stale_at.is_none() && !superseded) {
            if unverified_days > f64::from(limit) {
                let days = unverified_days.floor() as i64;
                overdue = unverified_days - f64::from(limit);
                reasons.push(Reason {
                    code: "not_verified",
                    text: match &node.verified_at {
                        Some(_) => format!(
                            "{} last verified {days} days ago (limit {limit})",
                            node.kind
                        ),
                        None => format!(
                            "{} never verified, {days} days old (limit {limit})",
                            node.kind
                        ),
                    },
                    days: Some(days),
                    other: None,
                });
            }
        }
        if node.kind == "todo"
            && node.status.as_deref() == Some("open")
            && age_days > f64::from(limits.todo_days)
        {
            let days = age_days.floor() as i64;
            overdue = age_days - f64::from(limits.todo_days);
            reasons.push(Reason {
                code: "old_todo",
                text: format!("todo open for {days} days (limit {})", limits.todo_days),
                days: Some(days),
                other: None,
            });
        }
        // Each pair once: on the newer node, pointing at the older one.
        let closed_todo = node.kind == "todo" && node.status.as_deref() != Some("open");
        if !superseded && !closed_todo {
            for candidate in search::duplicates(
                conn,
                an,
                &node.title,
                &node.body,
                &node.kind,
                Some(&node.id),
                3,
            )? {
                if candidate.node.id < node.id && !linked(conn, &node.id, &candidate.node.id)? {
                    rank = rank.min(1);
                    reasons.push(Reason {
                        code: "possible_duplicate",
                        text: format!(
                            "may duplicate `{}` {} ({})",
                            candidate.node.short(),
                            candidate.node.title,
                            candidate.reasons.join("; ")
                        ),
                        days: None,
                        other: Some(candidate.node.short()),
                    });
                }
            }
        }
        if !reasons.is_empty() {
            found.push((rank.min(2), overdue, Item { node, reasons }));
        }
    }
    found.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(b.1.total_cmp(&a.1))
            .then_with(|| a.2.node.id.cmp(&b.2.node.id))
    });
    Ok(found.into_iter().map(|(_, _, item)| item).collect())
}
