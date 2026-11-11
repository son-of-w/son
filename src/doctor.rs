//! `son doctor`: look at the database and the files `son init` wrote, and
//! say what is healthy, what is missing, what is out of date and what is
//! broken. It only looks: it opens the database read-only, does not
//! upgrade it, and writes nothing. Each finding names the command that
//! puts it right.

use crate::agents;
use crate::analyze::Analyzer;
use crate::db;
use crate::instructions;
use crate::project;
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Healthy,
    /// Not there yet. Not a fault.
    Missing,
    /// Works, but is behind: an older schema, an index or file out of date.
    Stale,
    /// Broken: son cannot be relied on until this is put right.
    Problem,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Healthy => "healthy",
            Status::Missing => "missing",
            Status::Stale => "stale",
            Status::Problem => "problem",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
    /// What to run to put it right.
    pub fix: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    /// The worst status of all checks.
    pub status: Status,
    pub database: String,
    pub checks: Vec<Check>,
}

fn check(
    name: &'static str,
    status: Status,
    detail: impl Into<String>,
    fix: Option<&str>,
) -> Check {
    Check {
        name,
        status,
        detail: detail.into(),
        fix: fix.map(str::to_string),
    }
}

fn database_checks(path: &Path, checks: &mut Vec<Check>) {
    if !path.exists() {
        checks.push(check(
            "database",
            Status::Missing,
            format!("{} does not exist yet", path.display()),
            Some("son init"),
        ));
        return;
    }
    let conn = match Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(conn) => conn,
        Err(err) => {
            checks.push(check(
                "database",
                Status::Problem,
                format!("cannot open {}: {err}", path.display()),
                Some("restore a backup: son import <backup file> --replace"),
            ));
            return;
        }
    };
    let _ = conn.busy_timeout(db::busy_wait());
    let version: i64 = match conn.query_row("PRAGMA user_version", [], |r| r.get(0)) {
        Ok(version) => version,
        Err(err) => {
            checks.push(check(
                "database",
                Status::Problem,
                format!("{} is not a readable SQLite file: {err}", path.display()),
                Some("restore a backup: son import <backup file> --replace"),
            ));
            return;
        }
    };
    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    checks.push(check(
        "database",
        Status::Healthy,
        format!("{} ({bytes} bytes)", path.display()),
        None,
    ));

    let current = db::SCHEMA_VERSION;
    checks.push(match version {
        v if v == current => check("schema", Status::Healthy, format!("version {v}"), None),
        0 => check(
            "schema",
            Status::Missing,
            "the file holds no son tables yet",
            Some("son init --db-only"),
        ),
        v if v < current => check(
            "schema",
            Status::Stale,
            format!("version {v}; this son uses version {current}. The next command that writes upgrades it"),
            Some("son init --db-only"),
        ),
        v => check(
            "schema",
            Status::Problem,
            format!("version {v} is newer than this son knows (version {current})"),
            Some("install a newer son"),
        ),
    });

    let integrity: Result<Vec<String>, _> = conn
        .prepare("PRAGMA integrity_check")
        .and_then(|mut stmt| stmt.query_map([], |r| r.get(0))?.collect());
    checks.push(match integrity {
        Ok(lines) if lines == ["ok"] => check(
            "integrity",
            Status::Healthy,
            "SQLite finds the file whole",
            None,
        ),
        Ok(lines) => check(
            "integrity",
            Status::Problem,
            format!("SQLite reports damage: {}", lines.join("; ")),
            Some("restore a backup: son import <backup file> --replace"),
        ),
        Err(err) => check(
            "integrity",
            Status::Problem,
            format!("the integrity check failed: {err}"),
            Some("restore a backup: son import <backup file> --replace"),
        ),
    });
    if version != current {
        return;
    }

    let broken: Result<i64, _> = conn
        .prepare("PRAGMA foreign_key_check")
        .and_then(|mut stmt| Ok(stmt.query_map([], |_| Ok(()))?.count() as i64));
    checks.push(match broken {
        Ok(0) => check(
            "references",
            Status::Healthy,
            "every row points at a row that exists",
            None,
        ),
        Ok(n) => check(
            "references",
            Status::Problem,
            format!("{n} row(s) point at a row that does not exist"),
            Some("restore a backup: son import <backup file> --replace"),
        ),
        Err(err) => check(
            "references",
            Status::Problem,
            format!("the check failed: {err}"),
            None,
        ),
    });
    checks.push(index_check(&conn));
}

/// Compare the search index with the nodes: each live node has one row,
/// holding the text the analyzer makes from it now.
fn index_check(conn: &Connection) -> Check {
    let an = Analyzer::new();
    let run = || -> rusqlite::Result<(i64, i64, i64, i64)> {
        let mut missing = 0;
        let mut outdated = 0;
        let mut nodes = 0;
        let mut stmt = conn.prepare(
            "SELECT n.title, n.body, \
                    (SELECT group_concat(tag, ' ') FROM tags WHERE node_id = n.id), \
                    f.title, f.tags, f.body \
             FROM nodes n LEFT JOIN nodes_fts f ON f.node_id = n.id WHERE n.deleted_at IS NULL",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            nodes += 1;
            let indexed: Option<String> = row.get(3)?;
            let Some(indexed_title) = indexed else {
                missing += 1;
                continue;
            };
            let (title, body, tags): (String, String, Option<String>) =
                (row.get(0)?, row.get(1)?, row.get(2)?);
            let (indexed_tags, indexed_body): (String, String) = (row.get(4)?, row.get(5)?);
            if an.index_text(&title) != indexed_title
                || an.index_text(&tags.unwrap_or_default()) != indexed_tags
                || an.index_text(&body) != indexed_body
            {
                outdated += 1;
            }
        }
        let extra: i64 = conn.query_row(
            "SELECT count(*) FROM nodes_fts f WHERE NOT EXISTS \
             (SELECT 1 FROM nodes n WHERE n.id = f.node_id AND n.deleted_at IS NULL)",
            [],
            |r| r.get(0),
        )?;
        Ok((nodes, missing, outdated, extra))
    };
    match run() {
        Ok((nodes, 0, 0, 0)) => check(
            "search index",
            Status::Healthy,
            format!("{nodes} live nodes, all indexed"),
            None,
        ),
        Ok((_, missing, outdated, extra)) => check(
            "search index",
            Status::Stale,
            format!(
                "{missing} node(s) not indexed, {outdated} indexed with old text, {extra} index row(s) for nodes that are gone; recall can miss or mislist these"
            ),
            Some("son reindex"),
        ),
        Err(err) => check(
            "search index",
            Status::Problem,
            format!("the search index cannot be read: {err}"),
            Some("son reindex"),
        ),
    }
}

/// The files `son init` wrote: are they what this son would write now?
fn file_checks(checks: &mut Vec<Check>) {
    let root = std::env::current_dir()
        .map(|cwd| project::root(&cwd))
        .unwrap_or_default();
    let mut found = 0;
    for (path, is_skill) in agents::candidate_files(&root) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if !instructions::is_installed_in(&text) {
            continue;
        }
        found += 1;
        let shown = crate::setup::short_path(&path);
        let name = if is_skill {
            "skill file"
        } else {
            "instruction file"
        };
        let current = if is_skill {
            text == instructions::skill()
        } else {
            text.contains(&instructions::block())
        };
        let whole = is_skill || text.contains(instructions::END);
        checks.push(if current {
            check(name, Status::Healthy, shown, None)
        } else if !whole {
            check(
                name,
                Status::Problem,
                format!("{shown}: the son block has a start mark and no end mark"),
                Some("remove the broken block by hand, then run `son init`"),
            )
        } else {
            check(
                name,
                Status::Stale,
                format!("{shown} was written by another version of son"),
                Some("son init"),
            )
        });
    }
    if found == 0 {
        checks.push(check(
            "agent setup",
            Status::Missing,
            "son is not set up for any agent here, so no agent has a reason to call it",
            Some("son init"),
        ));
    }
}

/// Look at everything. Reads only.
pub fn run(database: &Path) -> Report {
    let mut checks = Vec::new();
    database_checks(database, &mut checks);
    file_checks(&mut checks);
    Report {
        status: checks
            .iter()
            .map(|c| c.status)
            .max()
            .unwrap_or(Status::Healthy),
        database: database.display().to_string(),
        checks,
    }
}
