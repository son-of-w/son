use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::time::Duration;

const SCHEMA_VERSION: i64 = 3;

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

/// Version 2 adds a view of the search index's vocabulary: every indexed
/// term and how many nodes hold it. Fuzzy matching reads it to find the
/// stored words nearest to a mistyped one. It holds no data of its own.
const SCHEMA_V2: &str = "
CREATE VIRTUAL TABLE nodes_vocab USING fts5vocab('nodes_fts', 'row');
";

/// Version 3 adds the activity log: one row for every command that is run.
/// It is local bookkeeping for `son log` and `son stats`; it is not exported
/// and will not be synced, so a plain counter is fine as its key.
const SCHEMA_V3: &str = "
CREATE TABLE events (
    id       INTEGER PRIMARY KEY,
    at       TEXT NOT NULL,
    command  TEXT NOT NULL,
    kind     TEXT NOT NULL,
    ok       INTEGER NOT NULL,
    project  TEXT,
    source   TEXT,
    node_id  TEXT,
    detail   TEXT,
    results  INTEGER,
    close    INTEGER,
    ms       INTEGER NOT NULL DEFAULT 0,
    error    TEXT
);
CREATE INDEX events_at ON events(at);
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
    // In WAL mode this keeps the database safe from corruption and skips a
    // disk flush on every commit, which matters now that every command
    // writes a row to the activity log.
    conn.pragma_update(None, "synchronous", "NORMAL")?;
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
        let version = user_version(conn)?;
        if version < 1 {
            conn.execute_batch(SCHEMA_V1)?;
        }
        if version < 2 {
            conn.execute_batch(SCHEMA_V2)?;
        }
        if version < 3 {
            conn.execute_batch(SCHEMA_V3)?;
        }
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("son-db-{name}-{}.db", std::process::id()));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        path
    }

    #[test]
    fn a_version_1_database_is_brought_up_to_date_and_keeps_its_rows() {
        let path = temp_db("upgrade");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA_V1).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
            conn.execute_batch(
                "INSERT INTO nodes (id, type, title, created_at, updated_at) \
                 VALUES ('01X', 'note', 'kept', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'); \
                 INSERT INTO nodes_fts (node_id, title, tags, body) VALUES ('01X', 'kept', '', '');",
            )
            .unwrap();
        }
        let conn = open(&path).unwrap();
        assert_eq!(user_version(&conn).unwrap(), SCHEMA_VERSION);
        let title: String = conn
            .query_row("SELECT title FROM nodes WHERE id = '01X'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "kept");
        let terms: i64 = conn
            .query_row(
                "SELECT count(*) FROM nodes_vocab WHERE term = 'kept'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(terms, 1);
        let events: i64 = conn
            .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(events, 0);
    }

    #[test]
    fn a_newer_database_is_refused() {
        let path = temp_db("newer");
        {
            let conn = open(&path).unwrap();
            conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
                .unwrap();
        }
        assert!(open(&path).is_err());
    }
}
