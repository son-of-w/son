use anyhow::{bail, Context, Result};
use rusqlite::{Connection, ErrorCode};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SCHEMA_VERSION: i64 = 7;

/// Version 1 schema. The `origin`, `author`, `visibility` and `rev` columns
/// prepare for the version 2 central store; `rev` already tracks edits.
/// See todos/13.md, "Existing schema foundations".
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

/// Version 4 adds provenance. `evidence` says how the knowledge was learned
/// (NULL means unknown, which is what every older node is). `refs` holds
/// pointers to the evidence: son stores them and never opens them. The
/// `verified_*` columns record an explicit check, apart from `updated_at`:
/// an edit is not a verification.
const SCHEMA_V4: &str = "
ALTER TABLE nodes ADD COLUMN evidence TEXT;
ALTER TABLE nodes ADD COLUMN verified_at TEXT;
ALTER TABLE nodes ADD COLUMN verified_by TEXT;
ALTER TABLE nodes ADD COLUMN verified_note TEXT;
CREATE TABLE refs (
    node_id     TEXT NOT NULL REFERENCES nodes(id),
    kind        TEXT NOT NULL,
    value       TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    PRIMARY KEY (node_id, kind, value)
);
";

/// Version 5: the groups that nodes belong to are called collections, not
/// projects, because a group can hold household or travel memory as well as
/// a code repository. The node type is renamed; ids, names and members stay.
/// The `project_id` column and the `events.project` column keep their names:
/// they are internal.
const SCHEMA_V5: &str = "
UPDATE nodes SET type = 'collection' WHERE type = 'project';
";

/// Version 6 adds change history. An `ops` row is one command that changed
/// nodes or links. `revisions` holds each changed node before and after, as
/// JSON; `edge_changes` holds each link added or removed. Nothing is pruned.
/// `revisions.node_id` has no foreign key: `son drop --purge` deletes the
/// node and these rows together. A database from before this version starts
/// with no history; the first change to a node records how it was.
const SCHEMA_V6: &str = "
CREATE TABLE ops (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    at         TEXT NOT NULL,
    kind       TEXT NOT NULL,
    source     TEXT,
    author     TEXT,
    undoes     INTEGER,
    undone_by  INTEGER,
    purged     INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE revisions (
    id       INTEGER PRIMARY KEY,
    op_id    INTEGER NOT NULL REFERENCES ops(id),
    node_id  TEXT NOT NULL,
    before   TEXT,
    after    TEXT NOT NULL
);
CREATE INDEX revisions_node ON revisions(node_id);
CREATE INDEX revisions_op ON revisions(op_id);
CREATE TABLE edge_changes (
    id          INTEGER PRIMARY KEY,
    op_id       INTEGER NOT NULL REFERENCES ops(id),
    from_id     TEXT NOT NULL,
    to_id       TEXT NOT NULL,
    relation    TEXT NOT NULL,
    added       INTEGER NOT NULL,
    created_at  TEXT NOT NULL,
    origin      TEXT NOT NULL DEFAULT 'local'
);
CREATE INDEX edge_changes_op ON edge_changes(op_id);
CREATE INDEX edge_changes_from ON edge_changes(from_id);
CREATE INDEX edge_changes_to ON edge_changes(to_id);
";

/// Version 7: a node that was merged into another keeps the id of the node
/// that took its place, so that an old id still leads somewhere.
const SCHEMA_V7: &str = "
ALTER TABLE nodes ADD COLUMN merged_into TEXT;
";

/// `SON_DB` if set, else `.son-of-w/son.db` in the home directory.
pub fn path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("SON_DB").filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    let home = crate::agents::home()
        .context("the home directory is not known; set SON_DB to choose a database path")?;
    Ok(home.join(".son-of-w").join("son.db"))
}

/// Open the database, creating and migrating it if needed.
pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    let conn = Connection::open(path)
        .with_context(|| format!("cannot open database {}", path.display()))?;
    conn.busy_timeout(busy_wait())?;
    // Switching a new database to WAL needs the file to itself for a moment.
    // When many sessions open a brand-new database at once, SQLite refuses
    // the others at once, without waiting, so wait and try again here. A
    // database that is already in WAL mode is left as it is.
    retry_busy(|| {
        let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        }
        Ok(())
    })
    .map_err(busy_error)
    .context("cannot switch the database to WAL mode")?;
    // In WAL mode this keeps the database safe from corruption and skips a
    // disk flush on every commit, which matters now that every command
    // writes a row to the activity log.
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", true)?;
    migrate(&conn)?;
    Ok(conn)
}

/// Open an existing database for reading only. Nothing is created, changed
/// or migrated: `None` when there is no database yet (or an empty file).
/// For commands that promise to change nothing, such as
/// `son remember --check`.
pub fn open_read_only(path: &Path) -> Result<Option<Connection>> {
    if !path.exists() {
        return Ok(None);
    }
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("cannot open database {}", path.display()))?;
    conn.busy_timeout(busy_wait())?;
    match user_version(&conn)? {
        0 => Ok(None),
        SCHEMA_VERSION => Ok(Some(conn)),
        version => bail!(
            "the database is schema version {version} and this son uses version {SCHEMA_VERSION}. \
             A read-only command does not upgrade it; run `son init --db-only` once, then try again"
        ),
    }
}

/// How long to wait for another session to finish with the database: ten
/// seconds, or the number of milliseconds in `SON_BUSY_MS`. The wait has an
/// end so that a session that holds the lock for good makes the others fail
/// with a clear message and not hang.
pub fn busy_wait() -> Duration {
    static WAIT: std::sync::OnceLock<Duration> = std::sync::OnceLock::new();
    *WAIT.get_or_init(|| {
        std::env::var("SON_BUSY_MS")
            .ok()
            .and_then(|ms| ms.trim().parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_secs(10))
    })
}

/// True when SQLite refused because another connection holds a lock.
pub fn is_busy(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(e, _)
            if matches!(e.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

/// Say what a lock error means and what to do. Other errors pass unchanged.
pub fn busy_error(err: rusqlite::Error) -> anyhow::Error {
    if is_busy(&err) {
        anyhow::Error::new(err).context(format!(
            "the database is in use by another session; gave up after {} ms. Try again",
            busy_wait().as_millis()
        ))
    } else {
        err.into()
    }
}

/// Run `f`, and run it again for as long as SQLite answers "busy" or
/// "locked", up to [`busy_wait`]. SQLite's own busy timeout covers most
/// waits, but not all: some locks are refused at once (changing the journal
/// mode, WAL recovery, a read that must become a write).
pub fn retry_busy<T>(mut f: impl FnMut() -> rusqlite::Result<T>) -> rusqlite::Result<T> {
    let deadline = Instant::now() + busy_wait();
    let mut pause = Duration::from_millis(5);
    loop {
        match f() {
            Err(err) if is_busy(&err) && Instant::now() < deadline => {
                std::thread::sleep(pause);
                pause = (pause * 2).min(Duration::from_millis(100));
            }
            other => return other,
        }
    }
}

fn user_version(conn: &Connection) -> Result<i64> {
    retry_busy(|| conn.query_row("PRAGMA user_version", [], |r| r.get(0))).map_err(busy_error)
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
    retry_busy(|| conn.execute_batch("BEGIN IMMEDIATE")).map_err(busy_error)?;
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
        if version < 4 {
            conn.execute_batch(SCHEMA_V4)?;
        }
        if version < 5 {
            conn.execute_batch(SCHEMA_V5)?;
        }
        if version < 6 {
            conn.execute_batch(SCHEMA_V6)?;
        }
        if version < 7 {
            conn.execute_batch(SCHEMA_V7)?;
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
                 VALUES ('01P', 'project', 'old-project', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'); \
                 INSERT INTO nodes (id, type, title, project_id, created_at, updated_at) \
                 VALUES ('01X', 'note', 'kept', '01P', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'); \
                 INSERT INTO nodes_fts (node_id, title, tags, body) VALUES ('01X', 'kept', '', '');",
            )
            .unwrap();
        }
        let conn = open(&path).unwrap();
        assert_eq!(user_version(&conn).unwrap(), SCHEMA_VERSION);
        // A project of an older database is now a collection, with its members.
        let kept = crate::store::get_node(&conn, "01X").unwrap();
        assert_eq!(kept.project.as_deref(), Some("old-project"));
        assert_eq!(
            crate::store::get_node(&conn, "01P").unwrap().kind,
            "collection"
        );
        assert_eq!(
            crate::store::find_project(&conn, "old-project")
                .unwrap()
                .as_deref(),
            Some("01P")
        );
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
        // An older node has unknown provenance and is not verified.
        let node = crate::store::get_node(&conn, "01X").unwrap();
        assert_eq!(node.evidence, None);
        assert_eq!(node.verified_at, None);
        assert!(node.refs.is_empty());
    }

    #[test]
    fn a_migration_that_fails_leaves_the_database_as_it_was() {
        let path = temp_db("failed-upgrade");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA_V1).unwrap();
            conn.execute_batch(SCHEMA_V2).unwrap();
            conn.pragma_update(None, "user_version", 2).unwrap();
            // A table in the way makes the next step of the upgrade fail.
            conn.execute_batch(
                "CREATE TABLE events (x); \
                 INSERT INTO nodes (id, type, title, created_at, updated_at) \
                 VALUES ('01X', 'note', 'kept', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');",
            )
            .unwrap();
        }
        assert!(open(&path).is_err());
        {
            // Nothing of the failed upgrade stays: same version, same rows.
            let conn = Connection::open(&path).unwrap();
            assert_eq!(user_version(&conn).unwrap(), 2);
            let kept: i64 = conn
                .query_row("SELECT count(*) FROM nodes", [], |r| r.get(0))
                .unwrap();
            assert_eq!(kept, 1);
            conn.execute_batch("DROP TABLE events").unwrap();
        }
        // With the cause removed, the same file upgrades.
        let conn = open(&path).unwrap();
        assert_eq!(user_version(&conn).unwrap(), SCHEMA_VERSION);
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
