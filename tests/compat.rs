//! What stays the same in every 1.x release. See "What stays the same in
//! version 1" in the README.
//!
//! These tests pin the public surface of `son`: the commands and flags,
//! the error codes, the exit statuses, the environment variables, the
//! JSON fields of each command, and the databases and exports that old
//! releases wrote.
//!
//! When a test here fails, read this first:
//!
//! - You ADDED a command, a flag, a code, a variable or a JSON field: add
//!   the line to the file in `tests/compat/` that the failure names. Run
//!   `SON_COMPAT_WRITE=1 cargo test --test compat` to write the files
//!   again, and look at the diff: it must only add lines.
//! - A line is GONE or CHANGED: that breaks programs that use `son`. Do
//!   not change the file. Put the name back; for a rename, keep the old
//!   name as a hidden alias.

use clap::CommandFactory;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn compat_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/compat")
}

/// Compare `actual` with the stored file `name`. With `SON_COMPAT_WRITE=1`
/// the file is written and the test passes.
fn pinned(name: &str, actual: &str) {
    let path = compat_dir().join(name);
    if std::env::var_os("SON_COMPAT_WRITE").is_some() {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    // Line endings do not count: a Windows checkout may change them.
    let stored = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    if stored == actual {
        return;
    }
    let (old, new): (BTreeSet<&str>, BTreeSet<&str>) =
        (stored.lines().collect(), actual.lines().collect());
    let gone: Vec<&&str> = old.difference(&new).collect();
    let added: Vec<&&str> = new.difference(&old).collect();
    panic!(
        "the public surface in tests/compat/{name} changed.\n\
         GONE OR CHANGED (this breaks users; put it back, do not edit the file):\n{}\n\
         ADDED (fine; add these lines to the file, or run SON_COMPAT_WRITE=1 cargo test --test compat):\n{}\n",
        if gone.is_empty() {
            "  nothing".to_string()
        } else {
            gone.iter().map(|l| format!("  {l}")).collect::<Vec<_>>().join("\n")
        },
        if added.is_empty() {
            "  nothing".to_string()
        } else {
            added.iter().map(|l| format!("  {l}")).collect::<Vec<_>>().join("\n")
        },
    );
}

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("son-compat-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["home", "shop"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        Self { root }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_son"));
        command
            .args(args)
            .current_dir(self.root.join("shop"))
            .env("SON_DB", self.root.join("son.db"))
            .env("HOME", self.root.join("home"))
            .env_remove("USERPROFILE")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("CODEX_HOME")
            .env_remove("SON_COLLECTION")
            .env_remove("SON_NO_LOG")
            .env_remove("SON_SOURCE")
            .env_remove("SON_AUTHOR");
        command
    }

    /// Run, and give the exit status with stdout and stderr.
    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let output = self.command(args).output().unwrap();
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap(),
        )
    }

    /// Run with `--json`, require success, and give the answer.
    fn json(&self, args: &[&str]) -> Value {
        let mut full = vec!["--json"];
        full.extend(args);
        let (status, stdout, stderr) = self.run(&full);
        assert_eq!(status, 0, "son {args:?} failed: {stderr}");
        serde_json::from_str(&stdout).unwrap_or_else(|err| panic!("son {args:?}: {err}: {stdout}"))
    }

    fn id(&self, args: &[&str]) -> String {
        self.json(args)["short_id"].as_str().unwrap().to_string()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// One line for each command: its name, its flags and its arguments.
fn surface(command: &clap::Command, path: &str, lines: &mut Vec<String>) {
    let mut parts: Vec<String> = Vec::new();
    for arg in command.get_arguments() {
        let id = arg.get_id().as_str();
        if id == "help" || id == "version" {
            continue;
        }
        match arg.get_long() {
            Some(long) => {
                let mut names = vec![format!("--{long}")];
                if let Some(short) = arg.get_short() {
                    names.push(format!("-{short}"));
                }
                // Hidden names that still work.
                if let Some(aliases) = arg.get_all_aliases() {
                    names.extend(aliases.iter().map(|alias| format!("--{alias}")));
                }
                parts.push(names.join("|"));
            }
            None => parts.push(format!("<{id}>")),
        }
    }
    parts.sort();
    lines.push(format!("{path}: {}", parts.join(" ")));
    for sub in command.get_subcommands() {
        let mut names: Vec<&str> = vec![sub.get_name()];
        names.extend(sub.get_all_aliases());
        for name in names {
            if name == "help" {
                continue;
            }
            surface(sub, &format!("{path} {name}"), lines);
        }
    }
}

#[test]
fn commands_and_flags_stay() {
    let mut lines = Vec::new();
    surface(&son_of_w::cli::Cli::command(), "son", &mut lines);
    lines.sort();
    pinned("commands.txt", &format!("{}\n", lines.join("\n")));
}

#[test]
fn error_codes_stay() {
    let mut codes: Vec<&str> = son_of_w::error::CODES.to_vec();
    codes.sort();
    pinned("error-codes.txt", &format!("{}\n", codes.join("\n")));
    // Each one is in the table of the reference.
    let readme =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/reference.md"))
            .unwrap();
    for code in codes {
        assert!(
            readme.contains(&format!("| `{code}` |")),
            "the error code {code} is not in the table of docs/reference.md"
        );
    }
}

#[test]
fn environment_variables_stay() {
    // Every `SON_...` name in the source is a variable that son reads.
    let mut names: BTreeSet<String> = BTreeSet::new();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for entry in std::fs::read_dir(source).unwrap().flatten() {
        if entry.path().extension().is_none_or(|ending| ending != "rs") {
            continue;
        }
        let text = std::fs::read_to_string(entry.path()).unwrap();
        for part in text.split('"').skip(1).step_by(2) {
            let is_name = part.starts_with("SON_")
                && part.len() > 4
                && part.bytes().all(|b| b.is_ascii_uppercase() || b == b'_');
            if is_name {
                names.insert(part.to_string());
            }
        }
    }
    let names: Vec<String> = names.into_iter().collect();
    pinned("environment.txt", &format!("{}\n", names.join("\n")));
    let readme =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/reference.md"))
            .unwrap();
    for name in names {
        assert!(
            readme.contains(&format!("`{name}")),
            "the variable {name} is not in docs/reference.md"
        );
    }
}

#[test]
fn exit_statuses_stay() {
    let sb = Sandbox::new("exit");
    // Success is 0.
    assert_eq!(sb.run(&["remember", "note", "A note"]).0, 0);
    assert_eq!(sb.run(&["list"]).0, 0);
    assert_eq!(sb.run(&["--help"]).0, 0);
    assert_eq!(sb.run(&["--version"]).0, 0);
    // A mistake in the arguments is 2, and nothing ran.
    for args in [
        vec!["no-such-command"],
        vec!["recall"],
        vec!["remember", "sandwich", "x"],
        vec!["list", "--no-such-flag"],
    ] {
        assert_eq!(sb.run(&args).0, 2, "{args:?}");
    }
    // Every other failure is 1.
    for args in [
        vec!["show", "qqqqqqqq"],
        vec!["remember", "note", "  "],
        vec!["list", "--collection", "nope"],
        vec!["import", "/no/such/file.json"],
    ] {
        assert_eq!(sb.run(&args).0, 1, "{args:?}");
    }
    // With --json, a failure is one object on stderr with these fields,
    // and nothing on stdout.
    let (status, stdout, stderr) = sb.run(&["--json", "show", "qqqqqqqq"]);
    assert_eq!(status, 1);
    assert!(stdout.is_empty());
    let error: Value = serde_json::from_str(&stderr).unwrap();
    let fields: Vec<&String> = error["error"].as_object().unwrap().keys().collect();
    assert_eq!(fields, ["code", "message", "retry"]);
    assert_eq!(error["error"]["code"], "unknown_id");
    let (status, _, stderr) = sb.run(&["--json", "no-such-command"]);
    assert_eq!(status, 2);
    let error: Value = serde_json::from_str(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "invalid_argument");
}

/// The names of the fields of a JSON answer, with the fields of the
/// objects and lists inside it: `name: a b c`, `name.b[]: x y`.
fn fields(name: &str, value: &Value, lines: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            let keys: Vec<&str> = map.keys().map(String::as_str).collect();
            lines.insert(format!("{name}: {}", keys.join(" ")));
            for (key, inner) in map {
                // A node inside an answer has the fields of a node; they
                // are listed once, under "node".
                if is_node(inner) {
                    fields("node", inner, lines);
                } else {
                    fields(&format!("{name}.{key}"), inner, lines);
                }
            }
        }
        Value::Array(list) => {
            for inner in list {
                if is_node(inner) {
                    fields("node", inner, lines);
                } else {
                    fields(&format!("{name}[]"), inner, lines);
                }
            }
        }
        _ => {}
    }
}

fn is_node(value: &Value) -> bool {
    value.get("short_id").is_some()
        && value.get("created_at").is_some()
        && value.get("rev").is_some()
}

#[test]
fn json_fields_stay() {
    let sb = Sandbox::new("json");
    let home = sb.root.join("home");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::write(home.join(".claude/CLAUDE.md"), "# Mine\n").unwrap();
    let mut lines: BTreeSet<String> = BTreeSet::new();
    let mut see = |name: &str, value: Value| fields(name, &value, &mut lines);

    see(
        "init",
        sb.json(&["init", "-a", "claude-code", "-g", "--todo-skill"]),
    );
    see("init --db-only", sb.json(&["init", "--db-only"]));
    see("init --list-agents", sb.json(&["init", "--list-agents"]));
    let fact = sb.json(&[
        "remember",
        "fact",
        "API port is 8443",
        "--body",
        "Set in config/server.toml.",
        "--tag",
        "api,ports",
        "--evidence",
        "observation",
        "--ref",
        "file:config/server.toml",
    ]);
    let fact_id = fact["short_id"].as_str().unwrap().to_string();
    see("remember", fact);
    let other = sb.id(&["remember", "fact", "The API port is 8443 now"]);
    let decision = sb.id(&["remember", "decision", "Use Postgres for orders"]);
    let newer = sb.id(&["remember", "decision", "Use Postgres 17 for orders"]);
    sb.json(&["link", &newer, "supersedes", &decision]);
    see(
        "remember --check",
        sb.json(&["remember", "fact", "API port is 8443", "--check"]),
    );
    see(
        "todo add",
        sb.json(&["todo", "add", "Rotate the staging key", "--priority", "p1"]),
    );
    sb.json(&["todo", "add", "Order a new key", "--parent", "1"]);
    see("todo list", sb.json(&["todo", "list"]));
    see("todo backlog", sb.json(&["todo", "backlog"]));
    see(
        "todo done",
        sb.json(&["todo", "done", "1.1", "--note", "ordered"]),
    );
    see("todo reopen", sb.json(&["todo", "reopen", "1.1"]));
    see(
        "edit",
        sb.json(&[
            "edit",
            &fact_id,
            "--body",
            "Set in config/server.toml, line 3.",
        ]),
    );
    see(
        "edit (moved)",
        sb.json(&["edit", "1.1", "--collection", "other"]),
    );
    see(
        "verify",
        sb.json(&["verify", &fact_id, "--checked", "read the file"]),
    );
    see(
        "stale",
        sb.json(&["stale", &other, "--reason", "the port moved"]),
    );
    see(
        "link",
        sb.json(&["link", &fact_id, "relates_to", &decision]),
    );
    see("show", sb.json(&["show", &fact_id]));
    see("show (superseded)", sb.json(&["show", &decision]));
    see("recall", sb.json(&["recall", "api", "port", "--explain"]));
    see(
        "recall --max-chars",
        sb.json(&["recall", "api", "port", "--max-chars", "700"]),
    );
    see("context", sb.json(&["context"]));
    see(
        "context --max-chars",
        sb.json(&["context", "--max-chars", "600"]),
    );
    see(
        "context (empty)",
        sb.json(&["context", "--collection", "nothing-here"]),
    );
    see("list", sb.json(&["list"]));
    see("log", sb.json(&["log"]));
    see("stats", sb.json(&["stats"]));
    see("collections", sb.json(&["collections"]));
    see("tags", sb.json(&["tags"]));
    see("review", sb.json(&["review", "--fact-days", "0"]));
    let history = sb.json(&["history", &fact_id]);
    let op = history.to_string();
    see("history", history);
    // The number of a change of this node, as the answer gives it.
    let op: String = op
        .split("\"op\":")
        .nth(1)
        .map(|rest| rest.chars().take_while(char::is_ascii_digit).collect())
        .unwrap_or_default();
    see("history --op", sb.json(&["history", &fact_id, "--op", &op]));
    see(
        "merge",
        sb.json(&[
            "merge",
            &other,
            &fact_id,
            "--body",
            "Set in config/server.toml.",
        ]),
    );
    see("undo", sb.json(&["undo", &fact_id]));
    see("drop", sb.json(&["drop", &newer]));
    see("restore", sb.json(&["restore", &newer]));
    see(
        "synonym add",
        sb.json(&["synonym", "add", "pgsql", "postgres"]),
    );
    see("synonym list", sb.json(&["synonym", "list"]));
    see(
        "unlink",
        sb.json(&["unlink", &fact_id, "relates_to", &decision]),
    );
    see("doctor", sb.json(&["doctor"]));
    see("seed", sb.json(&["seed"]));
    let export = sb.json(&["export"]);
    let file = sb.root.join("export.json");
    std::fs::write(&file, export.to_string()).unwrap();
    // The export is a file format of its own; its nodes hold two fields
    // more than a node that a command prints.
    let mut export_lines: BTreeSet<String> = BTreeSet::new();
    fields("export", &export, &mut export_lines);
    let backup = sb.root.join("backup.db");
    see("backup", sb.json(&["backup", backup.to_str().unwrap()]));
    see("import", sb.json(&["import", file.to_str().unwrap()]));
    see(
        "import (backup)",
        sb.json(&["import", backup.to_str().unwrap(), "--replace"]),
    );
    see("reindex", sb.json(&["reindex"]));
    see("uninstall", sb.json(&["uninstall", "--dry-run"]));
    // Last: from here on the folder is kept as files, and every command
    // in it syncs.
    see("files", sb.json(&["files"]));
    see("files on", sb.json(&["files", "on"]));

    let all: Vec<String> = lines.into_iter().collect();
    pinned("json-fields.txt", &format!("{}\n", all.join("\n")));
    let all: Vec<String> = export_lines.into_iter().collect();
    pinned("export-fields.txt", &format!("{}\n", all.join("\n")));
}

/// Open a database that an old release wrote, with this binary.
fn old_database(version: &str) {
    let sb = Sandbox::new(&format!("db-{version}"));
    let fixture = compat_dir().join(format!("son-{version}.db"));
    std::fs::copy(&fixture, sb.root.join("son.db")).unwrap();
    let before = std::fs::read(&fixture).unwrap();

    // `son init` is the upgrade step: it says what it did and where the
    // copy of the old database is.
    let state = sb.json(&["init", "--db-only"])["database_state"].clone();
    assert_eq!(state["state"], "upgraded", "{version}: {state}");
    let copy = PathBuf::from(state["copy"].as_str().expect("a copy was made"));
    // The copy holds the old schema and the old rows.
    let look = |path: &Path| -> (i64, Vec<String>) {
        let conn = rusqlite::Connection::open(path).unwrap();
        let version = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        let mut titles: Vec<String> = conn
            .prepare("SELECT title FROM nodes ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        titles.sort();
        (version, titles)
    };
    assert_eq!(
        look(&copy),
        look(&fixture),
        "{version}: the copy is the old database"
    );
    let _ = before;

    // Everything that was stored is there and works.
    let list = sb.json(&["list", "--limit", "50"]);
    let titles: BTreeSet<&str> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["title"].as_str().unwrap())
        .collect();
    for title in [
        "Use Postgres for orders",
        "Terraform layout for AWS",
        "API port is 8443",
        "Rotate the staging key",
        "Write the README",
    ] {
        assert!(
            titles.contains(title),
            "{version}: '{title}' is not in {titles:?}"
        );
    }
    assert!(!titles.contains("A note that was dropped"));
    let dropped = sb.json(&["list", "--dropped", "--limit", "50"]);
    assert!(dropped.to_string().contains("A note that was dropped"));

    let found = sb.json(&["recall", "pgsql"]);
    assert_eq!(
        found["hits"][0]["title"], "Use Postgres for orders",
        "{version}: the synonym works"
    );
    let found = sb.json(&["recall", "terraform", "--tag", "aws"]);
    assert_eq!(found["hits"][0]["title"], "Terraform layout for AWS");

    let fact = &sb.json(&["recall", "api", "port"])["hits"][0];
    assert_eq!(fact["body"], "Set in config/server.toml, line 3.");
    let fact_id = fact["short_id"].as_str().unwrap().to_string();
    let shown = sb.json(&["show", &fact_id]);
    assert_eq!(
        shown["links"][0]["node"]["title"], "Use Postgres for orders",
        "{version}"
    );
    assert_eq!(shown["collection"], "shop");

    let todos = sb.json(&["todo", "list", "--done"]);
    let status: Vec<(&str, &str)> = todos
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["title"].as_str().unwrap(), t["status"].as_str().unwrap()))
        .collect();
    assert!(
        status.contains(&("Rotate the staging key", "open")),
        "{version}: {status:?}"
    );
    assert!(
        status.contains(&("Write the README", "done")),
        "{version}: {status:?}"
    );

    // It can be written to, and what doctor checks is whole.
    sb.json(&["remember", "note", "Written by the new release"]);
    sb.json(&["todo", "add", "A new todo"]);
    sb.json(&["reindex"]);
    let report = sb.json(&["doctor"]);
    for check in report["checks"].as_array().unwrap() {
        let name = check["name"].as_str().unwrap();
        if [
            "database",
            "schema",
            "integrity",
            "references",
            "search index",
            "records",
        ]
        .contains(&name)
        {
            assert_eq!(check["status"], "healthy", "{version}: {check}");
        }
    }
}

/// Import an export that an old release wrote, into a new database.
fn old_export(version: &str) {
    let sb = Sandbox::new(&format!("export-{version}"));
    let fixture = compat_dir().join(format!("export-{version}.json"));
    let stats = sb.json(&["import", fixture.to_str().unwrap()]);
    assert_eq!(stats["added"], 7, "{version}: {stats}");
    assert_eq!(stats["edges"], 1, "{version}: {stats}");
    let found = sb.json(&["recall", "pgsql"]);
    assert_eq!(found["hits"][0]["title"], "Use Postgres for orders");
    assert_eq!(found["hits"][0]["collection"], "shop");
    let todos = sb.json(&["todo", "list", "--collection", "shop", "--done"]);
    assert_eq!(todos.as_array().unwrap().len(), 2);
    // A second import of the same file changes nothing.
    let again = sb.json(&["import", fixture.to_str().unwrap()]);
    assert_eq!(
        (again["added"].clone(), again["updated"].clone()),
        (0.into(), 0.into())
    );
}

#[test]
fn a_database_from_0_1_1_opens() {
    old_database("0.1.1");
}

#[test]
fn a_database_from_0_1_9_opens() {
    old_database("0.1.9");
}

#[test]
fn a_database_from_0_1_15_opens() {
    old_database("0.1.15");
}

#[test]
fn exports_from_old_releases_import() {
    for version in ["0.1.1", "0.1.9", "0.1.15"] {
        old_export(version);
    }
}
