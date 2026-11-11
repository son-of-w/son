use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::time::Duration;

const SCHEMA_VERSION: i64 = 1;

/// Version 1 schema. The `origin`, `author`, `visibility` and `rev` columns
/// are not used yet; they exist so the version 2 central store needs no
/// migration of existing data (see PLAN.md, "Sync-ready columns").
const SCHEMA_V1: &str = "
CREATE TABLE nodes (
    id          TEXT PRIMARY KEY,
    type        TEXT NOT NULL,
    title       TEXT NOT NULL,
    body        TEXT NOT NULL DEFAULT '',
    status      TEXT,
    project_id  TEXT REFERENCES nodes(id),
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    deleted_at  TEXT,
    source      TEXT,
    origin      TEXT NOT NULL DEFAULT 'local',
    author      TEXT,
    visibility  TEXT NOT NULL DEFAULT 'private',
    rev         INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX nodes_project ON nodes(project_id);
CREATE INDEX nodes_type ON nodes(type, status);

CREATE TABLE edges (
    from_id     TEXT NOT NULL REFERENCES nodes(id),
    to_id       TEXT NOT NULL REFERENCES nodes(id),
    relation    TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    origin      TEXT NOT NULL DEFAULT 'local',
    PRIMARY KEY (from_id, to_id, relation)
);
CREATE INDEX edges_to ON edges(to_id);

CREATE TABLE tags (
    node_id     TEXT NOT NULL REFERENCES nodes(id),
    tag         TEXT NOT NULL,
    PRIMARY KEY (node_id, tag)
);
CREATE INDEX tags_tag ON tags(tag);

CREATE TABLE synonyms (
    term        TEXT PRIMARY KEY,
    canonical   TEXT NOT NULL
);

CREATE VIRTUAL TABLE nodes_fts USING fts5(
    node_id UNINDEXED,
    title,
    tags,
    body,
    tokenize = 'unicode61 remove_diacritics 2'
);

INSERT INTO synonyms (term, canonical) VALUES
    ('tf', 'terraform'),
    ('k8s', 'kubernetes'),
    ('kube', 'kubernetes'),
    ('pg', 'postgres'),
    ('postgresql', 'postgres'),
    ('js', 'javascript'),
    ('ts', 'typescript'),
    ('py', 'python'),
    ('gh', 'github'),
    ('db', 'database'),
    ('config', 'configuration'),
    ('env', 'environment'),
    ('repo', 'repository'),
    ('auth', 'authentication'),
    ('infra', 'infrastructure');
";

/// `SON_DB` if set, else `~/.son-of-w/son.db`.
pub fn path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("SON_DB").filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    let home = std::env::var_os("HOME")
        .context("HOME is not set; set SON_DB to choose a database path")?;
    Ok(PathBuf::from(home).join(".son-of-w").join("son.db"))
}

/// Open the database, creating and migrating it if needed.
pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    let conn = Connection::open(path)
        .with_context(|| format!("cannot open database {}", path.display()))?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
    conn.pragma_update(None, "foreign_keys", true)?;
    migrate(&conn)?;
    Ok(conn)
}

fn user_version(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

fn migrate(conn: &Connection) -> Result<()> {
    let version = user_version(conn)?;
    if version == SCHEMA_VERSION {
        return Ok(());
    }
    if version > SCHEMA_VERSION {
        bail!(
            "database is schema version {version}, but this son only knows version {SCHEMA_VERSION}; install a newer son"
        );
    }
    // Take the write lock, then look again: another session may have
    // created the schema while this one waited.
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> Result<()> {
        if user_version(conn)? < 1 {
            conn.execute_batch(SCHEMA_V1)?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(err)
        }
    }
}
