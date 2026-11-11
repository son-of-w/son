//! The activity log: one row for every command that is run, and the
//! numbers `son stats` draws from it.
//!
//! The log is local bookkeeping. It is not part of an export and will not
//! be synced. Set `SON_NO_LOG=1` to stop writing it.

use crate::store::{self, Node, COLS, FROM, NCOLS};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

const DETAIL_CHARS: usize = 200;
const LOCAL_TIME: &str = "'%Y-%m-%d %H:%M:%S'";

/// What a command learns about itself while it runs.
#[derive(Default)]
pub struct Draft {
    pub project: Option<String>,
    pub node_id: Option<String>,
    /// The query for a recall, the title for a write.
    pub detail: Option<String>,
    /// How many results a read gave.
    pub results: Option<i64>,
    /// How many of a recall's results were close matches only.
    pub close: Option<i64>,
}

pub struct Event<'a> {
    pub command: &'a str,
    pub write: bool,
    pub source: Option<&'a str>,
    pub ms: i64,
    /// The error text when the command failed.
    pub error: Option<String>,
    pub draft: &'a Draft,
}

pub fn enabled() -> bool {
    std::env::var_os("SON_NO_LOG").is_none_or(|v| v.is_empty() || v == "0")
}

fn clip(text: &str) -> String {
    text.trim().chars().take(DETAIL_CHARS).collect()
}

pub fn record(conn: &Connection, event: &Event) -> Result<()> {
    conn.execute(
        &format!(
            "INSERT INTO events (at, command, kind, ok, project, source, node_id, detail, results, close, ms, error) \
             VALUES ({}, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            store::NOW
        ),
        params![
            event.command,
            if event.write { "write" } else { "read" },
            event.error.is_none(),
            event.draft.project,
            event.source,
            event.draft.node_id,
            event.draft.detail.as_deref().map(clip),
            event.draft.results,
            event.draft.close,
            event.ms,
            event.error.as_deref().map(clip),
        ],
    )?;
    Ok(())
}

/// One page of something, with what is needed to print "page 2 of 7".
#[derive(Serialize)]
pub struct Page<T> {
    pub page: usize,
    pub pages: usize,
    pub total: usize,
    pub limit: usize,
    pub items: Vec<T>,
}

impl<T> Page<T> {
    fn new(items: Vec<T>, total: i64, limit: usize, page: usize) -> Self {
        let total = total.max(0) as usize;
        Self {
            page,
            pages: total.div_ceil(limit).max(1),
            total,
            limit,
            items,
        }
    }
}

fn offset(limit: usize, page: usize) -> i64 {
    (limit * (page - 1)) as i64
}

/// A node in `son list`: what last happened to it, and when (local time).
pub struct Change {
    pub node: Node,
    pub at: String,
    pub action: &'static str,
}

pub struct NodeFilter<'a> {
    pub project_id: Option<&'a str>,
    pub kind: Option<&'a str>,
    pub include_dropped: bool,
}

/// Nodes, most recently changed first.
pub fn nodes_page(
    conn: &Connection,
    filter: &NodeFilter,
    limit: usize,
    page: usize,
) -> Result<Page<Change>> {
    // Project nodes are left out unless asked for: `son projects` lists them.
    let conditions = "(?1 IS NULL OR n.project_id = ?1) \
         AND ((?2 IS NULL AND n.type != 'project') OR n.type = ?2) \
         AND (?3 OR n.deleted_at IS NULL)";
    let total: i64 = conn.query_row(
        &format!("SELECT count(*) FROM nodes n WHERE {conditions}"),
        params![filter.project_id, filter.kind, filter.include_dropped],
        |r| r.get(0),
    )?;
    let sql = format!(
        "SELECT {COLS}, strftime({LOCAL_TIME}, n.updated_at, 'localtime') {FROM} \
         WHERE {conditions} ORDER BY n.updated_at DESC, n.id DESC LIMIT ?4 OFFSET ?5"
    );
    let mut stmt = conn.prepare(&sql)?;
    let items = stmt
        .query_map(
            params![
                filter.project_id,
                filter.kind,
                filter.include_dropped,
                limit as i64,
                offset(limit, page)
            ],
            |r| {
                let node = store::node_from_row(r)?;
                let action = if node.deleted_at.is_some() {
                    "dropped"
                } else if node.rev <= 1 {
                    "created"
                } else {
                    "updated"
                };
                Ok(Change {
                    node,
                    at: r.get(NCOLS)?,
                    action,
                })
            },
        )?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Page::new(items, total, limit, page))
}

/// One row of the activity log, with the time in local time.
#[derive(Serialize)]
pub struct EventRow {
    pub at: String,
    pub command: String,
    pub kind: String,
    pub ok: bool,
    pub project: Option<String>,
    pub source: Option<String>,
    pub node_id: Option<String>,
    pub detail: Option<String>,
    pub results: Option<i64>,
    pub close: Option<i64>,
    pub ms: i64,
    pub error: Option<String>,
}

pub struct EventFilter<'a> {
    pub project: Option<&'a str>,
    pub command: Option<&'a str>,
    pub errors_only: bool,
}

/// Commands that were run, newest first.
pub fn events_page(
    conn: &Connection,
    filter: &EventFilter,
    limit: usize,
    page: usize,
) -> Result<Page<EventRow>> {
    let conditions = "(?1 IS NULL OR project = ?1 COLLATE NOCASE) \
         AND (?2 IS NULL OR command = ?2) \
         AND (NOT ?3 OR ok = 0)";
    let total: i64 = conn.query_row(
        &format!("SELECT count(*) FROM events WHERE {conditions}"),
        params![filter.project, filter.command, filter.errors_only],
        |r| r.get(0),
    )?;
    let sql = format!(
        "SELECT strftime({LOCAL_TIME}, at, 'localtime'), command, kind, ok, project, source, \
                node_id, detail, results, close, ms, error \
         FROM events WHERE {conditions} ORDER BY id DESC LIMIT ?4 OFFSET ?5"
    );
    let mut stmt = conn.prepare(&sql)?;
    let items = stmt
        .query_map(
            params![
                filter.project,
                filter.command,
                filter.errors_only,
                limit as i64,
                offset(limit, page)
            ],
            |r| {
                Ok(EventRow {
                    at: r.get(0)?,
                    command: r.get(1)?,
                    kind: r.get(2)?,
                    ok: r.get(3)?,
                    project: r.get(4)?,
                    source: r.get(5)?,
                    node_id: r.get(6)?,
                    detail: r.get(7)?,
                    results: r.get(8)?,
                    close: r.get(9)?,
                    ms: r.get(10)?,
                    error: r.get(11)?,
                })
            },
        )?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Page::new(items, total, limit, page))
}

#[derive(Serialize)]
pub struct Count {
    pub name: String,
    pub count: i64,
}

#[derive(Serialize)]
pub struct StoreStats {
    pub nodes: i64,
    pub by_type: Vec<Count>,
    pub projects: i64,
    pub dropped: i64,
    pub links: i64,
    pub tags: i64,
    pub synonyms: i64,
    pub todos_open: i64,
    pub todos_done: i64,
    pub todos_dropped: i64,
    pub open_todos_older_than_30_days: i64,
    pub first_node_at: Option<String>,
    pub last_change_at: Option<String>,
}

#[derive(Serialize)]
pub struct ActivityStats {
    pub commands: i64,
    pub reads: i64,
    pub writes: i64,
    pub errors: i64,
    /// Sessions that named themselves through `SON_SOURCE`.
    pub sessions: i64,
    pub active_days: i64,
    /// Successful writes for each recall: how often a session gives back.
    pub writes_per_recall: Option<f64>,
}

#[derive(Serialize)]
pub struct RecallStats {
    pub recalls: i64,
    pub found_something: i64,
    pub found_nothing: i64,
    /// Share of recalls that found something, from 0 to 1.
    pub hit_rate: Option<f64>,
    /// Recalls where a prefix or typo match was needed to fill the list.
    pub needed_close_match: i64,
    pub average_results: Option<f64>,
    pub median_ms: Option<i64>,
    pub p95_ms: Option<i64>,
    /// The queries that most often found nothing.
    pub missed: Vec<Count>,
}

#[derive(Serialize)]
pub struct Day {
    pub day: String,
    pub commands: i64,
    pub recalls: i64,
    pub writes: i64,
}

#[derive(Serialize)]
pub struct Stats {
    /// Days covered by `activity`, `recall` and the breakdowns; `None` is all time.
    pub days: Option<u32>,
    pub store: StoreStats,
    pub activity: ActivityStats,
    pub recall: RecallStats,
    pub by_command: Vec<Count>,
    pub by_project: Vec<Count>,
    pub by_day: Vec<Day>,
    pub first_event_at: Option<String>,
}

fn counts(conn: &Connection, sql: &str, since: &str) -> Result<Vec<Count>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map([since], |r| {
            Ok(Count {
                name: r.get(0)?,
                count: r.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

fn ratio(part: i64, whole: i64) -> Option<f64> {
    (whole > 0).then(|| part as f64 / whole as f64)
}

/// The value at `share` (0 to 1) of a sorted list.
fn percentile(sorted: &[i64], share: f64) -> Option<i64> {
    if sorted.is_empty() {
        return None;
    }
    let index = ((sorted.len() - 1) as f64 * share).round() as usize;
    Some(sorted[index])
}

pub fn stats(conn: &Connection, days: Option<u32>) -> Result<Stats> {
    let one = |sql: &str| -> Result<i64> { Ok(conn.query_row(sql, [], |r| r.get(0))?) };
    let text = |sql: &str| -> Result<Option<String>> {
        Ok(conn
            .query_row(sql, [], |r| r.get::<_, Option<String>>(0))
            .optional()?
            .flatten())
    };

    let mut by_type = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT type, count(*) FROM nodes WHERE deleted_at IS NULL AND type != 'project' \
             GROUP BY type ORDER BY count(*) DESC, type",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Count {
                name: r.get(0)?,
                count: r.get(1)?,
            })
        })?;
        for row in rows {
            by_type.push(row?);
        }
    }
    let todo = |status: &str| -> Result<i64> {
        Ok(conn.query_row(
            "SELECT count(*) FROM nodes WHERE type = 'todo' AND deleted_at IS NULL AND status = ?1",
            [status],
            |r| r.get(0),
        )?)
    };
    let store = StoreStats {
        nodes: one("SELECT count(*) FROM nodes WHERE deleted_at IS NULL AND type != 'project'")?,
        by_type,
        projects: one("SELECT count(*) FROM nodes WHERE deleted_at IS NULL AND type = 'project'")?,
        dropped: one("SELECT count(*) FROM nodes WHERE deleted_at IS NOT NULL")?,
        links: one("SELECT count(*) FROM edges")?,
        tags: one("SELECT count(DISTINCT tag) FROM tags")?,
        synonyms: one("SELECT count(*) FROM synonyms")?,
        todos_open: todo("open")?,
        todos_done: todo("done")?,
        todos_dropped: todo("dropped")?,
        open_todos_older_than_30_days: one(
            "SELECT count(*) FROM nodes WHERE type = 'todo' AND deleted_at IS NULL \
             AND status = 'open' AND julianday('now') - julianday(created_at) > 30",
        )?,
        first_node_at: text(
            "SELECT strftime('%Y-%m-%d', min(created_at), 'localtime') FROM nodes WHERE type != 'project'",
        )?,
        last_change_at: text(
            "SELECT strftime('%Y-%m-%d %H:%M', max(updated_at), 'localtime') FROM nodes",
        )?,
    };

    // The start of the window, in the same form as the `at` column.
    let since: String = match days {
        Some(days) => conn.query_row(
            "SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?1)",
            [format!("-{days} days")],
            |r| r.get(0),
        )?,
        None => String::new(),
    };
    let window = |sql: &str| -> Result<i64> {
        Ok(conn
            .query_row(sql, [&since], |r| r.get::<_, Option<i64>>(0))?
            .unwrap_or(0))
    };

    let recalls =
        window("SELECT count(*) FROM events WHERE at >= ?1 AND command = 'recall' AND ok")?;
    let writes = window("SELECT count(*) FROM events WHERE at >= ?1 AND kind = 'write' AND ok")?;
    let activity = ActivityStats {
        commands: window("SELECT count(*) FROM events WHERE at >= ?1")?,
        reads: window("SELECT count(*) FROM events WHERE at >= ?1 AND kind = 'read' AND ok")?,
        writes,
        errors: window("SELECT count(*) FROM events WHERE at >= ?1 AND NOT ok")?,
        sessions: window("SELECT count(DISTINCT source) FROM events WHERE at >= ?1")?,
        active_days: window(
            "SELECT count(DISTINCT date(at, 'localtime')) FROM events WHERE at >= ?1",
        )?,
        writes_per_recall: ratio(writes, recalls),
    };

    let found_something = window(
        "SELECT count(*) FROM events WHERE at >= ?1 AND command = 'recall' AND ok AND results > 0",
    )?;
    let mut times: Vec<i64> = conn
        .prepare("SELECT ms FROM events WHERE at >= ?1 AND command = 'recall' AND ok ORDER BY ms")?
        .query_map([&since], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    times.sort_unstable();
    let average_results: Option<f64> = conn.query_row(
        "SELECT avg(results) FROM events WHERE at >= ?1 AND command = 'recall' AND ok",
        [&since],
        |r| r.get(0),
    )?;
    let recall = RecallStats {
        recalls,
        found_something,
        found_nothing: recalls - found_something,
        hit_rate: ratio(found_something, recalls),
        needed_close_match: window(
            "SELECT count(*) FROM events WHERE at >= ?1 AND command = 'recall' AND ok AND close > 0",
        )?,
        average_results,
        median_ms: percentile(&times, 0.5),
        p95_ms: percentile(&times, 0.95),
        missed: counts(
            conn,
            "SELECT lower(detail), count(*) FROM events \
             WHERE at >= ?1 AND command = 'recall' AND ok AND results = 0 AND detail IS NOT NULL \
             GROUP BY lower(detail) ORDER BY count(*) DESC, max(id) DESC LIMIT 5",
            &since,
        )?,
    };

    let by_day = conn
        .prepare(
            "SELECT date(at, 'localtime'), count(*), \
                    sum(command = 'recall' AND ok), sum(kind = 'write' AND ok) \
             FROM events WHERE at >= ?1 GROUP BY 1 ORDER BY 1 DESC LIMIT 14",
        )?
        .query_map([&since], |r| {
            Ok(Day {
                day: r.get(0)?,
                commands: r.get(1)?,
                recalls: r.get(2)?,
                writes: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;

    Ok(Stats {
        days,
        store,
        activity,
        recall,
        by_command: counts(
            conn,
            "SELECT command, count(*) FROM events WHERE at >= ?1 \
             GROUP BY command ORDER BY count(*) DESC, command",
            &since,
        )?,
        by_project: counts(
            conn,
            "SELECT project, count(*) FROM events WHERE at >= ?1 AND project IS NOT NULL \
             GROUP BY project COLLATE NOCASE ORDER BY count(*) DESC, project LIMIT 10",
            &since,
        )?,
        by_day,
        first_event_at: text("SELECT strftime('%Y-%m-%d', min(at), 'localtime') FROM events")?,
    })
}
