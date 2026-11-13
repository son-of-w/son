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
    /// Something for the user to look at. son works, and doctor does not
    /// fail for it: text that looks like a secret, a file others can read.
    Warning,
    /// Broken: son cannot be relied on until this is put right.
    Problem,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Healthy => "healthy",
            Status::Missing => "missing",
            Status::Stale => "stale",
            Status::Warning => "warning",
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
    access_check(path, checks);
    copies_check(path, checks);

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
    checks.push(record_check(&conn));
    checks.push(secret_check(&conn));
}

/// Is the database file, or a file beside it, open to other users of the
/// machine? Looks only; a file that is wider than its owner is reported
/// with the command that closes it, and is not changed.
#[cfg(unix)]
fn access_check(path: &Path, checks: &mut Vec<Check>) {
    use std::os::unix::fs::PermissionsExt;
    let mut wide: Vec<String> = Vec::new();
    let mut fix: Vec<String> = Vec::new();
    let mode_of = |p: &Path| {
        std::fs::metadata(p)
            .ok()
            .map(|m| m.permissions().mode() & 0o777)
    };
    for suffix in ["", "-wal", "-shm"] {
        let file = std::path::PathBuf::from(format!("{}{suffix}", path.display()));
        if let Some(mode) = mode_of(&file).filter(|mode| mode & 0o077 != 0) {
            wide.push(format!("{} (mode {mode:o})", file.display()));
            fix.push(format!("chmod 600 '{}'", file.display()));
        }
    }
    // The folder is looked at only when it is son's own.
    let own_dir = crate::agents::home().map(|home| home.join(".son-of-w"));
    if let Some(dir) = path.parent().filter(|dir| Some(*dir) == own_dir.as_deref()) {
        if let Some(mode) = mode_of(dir).filter(|mode| mode & 0o077 != 0) {
            wide.push(format!("{} (mode {mode:o})", dir.display()));
            fix.push(format!("chmod 700 '{}'", dir.display()));
        }
    }
    checks.push(if wide.is_empty() {
        check(
            "file access",
            Status::Healthy,
            "only the owner can read the database",
            None,
        )
    } else {
        check(
            "file access",
            Status::Warning,
            format!(
                "other users of this machine can read: {}. son does not change this by itself",
                wide.join(", ")
            ),
            Some(&fix.join(" && ")),
        )
    });
}

#[cfg(not(unix))]
fn access_check(_path: &Path, _checks: &mut Vec<Check>) {}

/// The copies that were made before schema upgrades. son never removes
/// them, so say that they are there and how large they are.
fn copies_check(path: &Path, checks: &mut Vec<Check>) {
    let dir = db::upgrade_copy_dir(path);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut copies: Vec<(String, u64)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_copy = name.starts_with("son-before-schema-") && name.ends_with(".db");
            is_copy.then(|| (name, entry.metadata().map(|m| m.len()).unwrap_or(0)))
        })
        .collect();
    if copies.is_empty() {
        return;
    }
    copies.sort();
    let total: u64 = copies.iter().map(|(_, bytes)| bytes).sum();
    let names: Vec<String> = copies
        .iter()
        .map(|(name, bytes)| format!("{name} ({bytes} bytes)"))
        .collect();
    checks.push(check(
        "upgrade copies",
        Status::Healthy,
        format!(
            "{} copy(ies) of the database from before a schema upgrade in {}, {total} bytes in all: {}. \
             son never removes them; delete the ones you no longer need. Restore one with `son import <file> --replace`",
            copies.len(),
            dir.display(),
            names.join(", ")
        ),
        None,
    ));
}

/// Look for stored text that looks like a secret. Only the id, the field
/// and the kind are reported, never the text.
fn secret_check(conn: &Connection) -> Check {
    let Ok(nodes) = crate::store::list_nodes(conn, None, None, true) else {
        return check("secrets", Status::Problem, "the nodes cannot be read", None);
    };
    let found: Vec<String> = nodes
        .iter()
        .filter_map(|node| {
            crate::transfer::secret_in(node)
                .map(|(field, kind)| format!("`{}` ({field}: {kind})", node.short()))
        })
        .collect();
    if found.is_empty() {
        return check(
            "secrets",
            Status::Healthy,
            "no stored node looks like it holds a secret (common forms only)",
            None,
        );
    }
    let listed: Vec<&str> = found.iter().take(20).map(String::as_str).collect();
    check(
        "secrets",
        Status::Warning,
        format!(
            "{} node(s) look like they hold a secret: {}{}",
            found.len(),
            listed.join(", "),
            if found.len() > listed.len() {
                ", and more"
            } else {
                ""
            }
        ),
        Some(
            "for each real secret: change the secret where it is used, then `son drop <id> --purge`. \
             Backups and exports made before still hold the text",
        ),
    )
}

/// Look for stored nodes that this son could not have written: an id of
/// another form, a type or a status it does not know. An import of an
/// older son could store such rows.
fn record_check(conn: &Connection) -> Check {
    let nodes = match crate::store::list_nodes(conn, None, None, true) {
        Ok(nodes) => nodes,
        Err(err) => {
            return check(
                "records",
                Status::Problem,
                format!("the nodes cannot be read: {err:#}"),
                Some("restore a backup: son import <backup file> --replace"),
            )
        }
    };
    let bad: Vec<String> = nodes
        .iter()
        .filter_map(|node| {
            let faults = crate::transfer::node_faults(node);
            (!faults.is_empty()).then(|| format!("'{}' ({})", node.id, faults.join("; ")))
        })
        .collect();
    if bad.is_empty() {
        return check(
            "records",
            Status::Healthy,
            format!("{} nodes, all of a form this son knows", nodes.len()),
            None,
        );
    }
    let listed: Vec<&str> = bad.iter().take(10).map(String::as_str).collect();
    check(
        "records",
        Status::Problem,
        format!(
            "{} node(s) are not of a form this son knows: {}{}",
            bad.len(),
            listed.join(", "),
            if bad.len() > listed.len() {
                ", and more"
            } else {
                ""
            }
        ),
        Some("for each one: son drop '<id>' --purge (give the full id as shown)"),
    )
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

/// What a file that `son init` wrote is like now.
pub enum FileState {
    /// It is what this son writes.
    Current,
    /// It was written by another version of son. `son init` brings it up
    /// to date.
    Stale,
    /// Its marks do not make one block; the text says what is wrong.
    /// `son init` refuses to change it.
    Broken(String),
}

/// Every file here that holds a son skill or a son block, global and for
/// the current project, with its state. One meaning of "out of date" for
/// `son doctor` and for the warning in `son context`.
pub fn agent_files() -> Vec<(std::path::PathBuf, bool, FileState)> {
    let root = std::env::current_dir()
        .map(|cwd| project::root(&cwd))
        .unwrap_or_default();
    agents::candidate_files(&root)
        .into_iter()
        .filter_map(|(path, is_skill)| {
            let text = std::fs::read_to_string(&path).ok()?;
            if !instructions::is_installed_in(&text) {
                return None;
            }
            let state = if text.contains(instructions::TODO_SKILL_MARK) {
                if text == instructions::todo_skill(&instructions::skill_name_of(&path)) {
                    FileState::Current
                } else {
                    FileState::Stale
                }
            } else if is_skill {
                if text == instructions::skill() {
                    FileState::Current
                } else {
                    FileState::Stale
                }
            } else {
                match instructions::find_block(&text) {
                    Err(fault) => FileState::Broken(fault),
                    Ok(_) if text.contains(&instructions::block()) => FileState::Current,
                    Ok(_) => FileState::Stale,
                }
            };
            Some((path, is_skill, state))
        })
        .collect()
}

/// What `son context` says when the agent files need the user: nothing
/// when all are current. The lines tell the agent to tell the user; an
/// agent must not change the user's setup by itself.
pub fn setup_warnings() -> Vec<String> {
    let files = agent_files();
    let mut lines = Vec::new();
    let stale: Vec<String> = files
        .iter()
        .filter(|(_, _, state)| matches!(state, FileState::Stale))
        .map(|(path, _, _)| crate::setup::short_path(path))
        .collect();
    if !stale.is_empty() {
        lines.push(format!(
            "The son files for agents are from another version of son: {}. \
             Tell the user to run `son init`. Do not run it yourself.",
            stale.join(", ")
        ));
    }
    for (path, _, state) in &files {
        if let FileState::Broken(fault) = state {
            lines.push(format!(
                "The son block in {} is broken: it has {fault}. \
                 Tell the user to run `son doctor` and to repair the file by hand.",
                crate::setup::short_path(path)
            ));
        }
    }
    lines
}

/// The files `son init` wrote: are they what this son would write now?
fn file_checks(checks: &mut Vec<Check>) {
    let files = agent_files();
    for (path, is_skill, state) in &files {
        let shown = crate::setup::short_path(path);
        let name = if *is_skill {
            "skill file"
        } else {
            "instruction file"
        };
        checks.push(match state {
            FileState::Broken(fault) => check(
                name,
                Status::Problem,
                format!("{shown} has {fault}. `son init` refuses to change this file"),
                Some(instructions::REPAIR),
            ),
            FileState::Current => check(name, Status::Healthy, shown, None),
            FileState::Stale => check(
                name,
                Status::Stale,
                format!("{shown} was written by another version of son"),
                Some("son init"),
            ),
        });
    }
    if files.is_empty() {
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
