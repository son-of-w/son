//! A collection kept as files in its project folder, so that it can be
//! committed and shared through git.
//!
//! The `.son` file of a folder turns this on with `files = <folder>`. The
//! database stays the place every command reads from; the folder is a
//! second copy of one collection, and [`sync`] keeps the two in step:
//!
//! ```text
//! .son-data/
//!   collection.json        the format version and the id of the collection
//!   nodes/<id>.json        one node, with the links that start at it
//!   purged/<id>            an empty file for each node that was purged
//! ```
//!
//! **In.** A node file that changed since it was last read comes into the
//! database with the rules of `son import`: the content that was edited
//! last stays. The links of a node are part of its file, so they follow
//! the node. A file that cannot be used is skipped and named; it never
//! stops a command. A node that has a file in `purged` is purged here too.
//!
//! **Out.** A node whose file is missing or says something else is
//! written. A file is only removed by a purge: a file that was deleted by
//! hand comes back.
//!
//! **Ties.** When the database and a file have the same time of last edit
//! but not the same content, neither copy would give way on another
//! machine. The node here then gets the time of now before it is written,
//! so that this copy is the later one everywhere. A todo number that had
//! to change reaches the other machines this way.
//!
//! One file for each node means that two people who each add a node never
//! meet in git. Git only reports a conflict when both changed one node.

use crate::analyze::Analyzer;
use crate::history::{self, Recorder};
use crate::project::Shared;
use crate::store::{self, Meta, Node, Ref, NOW};
use crate::transfer::{self, Dump, Renumbered, Rules};
use anyhow::{anyhow, bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The version of the file format. A folder of a later version is left
/// alone: an older `son` would write its files without what it does not
/// know.
pub const FORMAT: u32 = 1;
/// The folder that `son files on` uses when none is named.
pub const DEFAULT_FOLDER: &str = ".son-data";
const COLLECTION_FILE: &str = "collection.json";
const NODES: &str = "nodes";
const PURGED: &str = "purged";
const ATTRIBUTES: &str = ".gitattributes";
/// Line ends stay as `son` writes them on every system, so that a file is
/// the same bytes for everyone.
const ATTRIBUTES_TEXT: &str =
    "# Written by son. The files are the same bytes on every system.\n* text eol=lf\n";
const OTHER_COLLECTION: &str = "this node is in another collection in this database";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct Link {
    relation: String,
    to: String,
}

/// One node as it is in its file. The order of the fields is the order in
/// the file. What belongs to one machine is not here: the collection (the
/// folder is the collection), the revision counter and the session name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileNode {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    priority: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    number: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    evidence: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    refs: Vec<Ref>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    links: Vec<Link>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    author: Option<String>,
    created_at: String,
    updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    deleted_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    merged_into: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verified_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verified_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verified_note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stale_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stale_note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    freshness_at: Option<String>,
}

impl FileNode {
    fn of(node: &Node, mut links: Vec<Link>) -> Self {
        links.sort();
        links.dedup();
        let mut refs = node.refs.clone();
        refs.sort();
        refs.dedup();
        Self {
            id: node.id.clone(),
            kind: node.kind.clone(),
            title: node.title.clone(),
            body: node.body.clone(),
            status: node.status.clone(),
            priority: node.priority.clone(),
            number: node.number.clone(),
            tags: node.tags.clone(),
            evidence: node.evidence.clone(),
            refs,
            links,
            author: node.author.clone(),
            created_at: node.created_at.clone(),
            updated_at: node.updated_at.clone(),
            deleted_at: node.deleted_at.clone(),
            merged_into: node.merged_into.clone(),
            verified_at: node.verified_at.clone(),
            verified_by: node.verified_by.clone(),
            verified_note: node.verified_note.clone(),
            stale_at: node.stale_at.clone(),
            stale_note: node.stale_note.clone(),
            freshness_at: node.freshness_at.clone(),
        }
    }

    /// The same node in the one form `son` writes: tags, references and
    /// links in order and once each.
    fn tidy(mut self) -> Self {
        self.tags = store::clean_tags(&self.tags);
        self.tags.sort();
        self.tags.dedup();
        self.refs.sort();
        self.refs.dedup();
        self.links.sort();
        self.links.dedup();
        if self.kind != "todo" {
            self.number = None;
        }
        self
    }

    /// The node for the database, in the collection `project_id`. What is
    /// not in a file stays as the stored node has it.
    fn node(&self, project_id: &str, stored: Option<&Node>) -> Node {
        Node {
            id: self.id.clone(),
            kind: self.kind.clone(),
            title: self.title.clone(),
            body: self.body.clone(),
            status: self.status.clone(),
            project_id: Some(project_id.to_string()),
            project: None,
            tags: self.tags.clone(),
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
            deleted_at: self.deleted_at.clone(),
            source: stored.and_then(|s| s.source.clone()),
            author: self.author.clone(),
            origin: stored.map_or_else(|| "local".to_string(), |s| s.origin.clone()),
            visibility: stored.map_or_else(|| "private".to_string(), |s| s.visibility.clone()),
            rev: stored.map_or(1, |s| s.rev + 1),
            evidence: self.evidence.clone(),
            verified_at: self.verified_at.clone(),
            verified_by: self.verified_by.clone(),
            verified_note: self.verified_note.clone(),
            refs: self.refs.clone(),
            merged_into: self.merged_into.clone(),
            stale_at: self.stale_at.clone(),
            stale_note: self.stale_note.clone(),
            priority: self.priority.clone(),
            freshness_at: self.freshness_at.clone(),
            number: self.number.clone(),
        }
    }

    /// The text of the file: always the same bytes for the same node.
    fn text(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_default();
        text.push('\n');
        text
    }

    /// Two short hashes: of the whole node, and of the node without its
    /// stale mark and verification. Those marks have a time of their own
    /// and do not make a node "edited".
    fn hashes(&self) -> (String, String) {
        let mut content = self.clone();
        content.verified_at = None;
        content.verified_by = None;
        content.verified_note = None;
        content.stale_at = None;
        content.stale_note = None;
        content.freshness_at = None;
        (hash(&self.text()), hash(&content.text()))
    }
}

/// FNV-1a, 64 bits. It only has to tell "the same as last time" from "not
/// the same", and it must give the same answer in every release, which the
/// hasher of the standard library does not promise.
fn hash(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        h ^= u64::from(byte);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

#[derive(Serialize, Deserialize)]
struct CollectionFile {
    version: u32,
    id: String,
    created_at: String,
}

/// A file that was not used, and why.
#[derive(Debug, Clone, Serialize)]
pub struct Skipped {
    pub file: String,
    pub reason: String,
}

/// What one sync did.
#[derive(Default, Serialize)]
pub struct Summary {
    /// Nodes that came in from files and were not in the database.
    pub added: usize,
    /// Nodes in the database that a file changed.
    pub updated: usize,
    /// Nodes that were purged because the folder says so.
    pub purged: usize,
    /// Files that were written.
    pub written: usize,
    /// Todos that got another number.
    pub renumbered: Vec<Renumbered>,
    /// Files that were skipped.
    pub skipped: Vec<Skipped>,
    /// True when the database was in use and the sync was left for later.
    #[serde(skip)]
    pub busy: bool,
}

impl Summary {
    /// One line for stderr, or `None` when there is nothing to say.
    /// Writing files is not news: every write does it.
    pub fn line(&self, folder: &str) -> Option<String> {
        let mut parts = Vec::new();
        if self.added > 0 {
            parts.push(format!("{} new", self.added));
        }
        if self.updated > 0 {
            parts.push(format!("{} changed", self.updated));
        }
        if self.purged > 0 {
            parts.push(format!("{} purged", self.purged));
        }
        for moved in &self.renumbered {
            parts.push(format!(
                "todo #{} is now #{} ({})",
                moved.from, moved.to, moved.title
            ));
        }
        (!parts.is_empty()).then(|| format!("read {folder}: {}", parts.join(", ")))
    }
}

/// A node file as the database last saw it.
#[derive(Debug, Clone)]
struct Record {
    size: i64,
    mtime: i64,
    format: i64,
    full: Option<String>,
    content: Option<String>,
    updated_at: Option<String>,
    canonical: bool,
    problem: Option<String>,
}

/// The name of a folder in the tables: its real path, so that two ways to
/// write one folder are one row.
fn root_key(root: &Path) -> String {
    std::fs::canonicalize(root)
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// The id in the name of a node file, for a name of the right form.
fn id_of(name: &str) -> Option<&str> {
    name.strip_suffix(".json").filter(|id| transfer::is_id(id))
}

fn stat(path: &Path) -> Option<(i64, i64)> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((meta.len() as i64, mtime as i64))
}

/// The node files in the folder: name, size and time of the last write.
fn node_files(dir: &Path) -> HashMap<String, (i64, i64)> {
    let Ok(entries) = std::fs::read_dir(dir.join(NODES)) else {
        return HashMap::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            id_of(&name)?;
            Some((name, stat(&entry.path())?))
        })
        .collect()
}

/// The ids that have a file in `purged`.
fn purged_files(dir: &Path) -> BTreeSet<String> {
    let Ok(entries) = std::fs::read_dir(dir.join(PURGED)) else {
        return BTreeSet::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| transfer::is_id(name))
        .collect()
}

fn records(conn: &Connection, root: &str) -> Result<HashMap<String, Record>> {
    let mut stmt = conn.prepare(
        "SELECT name, size, mtime, format, full, content, updated_at, canonical, problem \
         FROM shared_files WHERE root = ?1",
    )?;
    let rows = stmt
        .query_map([root], |r| {
            Ok((
                r.get::<_, String>(0)?,
                Record {
                    size: r.get(1)?,
                    mtime: r.get(2)?,
                    format: r.get(3)?,
                    full: r.get(4)?,
                    content: r.get(5)?,
                    updated_at: r.get(6)?,
                    canonical: r.get(7)?,
                    problem: r.get(8)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// How long after a file was written its time of last write is not yet
/// trusted. Some file systems keep that time in whole seconds: a file that
/// is changed again in the same second, to a text of the same size, would
/// look unchanged. Such a young file is read once more by a later command.
const YOUNG: std::time::Duration = std::time::Duration::from_secs(2);

fn save_record(conn: &Connection, root: &str, name: &str, record: &Record) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64);
    let young = now.saturating_sub(record.mtime) < YOUNG.as_nanos() as i64;
    let mtime = if young { -1 } else { record.mtime };
    conn.execute(
        "INSERT INTO shared_files (root, name, size, mtime, format, full, content, updated_at, canonical, problem) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
         ON CONFLICT(root, name) DO UPDATE SET size = excluded.size, mtime = excluded.mtime, \
            format = excluded.format, full = excluded.full, content = excluded.content, \
            updated_at = excluded.updated_at, canonical = excluded.canonical, problem = excluded.problem",
        params![
            root,
            name,
            record.size,
            mtime,
            record.format,
            record.full,
            record.content,
            record.updated_at,
            record.canonical,
            record.problem
        ],
    )?;
    Ok(())
}

fn forget_record(conn: &Connection, root: &str, name: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM shared_files WHERE root = ?1 AND name = ?2",
        params![root, name],
    )?;
    Ok(())
}

/// A short text that changes when the nodes or links of the collection
/// change. It lets a command see, without reading every node, that the
/// files may be behind.
fn state(conn: &Connection, project_id: &str) -> Result<String> {
    Ok(conn.query_row(
        "SELECT (SELECT count(*) || ':' || coalesce(max(updated_at), '') || ':' || \
                        coalesce(max(freshness_at), '') || ':' || coalesce(sum(rev), 0) \
                 FROM nodes WHERE project_id = ?1) \
             || ':' || \
                (SELECT count(*) || ':' || coalesce(max(e.created_at), '') \
                 FROM edges e JOIN nodes n ON n.id = e.from_id WHERE n.project_id = ?1)",
        [project_id],
        |r| r.get(0),
    )?)
}

/// The purged ids of the collection that the database knows.
fn purged_known(conn: &Connection, project_id: &str) -> Result<BTreeSet<String>> {
    let ids = conn
        .prepare("SELECT id FROM shared_purged WHERE project_id = ?1")?
        .query_map([project_id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(ids)
}

/// Call before a node is purged: when its collection is kept as files
/// somewhere, the purge is remembered, so that the folder can pass it on.
pub fn note_purge(conn: &Connection, node: &Node) -> Result<()> {
    let Some(project_id) = node.project_id.as_deref() else {
        return Ok(());
    };
    conn.execute(
        "INSERT OR IGNORE INTO shared_purged (project_id, id) \
         SELECT ?1, ?2 WHERE EXISTS (SELECT 1 FROM shared_folders WHERE project_id = ?1)",
        params![project_id, node.id],
    )?;
    Ok(())
}

fn read_collection_file(dir: &Path) -> Result<Option<CollectionFile>> {
    let path = dir.join(COLLECTION_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => bail!("cannot read {}: {err}", path.display()),
    };
    let file: CollectionFile = serde_json::from_str(&text).map_err(|err| {
        anyhow!(
            "{} cannot be read ({err}); repair it, or delete it and son writes it again",
            path.display()
        )
    })?;
    if file.version > FORMAT {
        bail!(
            "{} is of format {}, and this son knows format {FORMAT}; install a newer son",
            dir.display(),
            file.version
        );
    }
    if !transfer::is_id(&file.id) || !transfer::is_time(&file.created_at) {
        bail!(
            "{} does not hold an id and a time; repair it, or delete it and son writes it again",
            path.display()
        );
    }
    Ok(Some(file))
}

/// Write `text` to `path` so that a reader sees the old file or the new
/// one, never a part.
fn write_file(path: &Path, text: &str) -> Result<()> {
    let folder = path.parent().context("the file has no folder")?;
    std::fs::create_dir_all(folder).with_context(|| format!("cannot make {}", folder.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let part = folder.join(format!(".{name}.{}.part", std::process::id()));
    std::fs::write(&part, text).with_context(|| format!("cannot write {}", part.display()))?;
    std::fs::rename(&part, path).map_err(|err| {
        let _ = std::fs::remove_file(&part);
        anyhow!("cannot write {}: {err}", path.display())
    })
}

/// Which nodes a sync compares with their files.
pub enum Scope<'a> {
    /// The nodes whose file changed or is missing; every node when the
    /// database says that the collection changed since the last sync.
    Changed,
    /// As `Changed`, and these nodes: the ones a command just changed.
    /// Nothing else is read, so a write stays fast in a large collection.
    Nodes(&'a HashSet<String>),
    /// Every node of the collection.
    All,
}

/// What is known before any change is made. For [`Scope::Nodes`], `files`
/// and `records` hold those nodes only.
struct Look {
    files: HashMap<String, (i64, i64)>,
    records: HashMap<String, Record>,
    purged: BTreeSet<String>,
    /// The files to read: new, or not as they were last seen.
    changed: Vec<String>,
    /// True when there is something to read, write or note.
    work: bool,
    /// True when the database and the folder were in step at the last
    /// sync, and the collection did not change since.
    in_step: bool,
}

fn look(
    conn: &Connection,
    dir: &Path,
    root: &str,
    name: &str,
    has_collection_file: bool,
    only: Option<&HashSet<String>>,
) -> Result<Look> {
    let (files, records) = match only {
        None => (node_files(dir), records(conn, root)?),
        // Only these nodes: their files are looked at one by one, and the
        // folder is not listed. A large folder costs a write nothing.
        Some(ids) => {
            let names: HashSet<String> = ids.iter().map(|id| format!("{id}.json")).collect();
            let files = names
                .iter()
                .filter(|name| id_of(name).is_some())
                .filter_map(|name| Some((name.clone(), stat(&dir.join(NODES).join(name))?)))
                .collect();
            let mut records = records(conn, root)?;
            records.retain(|name, _| names.contains(name));
            (files, records)
        }
    };
    let purged = purged_files(dir);
    let mut changed: Vec<String> = files
        .iter()
        .filter(|(name, (size, mtime))| {
            records.get(*name).is_none_or(|r| {
                r.size != *size || r.mtime != *mtime || r.format != i64::from(FORMAT)
            })
        })
        .map(|(name, _)| name.clone())
        .collect();
    changed.sort();
    let missing = records.keys().any(|name| !files.contains_key(name));
    let registered: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT project_id, state FROM shared_folders WHERE root = ?1",
            [root],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let project_id = store::find_project(conn, name)?;
    let in_step = match (&registered, &project_id) {
        (Some((seen, Some(seen_state))), Some(id)) if seen == id => {
            *seen_state == state(conn, id)? && purged_known(conn, id)? == purged
        }
        _ => false,
    };
    let work = !in_step || !has_collection_file || missing || !changed.is_empty();
    Ok(Look {
        files,
        records,
        purged,
        changed,
        work,
        in_step,
    })
}

/// Take the write lock. With `patient`, wait for it as every write does.
/// Without, try once: `false` when another session holds the database.
fn begin(conn: &Connection, patient: bool) -> Result<bool> {
    if patient {
        crate::db::retry_busy(|| conn.execute_batch("BEGIN IMMEDIATE"))
            .map_err(crate::db::busy_error)?;
        return Ok(true);
    }
    conn.busy_timeout(std::time::Duration::ZERO)?;
    let result = conn.execute_batch("BEGIN IMMEDIATE");
    conn.busy_timeout(crate::db::busy_wait())?;
    match result {
        Ok(()) => Ok(true),
        Err(err) if crate::db::is_busy(&err) => Ok(false),
        Err(err) => Err(err.into()),
    }
}

/// The files that the database knows were skipped, from the last read.
fn known_skipped(look: &Look) -> Vec<Skipped> {
    let mut skipped: Vec<Skipped> = look
        .records
        .iter()
        .filter(|(name, _)| look.files.contains_key(*name))
        .filter_map(|(name, record)| {
            Some(Skipped {
                file: name.clone(),
                reason: record.problem.clone()?,
            })
        })
        .collect();
    skipped.sort_by(|a, b| a.file.cmp(&b.file));
    skipped
}

/// Bring the database and the folder of `shared` in step. An error means
/// the folder could not be used at all; a file that could not be used is
/// in the summary.
pub fn sync(
    conn: &Connection,
    an: &Analyzer,
    meta: &Meta,
    shared: &Shared,
    patient: bool,
    scope: Scope,
) -> Result<Summary> {
    let dir = shared.dir().map_err(|err| anyhow!(err))?;
    let name = shared.collection.clone().ok_or_else(|| {
        anyhow!(
            "{} turns on files but names no collection; add `collection = <name>`",
            shared.root.join(crate::project::SETTINGS).display()
        )
    })?;
    let root = root_key(&shared.root);
    let collection_file = read_collection_file(&dir)?;
    let only = match &scope {
        Scope::Nodes(ids) => Some(*ids),
        _ => None,
    };
    let first = look(conn, &dir, &root, &name, collection_file.is_some(), only)?;
    if !first.work && matches!(scope, Scope::Changed) {
        return Ok(Summary {
            skipped: known_skipped(&first),
            ..Summary::default()
        });
    }
    if !begin(conn, patient)? {
        return Ok(Summary {
            skipped: known_skipped(&first),
            busy: true,
            ..Summary::default()
        });
    }
    let result = (|| {
        // Look again: another session may have done the work.
        let look = look(conn, &dir, &root, &name, collection_file.is_some(), only)?;
        apply(
            conn,
            an,
            meta,
            &dir,
            &root,
            &name,
            collection_file.as_ref(),
            look,
            &scope,
        )
    })();
    match result {
        Ok(summary) => {
            conn.execute_batch("COMMIT")?;
            Ok(summary)
        }
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(err)
        }
    }
}

/// The id of the collection `name` in this database. The id in the folder
/// is used when the collection is new here, so that it is one collection
/// on every machine.
fn collection_id(
    conn: &Connection,
    an: &Analyzer,
    meta: &Meta,
    name: &str,
    file: Option<&CollectionFile>,
) -> Result<String> {
    if let Some(file) = file {
        if let Some(stored) = store::find_node(conn, &file.id)? {
            if stored.kind == "collection" && stored.deleted_at.is_none() {
                return Ok(stored.id);
            }
        }
    }
    if let Some(id) = store::find_project(conn, name)? {
        return Ok(id);
    }
    let Some(file) = file else {
        return history::ensure_collection(conn, an, meta, name);
    };
    let node: Node = serde_json::from_value(serde_json::json!({
        "id": file.id,
        "type": "collection",
        "title": name,
        "created_at": file.created_at,
        "updated_at": file.created_at,
        "author": meta.author,
    }))?;
    let dump = Dump {
        version: transfer::DUMP_VERSION,
        exported_at: file.created_at.clone(),
        nodes: vec![node],
        edges: Vec::new(),
        synonyms: Vec::new(),
    };
    let mut rec = Recorder::new(conn, "create");
    let rules = Rules {
        allow_secret: true,
        older_keeps_number: false,
    };
    transfer::import_into(conn, an, &dump, &rules, &mut rec)?;
    rec.finish(meta)?;
    store::find_project(conn, name)?.ok_or_else(|| anyhow!("the collection '{name}' was not made"))
}

/// The links that start at a node of the collection and end at another
/// node of it, for each such node.
fn links_inside(conn: &Connection, project_id: &str) -> Result<HashMap<String, Vec<Link>>> {
    let mut stmt = conn.prepare(
        "SELECT e.from_id, e.relation, e.to_id FROM edges e \
         JOIN nodes a ON a.id = e.from_id JOIN nodes b ON b.id = e.to_id \
         WHERE a.project_id = ?1 AND b.project_id = ?1",
    )?;
    let mut links: HashMap<String, Vec<Link>> = HashMap::new();
    let rows = stmt.query_map([project_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?))
    })?;
    for row in rows {
        let (from, relation, to) = row?;
        links.entry(from).or_default().push(Link { relation, to });
    }
    Ok(links)
}

/// The node in the text of a file, or why the text is not one.
fn parse(raw: &str) -> Result<FileNode, String> {
    serde_json::from_str::<FileNode>(raw)
        .map(FileNode::tidy)
        .map_err(|err| {
            if raw.lines().any(|line| line.starts_with("<<<<<<<")) {
                "the file holds the marks of a git conflict; keep one of the two versions"
                    .to_string()
            } else {
                format!("the file is not a node as son writes it ({err})")
            }
        })
}

/// Why the node of a file cannot come in, or `None` when it can.
fn fault_of(file: &FileNode, name: &str, node: &Node) -> Option<String> {
    if id_of(name) != Some(file.id.as_str()) {
        return Some("the id in the file is not the name of the file".to_string());
    }
    if file.kind == "collection" {
        return Some("a node file cannot hold a collection".to_string());
    }
    let mut faults = transfer::node_faults(node);
    if let Some(priority) = &file.priority {
        if !store::PRIORITIES.contains(&priority.as_str()) {
            faults.push(format!("priority '{priority}' is not known"));
        }
    }
    if let Some(number) = &file.number {
        if store::parse_number(number).as_ref() != Some(number) {
            faults.push(format!("number '{number}' is not a todo number"));
        }
    }
    for link in &file.links {
        if !store::RELATIONS.contains(&link.relation.as_str()) || !transfer::is_id(&link.to) {
            faults.push("a link has a relation that is not known, or no id".to_string());
            break;
        }
    }
    (!faults.is_empty()).then(|| faults.join("; "))
}

#[allow(clippy::too_many_arguments)]
fn apply(
    conn: &Connection,
    an: &Analyzer,
    meta: &Meta,
    dir: &Path,
    root: &str,
    name: &str,
    collection_file: Option<&CollectionFile>,
    look: Look,
    scope: &Scope,
) -> Result<Summary> {
    let Look {
        mut files,
        mut records,
        purged: purged_here,
        changed,
        in_step,
        ..
    } = look;
    // The nodes to compare with their files: all of them, or these.
    let mut some: Option<HashSet<String>> = match scope {
        Scope::All => None,
        Scope::Nodes(ids) => Some((*ids).clone()),
        Scope::Changed if in_step => Some(HashSet::new()),
        Scope::Changed => None,
    };
    fn also(some: &mut Option<HashSet<String>>, id: &str) {
        if let Some(some) = some.as_mut() {
            some.insert(id.to_string());
        }
    }
    for name in records.keys().filter(|name| !files.contains_key(*name)) {
        // A file that is gone: written again when its node is still here.
        also(&mut some, id_of(name).unwrap_or_default());
    }
    let mut summary = Summary::default();
    let project_id = collection_id(conn, an, meta, name, collection_file)?;
    let nodes_dir = dir.join(NODES);
    let format = i64::from(FORMAT);

    // Purges first: a purged node must not come in again from its file.
    for id in &purged_here {
        conn.execute(
            "INSERT OR IGNORE INTO shared_purged (project_id, id) VALUES (?1, ?2)",
            params![project_id, id],
        )?;
        let here = store::find_node(conn, id)?
            .is_some_and(|node| node.project_id.as_deref() == Some(project_id.as_str()));
        if here {
            history::purge(conn, id)?;
            summary.purged += 1;
        }
    }
    let purged = purged_known(conn, &project_id)?;
    for id in &purged {
        // Whatever this sync looked at: the file of a purged node goes.
        let file = format!("{id}.json");
        let _ = std::fs::remove_file(nodes_dir.join(&file));
        files.remove(&file);
        records.remove(&file);
        forget_record(conn, root, &file)?;
    }

    // In: read the files that changed.
    let mut arriving: Vec<(FileNode, Node, bool)> = Vec::new();
    for file_name in changed.iter().filter(|name| files.contains_key(*name)) {
        let (size, mtime) = files[file_name];
        let mut record = Record {
            size,
            mtime,
            format,
            full: None,
            content: None,
            updated_at: None,
            canonical: true,
            problem: None,
        };
        let path = nodes_dir.join(file_name);
        let parsed = std::fs::read_to_string(&path)
            .map_err(|err| format!("the file cannot be read ({err})"))
            .and_then(|raw| parse(&raw).map(|file| (raw, file)));
        match parsed {
            Err(problem) => record.problem = Some(problem),
            Ok((raw, file)) => {
                let stored = store::find_node(conn, &file.id)?;
                let node = file.node(&project_id, stored.as_ref());
                let elsewhere = stored
                    .as_ref()
                    .is_some_and(|s| s.project_id.as_deref() != Some(project_id.as_str()));
                let taken = stored
                    .as_ref()
                    .is_none_or(|s| s.updated_at < node.updated_at);
                let secret = || {
                    let known = stored
                        .as_ref()
                        .is_some_and(|s| transfer::secret_in(s).is_some());
                    transfer::secret_in(&node)
                        .filter(|_| taken && !known)
                        .map(|(field, kind)| {
                            format!(
                                "the {field} looks like it holds {kind}; take the secret out of the file"
                            )
                        })
                };
                let same_as_before = records.get(file_name).is_some_and(|before| {
                    before.problem.is_none()
                        && before.canonical
                        && before.full.as_deref() == Some(hash(&raw).as_str())
                        && before.format == format
                });
                if same_as_before {
                    // Read again only because it was young (see `YOUNG`):
                    // the same text as last time.
                    let before = records[file_name].clone();
                    record.full = before.full;
                    record.content = before.content;
                    record.updated_at = before.updated_at;
                } else if elsewhere {
                    record.problem = Some(OTHER_COLLECTION.to_string());
                } else if let Some(problem) = fault_of(&file, file_name, &node).or_else(secret) {
                    record.problem = Some(problem);
                } else {
                    let (full, content) = file.hashes();
                    record.full = Some(full);
                    record.content = Some(content);
                    record.updated_at = Some(file.updated_at.clone());
                    record.canonical = file.text() == raw;
                    also(&mut some, &file.id);
                    arriving.push((file, node, taken));
                }
            }
        }
        save_record(conn, root, file_name, &record)?;
        records.insert(file_name.clone(), record);
    }

    if !arriving.is_empty() {
        let mut rec = Recorder::new(conn, "sync");
        let dump = Dump {
            version: transfer::DUMP_VERSION,
            exported_at: String::new(),
            nodes: arriving.iter().map(|(_, node, _)| node.clone()).collect(),
            edges: Vec::new(),
            synonyms: Vec::new(),
        };
        let rules = Rules {
            // Each file was looked at above.
            allow_secret: true,
            older_keeps_number: true,
        };
        let stats = transfer::import_into(conn, an, &dump, &rules, &mut rec)?;
        summary.added = stats.added;
        summary.updated = stats.updated;
        summary.renumbered = stats.renumbered;
        for moved in &summary.renumbered {
            also(&mut some, &moved.id);
        }
        // The links of a node are part of its file: where the file's
        // content was taken, its links are the links.
        let inside = |id: &str| -> Result<bool> {
            Ok(store::find_node(conn, id)?
                .is_some_and(|node| node.project_id.as_deref() == Some(project_id.as_str())))
        };
        let mut have = links_inside(conn, &project_id)?;
        for (file, _, taken) in &arriving {
            if !taken {
                continue;
            }
            let mut wanted: Vec<&Link> = Vec::new();
            for link in &file.links {
                if link.to != file.id && inside(&link.to)? {
                    wanted.push(link);
                }
            }
            let stored = have.remove(&file.id).unwrap_or_default();
            for gone in stored.iter().filter(|link| !wanted.contains(link)) {
                rec.unlink(&file.id, &gone.relation, &gone.to)?;
            }
            for new in wanted.iter().filter(|link| !stored.contains(link)) {
                rec.link(&file.id, &new.relation, &new.to)?;
            }
        }
        rec.finish(meta)?;
    }

    // Out: write each node whose file is missing or says something else.
    let links = links_inside(conn, &project_id)?;
    let nodes: Vec<Node> = match &some {
        None => store::list_nodes(conn, Some(&project_id), None, true)?,
        Some(ids) => {
            let mut nodes = Vec::new();
            for id in ids {
                nodes.extend(store::find_node(conn, id)?);
            }
            nodes
        }
    }
    .into_iter()
    .filter(|node| {
        node.kind != "collection" && node.project_id.as_deref() == Some(project_id.as_str())
    })
    .collect();
    for mut node in nodes {
        let file_name = format!("{}.json", node.id);
        let record = records.get(&file_name);
        if record.is_some_and(|r| r.problem.is_some()) && files.contains_key(&file_name) {
            // A file that could not be used is left for a person to repair.
            continue;
        }
        let of = |node: &Node| FileNode::of(node, links.get(&node.id).cloned().unwrap_or_default());
        let mut file = of(&node);
        let (mut full, mut content) = file.hashes();
        if let (Some(record), true) = (record, files.contains_key(&file_name)) {
            if record.full.as_deref() == Some(full.as_str()) && record.canonical {
                continue;
            }
            let tie = record.content.as_deref() != Some(content.as_str())
                && record
                    .updated_at
                    .as_deref()
                    .is_some_and(|at| node.updated_at.as_str() <= at);
            if tie {
                // See "Ties" at the top of this file.
                conn.execute(
                    &format!(
                        "UPDATE nodes SET rev = rev + 1, updated_at = max({NOW}, \
                         coalesce(strftime('%Y-%m-%dT%H:%M:%fZ', ?2, '+0.001 seconds'), '')) \
                         WHERE id = ?1"
                    ),
                    params![node.id, record.updated_at],
                )?;
                node = store::get_node(conn, &node.id)?;
                file = of(&node);
                (full, content) = file.hashes();
            }
        }
        let path = nodes_dir.join(&file_name);
        write_file(&path, &file.text())?;
        let (size, mtime) = stat(&path).context("the file that was written is gone")?;
        let record = Record {
            size,
            mtime,
            format,
            full: Some(full),
            content: Some(content),
            updated_at: Some(node.updated_at.clone()),
            canonical: true,
            problem: None,
        };
        save_record(conn, root, &file_name, &record)?;
        records.insert(file_name.clone(), record);
        files.insert(file_name, (size, mtime));
        summary.written += 1;
    }
    // A record of a file that is gone, for a node that is no longer in
    // the collection, has nothing left to say.
    for file_name in records.keys().filter(|name| !files.contains_key(*name)) {
        forget_record(conn, root, file_name)?;
    }

    for id in purged.iter().filter(|id| !purged_here.contains(*id)) {
        write_file(&dir.join(PURGED).join(id), "")?;
    }
    if collection_file.is_none() {
        let created_at = store::get_node(conn, &project_id)?.created_at;
        let file = CollectionFile {
            version: FORMAT,
            id: project_id.clone(),
            created_at,
        };
        let mut text = serde_json::to_string_pretty(&file)?;
        text.push('\n');
        write_file(&dir.join(COLLECTION_FILE), &text)?;
    }
    if !dir.join(ATTRIBUTES).exists() {
        write_file(&dir.join(ATTRIBUTES), ATTRIBUTES_TEXT)?;
    }
    conn.execute(
        &format!(
            "INSERT INTO shared_folders (root, project_id, seen_at, state) VALUES (?1, ?2, {NOW}, ?3) \
             ON CONFLICT(root) DO UPDATE SET project_id = excluded.project_id, \
                seen_at = excluded.seen_at, state = excluded.state"
        ),
        params![root, project_id, state(conn, &project_id)?],
    )?;

    summary.skipped = records
        .iter()
        .filter(|(name, _)| files.contains_key(*name))
        .filter_map(|(name, record)| {
            Some(Skipped {
                file: name.clone(),
                reason: record.problem.clone()?,
            })
        })
        .collect();
    summary.skipped.sort_by(|a, b| a.file.cmp(&b.file));
    Ok(summary)
}

impl Skipped {
    /// False for a file that is left alone on purpose and needs no repair.
    pub fn needs_repair(&self) -> bool {
        self.reason != OTHER_COLLECTION
    }
}

/// Forget a folder: it is no longer shared, or it is gone.
pub fn forget(conn: &Connection, root: &Path) -> Result<()> {
    let root = root_key(root);
    forget_key(conn, &root)
}

fn forget_key(conn: &Connection, root: &str) -> Result<()> {
    conn.execute("DELETE FROM shared_files WHERE root = ?1", [root])?;
    conn.execute("DELETE FROM shared_folders WHERE root = ?1", [root])?;
    Ok(())
}

/// The highest number of an operation in the history. With [`changed_since`]
/// it tells which nodes a command changed.
pub fn last_op(conn: &Connection) -> i64 {
    conn.query_row("SELECT coalesce(max(id), 0) FROM ops", [], |r| r.get(0))
        .unwrap_or(0)
}

/// The nodes that operations after `op` changed, and the nodes where a
/// link that such an operation added or removed starts.
pub fn changed_since(conn: &Connection, op: i64) -> HashSet<String> {
    conn.prepare(
        "SELECT node_id FROM revisions WHERE op_id > ?1 \
         UNION SELECT from_id FROM edge_changes WHERE op_id > ?1",
    )
    .and_then(|mut stmt| {
        stmt.query_map([op], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()
    })
    .unwrap_or_default()
}

/// Sync every folder that this database knows, wherever the command runs:
/// a write from another folder must reach the files. `changed` holds the
/// nodes that the command changed. A folder that is gone, or whose `.son`
/// no longer turns files on, is forgotten.
pub fn sync_known(
    conn: &Connection,
    an: &Analyzer,
    meta: &Meta,
    changed: &HashSet<String>,
) -> Vec<(Shared, Result<Summary>)> {
    let roots: Vec<String> = conn
        .prepare("SELECT root FROM shared_folders ORDER BY root")
        .and_then(|mut stmt| {
            stmt.query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()
        })
        .unwrap_or_default();
    let mut done = Vec::new();
    for root in roots {
        match crate::project::shared_in(Path::new(&root)) {
            Some(shared) => {
                let result = sync(conn, an, meta, &shared, true, Scope::Nodes(changed));
                done.push((shared, result));
            }
            None => {
                let _ = forget_key(conn, &root);
            }
        }
    }
    done
}

/// What `son files` shows.
#[derive(Debug, Serialize)]
pub struct Status {
    pub on: bool,
    /// The folder that holds `.son`.
    pub root: Option<String>,
    /// The folder with the files.
    pub folder: Option<String>,
    pub collection: Option<String>,
    /// The number of node files.
    pub nodes: usize,
    pub skipped: Vec<Skipped>,
}

pub fn status(conn: &Connection, shared: Option<&Shared>) -> Result<Status> {
    let Some(shared) = shared else {
        return Ok(Status {
            on: false,
            root: None,
            folder: None,
            collection: None,
            nodes: 0,
            skipped: Vec::new(),
        });
    };
    let dir: PathBuf = shared.dir().map_err(|err| anyhow!(err))?;
    let files = node_files(&dir);
    let records = records(conn, &root_key(&shared.root))?;
    let mut skipped: Vec<Skipped> = records
        .iter()
        .filter(|(name, _)| files.contains_key(*name))
        .filter_map(|(name, record)| {
            Some(Skipped {
                file: name.clone(),
                reason: record.problem.clone()?,
            })
        })
        .collect();
    skipped.sort_by(|a, b| a.file.cmp(&b.file));
    Ok(Status {
        on: true,
        root: Some(shared.root.display().to_string()),
        folder: Some(dir.display().to_string()),
        collection: shared.collection.clone(),
        nodes: files.len(),
        skipped,
    })
}

/// What `son doctor` reports about a shared folder. Reads the files only:
/// no database is opened and nothing is changed.
pub struct Inspection {
    pub folder: PathBuf,
    pub nodes: usize,
    pub skipped: Vec<Skipped>,
}

pub fn inspect(shared: &Shared) -> Result<Inspection> {
    let dir = shared.dir().map_err(|err| anyhow!(err))?;
    if shared.collection.is_none() {
        bail!(
            "{} turns on files but names no collection; add `collection = <name>`",
            shared.root.join(crate::project::SETTINGS).display()
        );
    }
    let collection = read_collection_file(&dir)?;
    // Any id of the right form: the checks only need the node to be in
    // a collection.
    let project_id = collection.map_or_else(|| "0".repeat(26), |file| file.id);
    let files = node_files(&dir);
    let mut skipped = Vec::new();
    for name in files.keys() {
        let reason = match std::fs::read_to_string(dir.join(NODES).join(name)) {
            Err(err) => Some(format!("the file cannot be read ({err})")),
            Ok(raw) => match parse(&raw) {
                Err(reason) => Some(reason),
                Ok(file) => fault_of(&file, name, &file.node(&project_id, None)),
            },
        };
        if let Some(reason) = reason {
            skipped.push(Skipped {
                file: name.clone(),
                reason,
            });
        }
    }
    skipped.sort_by(|a, b| a.file.cmp(&b.file));
    Ok(Inspection {
        folder: dir,
        nodes: files.len(),
        skipped,
    })
}

/// The ids of the nodes of `ids` that are in a collection kept as files.
pub fn shared_ids(conn: &Connection, ids: &[&str]) -> Result<HashSet<String>> {
    let mut found = HashSet::new();
    for id in ids {
        let shared: bool = conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM nodes n JOIN shared_folders f ON f.project_id = n.project_id \
                            WHERE n.id = ?1)",
            [id],
            |r| r.get(0),
        )?;
        if shared {
            found.insert((*id).to_string());
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_node_is_always_the_same_text() {
        let node: Node = serde_json::from_value(serde_json::json!({
            "id": "01JD3K0000000000000050F3H6",
            "type": "pattern",
            "title": "Terraform AWS layout",
            "body": "One module per service.",
            "collection_id": "01JD3K00000000000000000000",
            "tags": ["aws", "terraform"],
            "created_at": "2026-11-19T15:02:11.000Z",
            "updated_at": "2026-11-19T15:02:11.000Z",
        }))
        .unwrap();
        let link = |to: &str| Link {
            relation: "relates_to".to_string(),
            to: to.to_string(),
        };
        let a = FileNode::of(&node, vec![link("B"), link("A"), link("B")]);
        let b = FileNode::of(&node, vec![link("A"), link("B")]);
        assert_eq!(a.text(), b.text());
        assert!(a.text().ends_with("}\n"));
        // What is empty is left out, and the collection is not in the file.
        assert!(!a.text().contains("status"));
        assert!(!a.text().contains("collection"));
        let back: FileNode = serde_json::from_str(&a.text()).unwrap();
        assert_eq!(back.tidy(), a);
    }

    #[test]
    fn a_mark_changes_the_whole_hash_and_not_the_content_hash() {
        let node: Node = serde_json::from_value(serde_json::json!({
            "id": "01JD3K0000000000000050F3H6",
            "type": "fact",
            "title": "A fact",
            "created_at": "2026-11-19T15:02:11.000Z",
            "updated_at": "2026-11-19T15:02:11.000Z",
        }))
        .unwrap();
        let plain = FileNode::of(&node, Vec::new());
        let mut marked = plain.clone();
        marked.stale_at = Some("2026-11-20T10:00:00.000Z".to_string());
        marked.freshness_at = marked.stale_at.clone();
        assert_ne!(plain.hashes().0, marked.hashes().0);
        assert_eq!(plain.hashes().1, marked.hashes().1);
        let mut edited = plain.clone();
        edited.title = "Another fact".to_string();
        assert_ne!(plain.hashes().1, edited.hashes().1);
    }
}
