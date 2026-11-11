//! Full backups, and restoring one.
//!
//! `son backup <destination>` writes one SQLite file that holds everything
//! in the database at one moment: nodes, links, tags, evidence, synonyms,
//! the history of changes, the activity log and the idempotency keys. It is
//! made by SQLite itself (`VACUUM INTO`), from one read transaction, so it
//! is consistent while other sessions write. Backups are made only when
//! asked for: no command makes one by itself.
//!
//! `son import <backup file>` puts such a file back. It is a full restore,
//! not a merge: the destination ends up holding exactly what the backup
//! holds. The JSON export (`son export --json`) is the other format: it is
//! portable, it merges on import, and it does not carry history, the log
//! or keys.

use crate::analyze::Analyzer;
use crate::db;
use crate::error::coded;
use crate::store;
use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// The first bytes of every SQLite file.
pub const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";

/// What a database holds, for reports.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Counts {
    pub nodes: i64,
    pub collections: i64,
    pub links: i64,
    pub history: i64,
    pub log: i64,
    pub keys: i64,
}

impl Counts {
    pub fn of(conn: &Connection, schema: &str) -> Result<Counts> {
        let one = |sql: String| -> Result<i64> { Ok(conn.query_row(&sql, [], |r| r.get(0))?) };
        Ok(Counts {
            nodes: one(format!(
                "SELECT count(*) FROM {schema}.nodes WHERE type != 'collection'"
            ))?,
            collections: one(format!(
                "SELECT count(*) FROM {schema}.nodes WHERE type = 'collection'"
            ))?,
            links: one(format!("SELECT count(*) FROM {schema}.edges"))?,
            history: one(format!("SELECT count(*) FROM {schema}.ops"))?,
            log: one(format!("SELECT count(*) FROM {schema}.events"))?,
            keys: one(format!("SELECT count(*) FROM {schema}.idempotency"))?,
        })
    }

    /// True when there is nothing a restore could destroy: no node, link,
    /// history, log entry or key.
    pub fn is_empty(&self) -> bool {
        self.nodes + self.collections + self.links + self.history + self.log + self.keys == 0
    }
}

#[derive(Serialize)]
pub struct Made {
    pub path: String,
    pub bytes: u64,
    #[serde(flatten)]
    pub counts: Counts,
}

fn sql_text(path: &Path) -> Result<String> {
    let text = path
        .to_str()
        .ok_or_else(|| coded("invalid_argument", "the path is not valid UTF-8"))?;
    Ok(text.replace('\'', "''"))
}

/// Write a full copy of the database to `destination`. A folder gets a new
/// file named after the time. A file that exists is never overwritten.
pub fn make(conn: &Connection, destination: &Path) -> Result<Made> {
    let target: PathBuf = if destination.is_dir() {
        let stamp: String =
            conn.query_row("SELECT strftime('%Y%m%d-%H%M%S', 'now')", [], |r| r.get(0))?;
        destination.join(format!("son-backup-{stamp}.db"))
    } else {
        destination.to_path_buf()
    };
    if target.exists() {
        return Err(coded(
            "invalid_request",
            format!(
                "{} already exists; a backup never overwrites a file. Choose another name",
                target.display()
            ),
        ));
    }
    if let Some(parent) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
        if !parent.is_dir() {
            return Err(coded(
                "invalid_request",
                format!("the folder {} does not exist", parent.display()),
            ));
        }
    }
    let made = db::retry_busy(|| {
        conn.execute_batch(&format!(
            "VACUUM INTO '{}'",
            sql_text(&target).unwrap_or_default()
        ))
    });
    if let Err(err) = made {
        let _ = std::fs::remove_file(&target);
        return Err(db::busy_error(err))
            .with_context(|| format!("cannot write {}", target.display()));
    }
    // Open what was written, as a restore would, before saying it is good.
    let check = || -> Result<Counts> {
        let copy = Connection::open_with_flags(&target, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        integrity(&copy, "main")?;
        Counts::of(&copy, "main")
    };
    let counts = match check() {
        Ok(counts) => counts,
        Err(err) => {
            let _ = std::fs::remove_file(&target);
            return Err(err).context("the backup did not pass its check and was removed");
        }
    };
    Ok(Made {
        bytes: std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0),
        path: target.display().to_string(),
        counts,
    })
}

fn integrity(conn: &Connection, schema: &str) -> Result<()> {
    let result: String = conn.query_row(&format!("PRAGMA {schema}.integrity_check"), [], |r| {
        r.get(0)
    })?;
    if result != "ok" {
        return Err(coded(
            "import_failed",
            format!("the file is damaged: {result}"),
        ));
    }
    Ok(())
}

/// True when the file starts as a SQLite database does.
pub fn is_sqlite(path: &Path) -> bool {
    use std::io::Read;
    let mut start = [0u8; 16];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut start))
        .is_ok()
        && start == SQLITE_MAGIC
}

/// The tables of a son database that hold data, children before parents.
const TABLES: [&str; 10] = [
    "idempotency",
    "edge_changes",
    "revisions",
    "ops",
    "events",
    "refs",
    "tags",
    "edges",
    "synonyms",
    "nodes",
];

#[derive(Serialize)]
pub struct Restored {
    pub restored: bool,
    pub from: String,
    /// The schema version of the backup file.
    pub schema: i64,
    pub replaced: Counts,
    #[serde(flatten)]
    pub counts: Counts,
}

/// Make the database hold exactly what the backup at `source` holds.
///
/// Order of work, so that a failure leaves the destination as it was:
/// 1. Check the source: it is a SQLite file, it is a son database, its
///    schema is one this son knows, and SQLite finds it whole.
/// 2. Copy it to a temporary file and bring the copy to the current
///    schema. The source is only read.
/// 3. In one transaction on the destination: refuse if it holds data and
///    `replace` was not given; else empty every table and fill it from
///    the copy. Other sessions wait for the transaction, and see either
///    the old content or the new, never a mix.
pub fn restore(conn: &Connection, an: &Analyzer, source: &Path, replace: bool) -> Result<Restored> {
    let failed = |what: String| coded("import_failed", what);
    let src = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|err| failed(format!("cannot open {}: {err}", source.display())))?;
    let schema: i64 = src
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|err| {
            failed(format!(
                "{} is not a readable SQLite file: {err}",
                source.display()
            ))
        })?;
    let tables: i64 = src
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name IN ('nodes', 'edges', 'tags', 'synonyms')",
            [],
            |r| r.get(0),
        )
        .map_err(|err| failed(format!("{} cannot be read: {err}", source.display())))?;
    if tables != 4 || schema < 1 {
        return Err(failed(format!(
            "{} is a SQLite file, but not a son database",
            source.display()
        )));
    }
    if schema > db::SCHEMA_VERSION {
        return Err(failed(format!(
            "the backup is schema version {schema}, but this son only knows version {}; install a newer son",
            db::SCHEMA_VERSION
        )));
    }
    integrity(&src, "main").map_err(|err| failed(format!("{err:#}")))?;

    // A private copy, upgraded to the current schema.
    let dir = std::env::temp_dir().join(format!(
        "son-restore-{}-{}",
        std::process::id(),
        ulid::Ulid::generate()
    ));
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let result = (|| -> Result<Restored> {
        let copy = dir.join("copy.db");
        src.execute_batch(&format!("VACUUM INTO '{}'", sql_text(&copy)?))
            .map_err(|err| failed(format!("cannot read the backup: {err}")))?;
        drop(db::open(&copy).map_err(|err| failed(format!("cannot upgrade the backup: {err:#}")))?);

        conn.execute_batch(&format!("ATTACH DATABASE '{}' AS backup", sql_text(&copy)?))?;
        let done = store::write(conn, || {
            let replaced = Counts::of(conn, "main")?;
            if !replaced.is_empty() && !replace {
                return Err(coded(
                    "not_empty",
                    format!(
                        "this database is not empty ({} nodes, {} history entries, {} log entries, {} keys). \
                         Restoring a backup replaces all of it. Add --replace to do that; \
                         make a backup first with `son backup <file>` if you may want it back. Nothing was changed",
                        replaced.nodes + replaced.collections,
                        replaced.history,
                        replaced.log,
                        replaced.keys
                    ),
                ));
            }
            conn.execute_batch("PRAGMA defer_foreign_keys = ON")?;
            for table in TABLES {
                conn.execute(&format!("DELETE FROM main.{table}"), [])?;
            }
            for table in TABLES.iter().rev() {
                let columns: Vec<String> = conn
                    .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))?
                    .query_map([], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                let columns = columns.join(", ");
                conn.execute(
                    &format!(
                        "INSERT INTO main.{table} ({columns}) SELECT {columns} FROM backup.{table}"
                    ),
                    [],
                )?;
            }
            // Keep the numbering of history entries going from where the
            // backup left it.
            conn.execute("DELETE FROM main.sqlite_sequence WHERE name = 'ops'", [])?;
            conn.execute(
                "INSERT INTO main.sqlite_sequence (name, seq) \
                 SELECT name, seq FROM backup.sqlite_sequence WHERE name = 'ops'",
                [],
            )?;
            store::reindex_all(conn, an)?;
            Ok(Restored {
                restored: true,
                from: source.display().to_string(),
                schema,
                replaced,
                counts: Counts::of(conn, "main")?,
            })
        });
        let _ = conn.execute_batch("DETACH DATABASE backup");
        done
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}
