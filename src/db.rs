use anyhow::{Context, Result};
use rusqlite::{Connection, ErrorCode};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The schema version this son reads and writes.
pub const SCHEMA_VERSION: i64 = 10;

/// Version 1 schema. The `origin`, `author`, `visibility` and `rev` columns
/// prepare for the version 2 central store; `rev` already tracks edits.
/// See todo #13, "Existing schema foundations".
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

/// Version 8 adds idempotency keys. A caller that is not sure a write
/// arrived (a lost answer, a timeout) sends it again with the same `--key`.
/// The row holds what was asked (`request`) and what was answered
/// (`result`), and is written in the same transaction as the node, so a key
/// is there if and only if its write happened. Keys are for the whole
/// database, not for one session.
const SCHEMA_V8: &str = "
CREATE TABLE idempotency (
    key         TEXT PRIMARY KEY,
    command     TEXT NOT NULL,
    request     TEXT NOT NULL,
    node_id     TEXT,
    result      TEXT NOT NULL,
    created_at  TEXT NOT NULL
);
CREATE INDEX idempotency_node ON idempotency(node_id);
";

/// Version 9: a node can be marked stale, with a reason. It is a label
/// that someone sets and clears; nothing becomes stale by itself.
const SCHEMA_V9: &str = "
ALTER TABLE nodes ADD COLUMN stale_at TEXT;
ALTER TABLE nodes ADD COLUMN stale_note TEXT;
";

/// Version 10: a todo can have a priority, `p0` (most urgent) to `p4`.
/// NULL means none was given; every older todo has none.
const SCHEMA_V10: &str = "
ALTER TABLE nodes ADD COLUMN priority TEXT;
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

/// Make the folder `dir` and the folders above it that are missing. On
/// macOS and Linux the new folders are for their owner only (mode 0700). A
/// folder that is there already is left as it is. On Windows the access
/// rules of the folder above apply.
pub fn private_dir(dir: &Path) -> std::io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// Make an empty file at `path` when there is none. On macOS and Linux it
/// is for its owner only (mode 0600). A file that is there already is left
/// as it is: someone may have opened it to others on purpose, and
/// `son doctor` reports it.
pub fn private_file(path: &Path) -> std::io::Result<()> {
    claim_private_file(path).map(|_| ())
}

/// [`private_file`], and say if this call made the file. Two sessions
/// that ask for the same new file at once: exactly one gets `true`.
fn claim_private_file(path: &Path) -> std::io::Result<bool> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(err) => Err(err),
    }
}

/// What opening the database did to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opened {
    /// There was no schema; it was made.
    Created,
    /// The schema was the one this son uses.
    Current,
    /// The schema was older and was brought up to date. `copy` is the copy
    /// of the database from before, when one was made.
    Upgraded { from: i64, copy: Option<PathBuf> },
}

impl Opened {
    /// One line for a person.
    pub fn line(&self) -> String {
        match self {
            Opened::Created => format!("database: created (schema {SCHEMA_VERSION})"),
            Opened::Current => format!("database: schema {SCHEMA_VERSION}, up to date"),
            Opened::Upgraded {
                from,
                copy: Some(copy),
            } => format!(
                "database: upgraded from schema {from} to {SCHEMA_VERSION}; the old database was copied to {}",
                copy.display()
            ),
            Opened::Upgraded { from, copy: None } => format!(
                "database: upgraded from schema {from} to {SCHEMA_VERSION}; no copy was made ({})",
                if upgrade_copy_wanted() {
                    "it held no nodes"
                } else {
                    "SON_NO_UPGRADE_BACKUP is set"
                }
            ),
        }
    }

    /// The same for `--json`.
    pub fn json(&self) -> serde_json::Value {
        let (state, from, copy) = match self {
            Opened::Created => ("created", None, None),
            Opened::Current => ("current", None, None),
            Opened::Upgraded { from, copy } => (
                "upgraded",
                Some(*from),
                copy.as_ref().map(|c| c.display().to_string()),
            ),
        };
        serde_json::json!({ "state": state, "schema": SCHEMA_VERSION, "from": from, "copy": copy })
    }
}

/// False when `SON_NO_UPGRADE_BACKUP` is set: upgrade the schema without
/// a copy of the database from before.
pub fn upgrade_copy_wanted() -> bool {
    std::env::var_os("SON_NO_UPGRADE_BACKUP").is_none_or(|v| v.is_empty() || v == "0")
}

/// The folder for the copies made before a schema upgrade: `backups`
/// beside the database.
pub fn upgrade_copy_dir(database: &Path) -> PathBuf {
    match database.parent().filter(|p| !p.as_os_str().is_empty()) {
        Some(dir) => dir.join("backups"),
        None => PathBuf::from("backups"),
    }
}

/// Open the database, creating and migrating it if needed.
pub fn open(path: &Path) -> Result<Connection> {
    open_reporting(path).map(|(conn, _)| conn)
}

/// [`open`], and say what was done. Before a schema upgrade of a database
/// that holds nodes, a copy of the database is written (see [`migrate`]).
pub fn open_reporting(path: &Path) -> Result<(Connection, Opened)> {
    open_with(path, upgrade_copy_wanted())
}

/// [`open`] with no copy before a schema upgrade. For a file that is
/// itself a temporary copy.
pub fn open_without_copy(path: &Path) -> Result<Connection> {
    open_with(path, false).map(|(conn, _)| conn)
}

fn open_with(path: &Path, copy: bool) -> Result<(Connection, Opened)> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        private_dir(parent).with_context(|| format!("cannot create {}", parent.display()))?;
    }
    // A new database is made for its owner only. SQLite gives its `-wal`
    // and `-shm` files the access mode of the database file.
    private_file(path).with_context(|| format!("cannot create {}", path.display()))?;
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
    let opened = migrate(&conn, path, copy)?;
    Ok((conn, opened))
}

/// Write a copy of the database as it is now, before its schema is
/// upgraded from `version`. SQLite makes the copy from one read of the
/// database, so it is whole. The file is new: a copy never replaces one.
/// `None` when another session upgraded the database in the meantime, so
/// that there is nothing old left to copy.
fn copy_before_upgrade(
    conn: &Connection,
    database: &Path,
    version: i64,
) -> Result<Option<PathBuf>> {
    let dir = upgrade_copy_dir(database);
    private_dir(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let stamp: String =
        conn.query_row("SELECT strftime('%Y%m%d-%H%M%S', 'now')", [], |r| r.get(0))?;
    // Take a name that no other session has: making the file is the claim.
    let mut number = 1;
    let target = loop {
        let name = if number == 1 {
            format!("son-before-schema-{version}-{stamp}.db")
        } else {
            format!("son-before-schema-{version}-{stamp}-{number}.db")
        };
        let target = dir.join(name);
        if claim_private_file(&target)
            .with_context(|| format!("cannot create {}", target.display()))?
        {
            break target;
        }
        number += 1;
    };
    let name = target
        .to_str()
        .map(|name| name.replace('\'', "''"))
        .with_context(|| format!("the path {} is not valid text", target.display()));
    let written = name.and_then(|name| {
        retry_busy(|| conn.execute_batch(&format!("VACUUM INTO '{name}'")))?;
        // Read it back before the upgrade relies on it.
        let copy =
            Connection::open_with_flags(&target, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let copied: i64 = copy.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let _: i64 = copy.query_row("SELECT count(*) FROM nodes", [], |r| r.get(0))?;
        Ok(copied)
    });
    match written {
        Ok(copied) if copied == version => Ok(Some(target)),
        Ok(SCHEMA_VERSION) => {
            let _ = std::fs::remove_file(&target);
            Ok(None)
        }
        Ok(copied) => {
            let _ = std::fs::remove_file(&target);
            anyhow::bail!("the copy is schema version {copied}, not {version}")
        }
        Err(err) => {
            let _ = std::fs::remove_file(&target);
            Err(err.context(format!("cannot write {}", target.display())))
        }
    }
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
        version => Err(crate::error::coded(
            "unsupported_schema",
            format!(
                "the database is schema version {version} and this son uses version {SCHEMA_VERSION}. \
                 A read-only command does not upgrade it; run `son init --db-only` once, then try again"
            ),
        )),
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

/// Bring the schema to [`SCHEMA_VERSION`].
///
/// Before an older database that holds nodes is upgraded, a copy of it is
/// written to the `backups` folder beside it (when `copy` is true). This
/// is the one backup that son makes without being asked. If the copy
/// cannot be written, nothing is upgraded.
///
/// The copy is made before the write lock is taken, because SQLite cannot
/// make it inside a transaction. All upgrade steps are one transaction, so
/// a copy holds the old schema whole or the new one whole, never a part;
/// a copy that is not of the old version is refused. Several sessions that
/// start at once can each make a copy of the same old state; one of them
/// does the upgrade, and the others remove their copy.
fn migrate(conn: &Connection, database: &Path, copy: bool) -> Result<Opened> {
    let version = user_version(conn)?;
    if version == SCHEMA_VERSION {
        return Ok(Opened::Current);
    }
    if version > SCHEMA_VERSION {
        return Err(crate::error::coded(
            "unsupported_schema",
            format!(
                "database is schema version {version}, but this son only knows version {SCHEMA_VERSION}; install a newer son"
            ),
        ));
    }
    let mut copied: Option<PathBuf> = None;
    if version >= 1 && copy {
        let nodes: i64 =
            retry_busy(|| conn.query_row("SELECT count(*) FROM nodes", [], |r| r.get(0)))
                .map_err(busy_error)?;
        if nodes > 0 {
            copied = copy_before_upgrade(conn, database, version).map_err(|err| {
                crate::error::coded(
                    "upgrade_backup_failed",
                    format!(
                        "the database was not upgraded from schema {version} to {SCHEMA_VERSION}, \
                         because the copy of the old database could not be written: {err:#}. \
                         Nothing was changed. Make room or correct the access of the folder and run \
                         the command again; or set SON_NO_UPGRADE_BACKUP=1 to upgrade with no copy"
                    ),
                )
            })?;
        }
    }
    let discard = |copied: &Option<PathBuf>| {
        if let Some(file) = copied {
            let _ = std::fs::remove_file(file);
        }
    };
    // Take the write lock, then look again: another session may have
    // created or upgraded the schema while this one waited.
    if let Err(err) = retry_busy(|| conn.execute_batch("BEGIN IMMEDIATE")) {
        discard(&copied);
        return Err(busy_error(err));
    }
    let before = match user_version(conn) {
        Ok(before) => before,
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK");
            discard(&copied);
            return Err(err);
        }
    };
    if before == SCHEMA_VERSION {
        // Another session did the upgrade, and made its own copy.
        let _ = conn.execute_batch("ROLLBACK");
        discard(&copied);
        return Ok(Opened::Current);
    }
    let result = (|| -> Result<()> {
        let version = before;
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
        if version < 8 {
            conn.execute_batch(SCHEMA_V8)?;
        }
        if version < 9 {
            conn.execute_batch(SCHEMA_V9)?;
        }
        if version < 10 {
            conn.execute_batch(SCHEMA_V10)?;
        }
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(if before < 1 {
                Opened::Created
            } else {
                Opened::Upgraded {
                    from: before,
                    copy: copied,
                }
            })
        }
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK");
            discard(&copied);
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

    /// A version 8 database with one collection and one note in it.
    fn version_8(path: &Path) {
        let conn = Connection::open(path).unwrap();
        for step in [
            SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7, SCHEMA_V8,
        ] {
            conn.execute_batch(step).unwrap();
        }
        conn.pragma_update(None, "user_version", 8).unwrap();
        conn.execute_batch(
            "INSERT INTO nodes (id, type, title, created_at, updated_at) \
             VALUES ('01C', 'collection', 'old', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'); \
             INSERT INTO nodes (id, type, title, project_id, created_at, updated_at) \
             VALUES ('01X', 'note', 'kept', '01C', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');",
        )
        .unwrap();
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("son-db-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn copies(dir: &Path) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = std::fs::read_dir(dir.join("backups"))
            .map(|entries| entries.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        found.sort();
        found
    }

    #[test]
    fn an_upgrade_first_writes_a_copy_of_the_old_database() {
        let dir = temp_dir("copy");
        let path = dir.join("son.db");
        version_8(&path);
        let (conn, opened) = open_with(&path, true).unwrap();
        assert_eq!(user_version(&conn).unwrap(), SCHEMA_VERSION);
        let found = copies(&dir);
        assert_eq!(found.len(), 1);
        assert_eq!(
            opened,
            Opened::Upgraded {
                from: 8,
                copy: Some(found[0].clone())
            }
        );
        let name = found[0].file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("son-before-schema-8-") && name.ends_with(".db"));
        // The copy is the old database: old schema, old rows.
        let copy = Connection::open(&found[0]).unwrap();
        assert_eq!(user_version(&copy).unwrap(), 8);
        let title: String = copy
            .query_row("SELECT title FROM nodes WHERE id = '01X'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "kept");
        assert!(copy.prepare("SELECT stale_at FROM nodes").is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&found[0]).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        // Opening again is no upgrade, and makes no second copy.
        drop(conn);
        assert_eq!(open_with(&path, true).unwrap().1, Opened::Current);
        assert_eq!(copies(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_copy_for_a_new_or_empty_database_or_when_switched_off() {
        let dir = temp_dir("nocopy");
        // New.
        assert_eq!(
            open_with(&dir.join("new.db"), true).unwrap().1,
            Opened::Created
        );
        // Old, with no nodes.
        let empty = dir.join("empty.db");
        version_8(&empty);
        Connection::open(&empty)
            .unwrap()
            .execute_batch("DELETE FROM nodes")
            .unwrap();
        assert_eq!(
            open_with(&empty, true).unwrap().1,
            Opened::Upgraded {
                from: 8,
                copy: None
            }
        );
        // Old, with nodes, and the copy switched off.
        let off = dir.join("off.db");
        version_8(&off);
        assert_eq!(
            open_with(&off, false).unwrap().1,
            Opened::Upgraded {
                from: 8,
                copy: None
            }
        );
        assert!(copies(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn when_the_copy_cannot_be_written_nothing_is_upgraded() {
        let dir = temp_dir("copyfail");
        let path = dir.join("son.db");
        version_8(&path);
        // A file where the folder for the copies must be.
        std::fs::write(dir.join("backups"), "in the way").unwrap();
        let err = open_with(&path, true).unwrap_err();
        assert_eq!(crate::error::code_of(&err), "upgrade_backup_failed");
        let message = format!("{err:#}");
        assert!(message.contains("SON_NO_UPGRADE_BACKUP=1"), "{message}");
        let conn = Connection::open(&path).unwrap();
        assert_eq!(user_version(&conn).unwrap(), 8);
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM nodes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 2);
        drop(conn);
        // The way past it that the message names.
        assert_eq!(
            open_with(&path, false).unwrap().1,
            Opened::Upgraded {
                from: 8,
                copy: None
            }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sessions_that_start_at_once_upgrade_once_and_keep_whole_copies() {
        let dir = temp_dir("copyrace");
        let path = dir.join("son.db");
        version_8(&path);
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || open_with(&path, true).map(|(_, opened)| opened))
            })
            .collect();
        let results: Vec<Opened> = handles
            .into_iter()
            .map(|h| h.join().unwrap().unwrap())
            .collect();
        let upgrades = results
            .iter()
            .filter(|r| matches!(r, Opened::Upgraded { .. }))
            .count();
        assert_eq!(upgrades, 1, "{results:?}");
        // Every copy that stays is a whole copy of the old schema.
        let found = copies(&dir);
        assert!(!found.is_empty());
        for file in found {
            let copy = Connection::open(&file).unwrap();
            assert_eq!(user_version(&copy).unwrap(), 8, "{}", file.display());
        }
        let _ = std::fs::remove_dir_all(&dir);
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
