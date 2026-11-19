//! A collection kept as files in its project folder (`son files`).
//!
//! Two people, each with a home, a database and a clone of one project.
//! Files are copied between the clones as a `git pull` would bring them.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Team {
    root: PathBuf,
}

struct Out {
    ok: bool,
    stdout: String,
    stderr: String,
}

const DATA: &str = ".son-data";

impl Team {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("son-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for person in ["ann", "bob"] {
            std::fs::create_dir_all(root.join(person).join("home")).unwrap();
            std::fs::create_dir_all(root.join(person).join("proj")).unwrap();
            std::fs::create_dir_all(root.join(person).join("other")).unwrap();
        }
        Self { root }
    }

    /// The clone of the project that `person` has.
    fn proj(&self, person: &str) -> PathBuf {
        self.root.join(person).join("proj")
    }

    fn nodes(&self, person: &str) -> PathBuf {
        self.proj(person).join(DATA).join("nodes")
    }

    fn run_in(&self, person: &str, dir: &str, args: &[&str]) -> Out {
        let home = self.root.join(person);
        let output = Command::new(env!("CARGO_BIN_EXE_son"))
            .args(args)
            .current_dir(home.join(dir))
            .env("SON_DB", home.join("son.db"))
            .env("HOME", home.join("home"))
            .env("SON_AUTHOR", person)
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("CODEX_HOME")
            .env_remove("SON_COLLECTION")
            .env_remove("SON_NO_LOG")
            .env_remove("SON_SOURCE")
            .output()
            .unwrap();
        Out {
            ok: output.status.success(),
            stdout: String::from_utf8(output.stdout).unwrap(),
            stderr: String::from_utf8(output.stderr).unwrap(),
        }
    }

    /// Run in the project folder and require success.
    fn son(&self, person: &str, args: &[&str]) -> Out {
        let out = self.run_in(person, "proj", args);
        assert!(out.ok, "{person}: son {args:?} failed: {}", out.stderr);
        out
    }

    /// Ann turns files on; Bob gets the `.son` file, as from a clone.
    fn start(&self) {
        std::fs::write(self.proj("ann").join(".son"), "collection = shop\n").unwrap();
        self.son("ann", &["files", "on"]);
        self.pull("bob", "ann");
    }

    /// `to` gets every shared file that `from` has, as a pull would bring
    /// it. Files that only `to` has stay.
    fn pull(&self, to: &str, from: &str) {
        std::fs::copy(self.proj(from).join(".son"), self.proj(to).join(".son")).unwrap();
        copy_dir(&self.proj(from).join(DATA), &self.proj(to).join(DATA));
    }

    /// The node file of `person` whose title is `title`.
    fn file(&self, person: &str, title: &str) -> PathBuf {
        let mut found: Vec<PathBuf> = std::fs::read_dir(self.nodes(person))
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| read_json(path).is_some_and(|node| node["title"] == title))
            .collect();
        assert_eq!(found.len(), 1, "one file with the title '{title}'");
        found.remove(0)
    }

    fn node(&self, person: &str, title: &str) -> Value {
        read_json(&self.file(person, title)).unwrap()
    }

    /// The names of the node files of `person`, in order.
    fn names(&self, person: &str) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.nodes(person))
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
                    .filter(|name| name.ends_with(".json"))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    /// Everything `person` has in the collection, as `son export` gives
    /// it, without what is of one machine.
    fn content(&self, person: &str) -> Vec<Value> {
        let dump: Value = serde_json::from_str(
            &self
                .son(person, &["export", "--json", "--collection", "shop"])
                .stdout,
        )
        .unwrap();
        let mut nodes: Vec<Value> = dump["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|node| node["type"] != "collection")
            .map(|node| {
                let mut node = node.clone();
                for field in ["rev", "source", "collection_id", "collection"] {
                    node.as_object_mut().unwrap().remove(field);
                }
                node
            })
            .collect();
        nodes.sort_by_key(|node| node["id"].as_str().unwrap().to_string());
        let mut edges: Vec<String> = dump["edges"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| format!("{} {} {}", e["from_id"], e["relation"], e["to_id"]))
            .collect();
        edges.sort();
        nodes.push(Value::from(edges));
        nodes
    }
}

impl Drop for Team {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// The files under `dir` with their bytes, in order.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let mut folders = vec![dir.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(folder).unwrap().flatten() {
            if entry.path().is_dir() {
                folders.push(entry.path());
            } else {
                files.push((entry.path(), std::fs::read(entry.path()).unwrap()));
            }
        }
    }
    files.sort();
    files
}

#[test]
fn turning_it_on_writes_the_setting_and_one_file_for_each_node() {
    let team = Team::new("on");
    std::fs::write(
        team.proj("ann").join(".son"),
        "# our settings\nfuture_key = 1\ncollection = shop\n",
    )
    .unwrap();
    team.son(
        "ann",
        &[
            "remember",
            "fact",
            "Money is cents",
            "--body",
            "No floats.",
            "--tag",
            "billing",
        ],
    );
    team.son(
        "ann",
        &["todo", "add", "Rotate the key", "--priority", "p1"],
    );
    // Off: nothing is written.
    assert!(!team.proj("ann").join(DATA).exists());
    let off = team.son("ann", &["files"]);
    assert!(off.stdout.starts_with("files: off"), "{}", off.stdout);

    let on = team.son("ann", &["files", "on"]);
    assert!(on.stdout.contains("files: on for collection 'shop'"));
    assert!(on.stdout.contains("wrote 2 file(s)"), "{}", on.stdout);
    // The setting is added; every other line stays.
    assert_eq!(
        std::fs::read_to_string(team.proj("ann").join(".son")).unwrap(),
        "# our settings\nfuture_key = 1\ncollection = shop\nfiles = .son-data\n"
    );
    assert_eq!(team.names("ann").len(), 2);
    let fact = team.node("ann", "Money is cents");
    assert_eq!(fact["type"], "fact");
    assert_eq!(fact["body"], "No floats.");
    assert_eq!(fact["tags"][0], "billing");
    assert_eq!(fact["author"], "ann");
    // What is of one machine or of the folder is not in the file.
    for absent in ["rev", "source", "collection", "collection_id", "origin"] {
        assert!(fact.get(absent).is_none(), "{absent} is in the file");
    }
    let todo = team.node("ann", "Rotate the key");
    assert_eq!(todo["number"], "1");
    assert_eq!(todo["priority"], "p1");
    let collection = read_json(&team.proj("ann").join(DATA).join("collection.json")).unwrap();
    assert_eq!(collection["version"], 1);

    // A write updates the files at once.
    team.son("ann", &["remember", "note", "A third node"]);
    assert_eq!(team.names("ann").len(), 3);

    // When nothing changed, no file is touched: the same bytes stay.
    let before = snapshot(&team.proj("ann").join(DATA));
    team.son("ann", &["recall", "money"]);
    let status = team.son("ann", &["files", "--json"]);
    assert_eq!(snapshot(&team.proj("ann").join(DATA)), before);
    let status: Value = serde_json::from_str(&status.stdout).unwrap();
    assert_eq!(status["files"]["on"], true);
    assert_eq!(status["files"]["nodes"], 3);
    assert_eq!(status["files"]["collection"], "shop");

    // Off: the setting goes, the folder and the nodes stay, and a write
    // no longer changes the files.
    team.son("ann", &["files", "off"]);
    assert_eq!(
        std::fs::read_to_string(team.proj("ann").join(".son")).unwrap(),
        "# our settings\nfuture_key = 1\ncollection = shop\n"
    );
    team.son("ann", &["remember", "note", "A fourth node"]);
    assert_eq!(team.names("ann").len(), 3);
    assert!(team
        .son("ann", &["recall", "money"])
        .stdout
        .contains("Money is cents"));
}

#[test]
fn a_collection_without_the_setting_gets_no_files() {
    let team = Team::new("plain");
    let out = team.son("ann", &["remember", "fact", "Nothing is shared here"]);
    assert_eq!(out.stderr, "");
    assert!(!team.proj("ann").join(DATA).exists());
    assert!(!team.proj("ann").join(".son").exists());
}

#[test]
fn what_one_person_does_reaches_the_other() {
    let team = Team::new("pass");
    team.start();
    let fact = team.son(
        "ann",
        &["remember", "fact", "Money is cents", "--tag", "billing"],
    );
    let fact = fact.stdout.split('`').nth(1).unwrap().to_string();
    let rule = team.son(
        "ann",
        &["remember", "decision", "No floats in the billing tables"],
    );
    let rule = rule.stdout.split('`').nth(1).unwrap().to_string();
    team.son("ann", &["link", &rule, "derived_from", &fact]);
    team.son("ann", &["todo", "add", "Rotate the key"]);

    // Bob's first command in his clone reads the files.
    team.pull("bob", "ann");
    let context = team.son("bob", &["context"]);
    assert!(
        context.stderr.contains("read .son-data: 3 new"),
        "{}",
        context.stderr
    );
    assert!(context.stdout.contains("Money is cents"));
    assert!(context.stdout.contains("#1"));
    assert_eq!(team.content("bob"), team.content("ann"));
    // A node from a teammate is a normal node, with its author.
    let shown = team.son("bob", &["show", &fact, "--json"]);
    assert!(
        shown.stdout.contains("\"author\": \"ann\""),
        "{}",
        shown.stdout
    );

    // Bob edits, verifies, finishes, unlinks and drops; Ann gets all of it.
    team.son(
        "bob",
        &["edit", &fact, "--body", "Stripe amounts are in cents."],
    );
    team.son("bob", &["verify", &rule, "--checked", "read the schema"]);
    team.son("bob", &["todo", "done", "1", "--note", "rotated"]);
    team.son("bob", &["unlink", &rule, "derived_from", &fact]);
    team.son("bob", &["remember", "note", "A note to drop"]);
    let note = team.file("bob", "A note to drop");
    let note_id = read_json(&note).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    team.son("bob", &["drop", &note_id]);
    assert!(read_json(&note).unwrap()["deleted_at"].is_string());
    team.pull("ann", "bob");
    let recall = team.son("ann", &["recall", "stripe"]);
    assert!(recall.stdout.contains("Stripe amounts are in cents."));
    assert_eq!(team.content("ann"), team.content("bob"));
    let ann = team.content("ann");
    assert_eq!(
        ann.last().unwrap().as_array().unwrap().len(),
        0,
        "the link is gone"
    );
    assert!(
        team.son("ann", &["todo", "list"])
            .stdout
            .contains("no open todos")
            || !team
                .son("ann", &["todo", "list"])
                .stdout
                .contains("Rotate the key")
    );
    // Ann's files are as Bob's: nothing is written back in another form.
    assert_eq!(
        snapshot(&team.nodes("ann"))
            .into_iter()
            .map(|(path, bytes)| (path.file_name().unwrap().to_owned(), bytes))
            .collect::<Vec<_>>(),
        snapshot(&team.nodes("bob"))
            .into_iter()
            .map(|(path, bytes)| (path.file_name().unwrap().to_owned(), bytes))
            .collect::<Vec<_>>()
    );

    // The sync is one operation in the history, and can be undone.
    let history = team.son("ann", &["history", &fact]);
    assert!(history.stdout.contains("sync"), "{}", history.stdout);
}

#[test]
fn two_people_who_each_add_a_node_both_get_both() {
    let team = Team::new("both");
    team.start();
    team.son("ann", &["remember", "fact", "Ann's fact"]);
    team.son("bob", &["remember", "fact", "Bob's fact"]);
    team.pull("ann", "bob");
    team.pull("bob", "ann");
    for person in ["ann", "bob"] {
        let list = team.son(person, &["list"]).stdout;
        assert!(
            list.contains("Ann's fact") && list.contains("Bob's fact"),
            "{list}"
        );
        assert_eq!(team.names(person).len(), 2);
    }
    assert_eq!(team.content("ann"), team.content("bob"));
}

#[test]
fn when_both_edit_one_node_the_later_edit_stays_everywhere() {
    let team = Team::new("edit");
    team.start();
    let id = team.son("ann", &["remember", "fact", "The limit"]);
    let id = id.stdout.split('`').nth(1).unwrap().to_string();
    team.pull("bob", "ann");
    team.son("bob", &["list"]);
    team.son("ann", &["edit", &id, "--body", "The limit is 10."]);
    std::thread::sleep(std::time::Duration::from_millis(20));
    team.son("bob", &["edit", &id, "--body", "The limit is 20."]);
    // Ann gets Bob's later edit.
    team.pull("ann", "bob");
    assert!(team
        .son("ann", &["show", &id])
        .stdout
        .contains("The limit is 20."));
    // Bob gets Ann's older file: his edit stays, and his file is put back.
    std::fs::copy(team.file("ann", "The limit"), team.file("bob", "The limit")).unwrap();
    let ann_old = team.node("ann", "The limit");
    assert_eq!(ann_old["body"], "The limit is 20.");
    std::fs::write(
        team.file("bob", "The limit"),
        std::fs::read_to_string(team.file("bob", "The limit"))
            .unwrap()
            .replace("The limit is 20.", "The limit is 10."),
    )
    .unwrap();
    // The file now says 10 with the time of the edit to 20: the same
    // time, another content. The database wins and takes the time of now.
    assert!(team
        .son("bob", &["show", &id])
        .stdout
        .contains("The limit is 20."));
    let bob = team.node("bob", "The limit");
    assert_eq!(bob["body"], "The limit is 20.");
    assert!(bob["updated_at"].as_str() > ann_old["updated_at"].as_str());
    team.pull("ann", "bob");
    team.son("ann", &["list"]);
    assert_eq!(team.content("ann"), team.content("bob"));
}

#[test]
fn two_todos_with_one_number_end_with_the_same_numbers_everywhere() {
    let team = Team::new("numbers");
    team.start();
    team.son("ann", &["todo", "add", "The first todo"]);
    team.pull("bob", "ann");
    team.son("bob", &["todo", "list"]);
    // Both add #2. Bob's is made first, and has a child.
    team.son("bob", &["todo", "add", "Bob's todo"]);
    team.son("bob", &["todo", "add", "Bob's child", "--parent", "2"]);
    std::thread::sleep(std::time::Duration::from_millis(5));
    team.son("ann", &["todo", "add", "Ann's todo"]);
    team.son("ann", &["todo", "add", "Ann's child", "--parent", "2"]);
    assert_eq!(team.node("ann", "Ann's todo")["number"], "2");
    assert_eq!(team.node("bob", "Bob's todo")["number"], "2");

    // Each gets the other's files. The todo that was made first keeps
    // the number on both machines.
    let ann_files = snapshot(&team.nodes("ann"));
    team.pull("ann", "bob");
    for (path, bytes) in ann_files {
        let target = team.nodes("bob").join(path.file_name().unwrap());
        if !target.exists() {
            std::fs::write(target, bytes).unwrap();
        }
    }
    let ann = team.son("ann", &["todo", "backlog"]);
    assert!(ann.stderr.contains("todo #2 is now #3"), "{}", ann.stderr);
    let bob = team.son("bob", &["todo", "backlog"]);
    assert!(bob.stderr.contains("todo #2 is now #3"), "{}", bob.stderr);
    for person in ["ann", "bob"] {
        assert_eq!(team.node(person, "Bob's todo")["number"], "2");
        assert_eq!(team.node(person, "Bob's child")["number"], "2.1");
        assert_eq!(team.node(person, "Ann's todo")["number"], "3");
        assert_eq!(team.node(person, "Ann's child")["number"], "3.1");
    }
    // One more exchange, both ways: the later of the two changes of
    // number is the one that stays, and then nothing moves.
    team.pull("ann", "bob");
    team.son("ann", &["list"]);
    team.pull("bob", "ann");
    team.son("bob", &["list"]);
    assert_eq!(team.content("ann"), team.content("bob"));
    let settled = snapshot(&team.nodes("ann"));
    team.pull("ann", "bob");
    team.son("ann", &["list"]);
    assert_eq!(snapshot(&team.nodes("ann")), settled);
}

#[test]
fn a_purge_is_passed_on() {
    let team = Team::new("purge");
    team.start();
    team.son("ann", &["remember", "fact", "Keep this"]);
    let secret = team.son("ann", &["remember", "note", "The deploy note"]);
    let secret = secret.stdout.split('`').nth(1).unwrap().to_string();
    team.pull("bob", "ann");
    assert!(team
        .son("bob", &["list"])
        .stdout
        .contains("The deploy note"));
    let file = team.file("ann", "The deploy note");
    let name = file.file_name().unwrap().to_owned();

    team.son("ann", &["drop", &secret, "--purge"]);
    assert!(!file.exists(), "the file of a purged node is removed");
    let id = name.to_str().unwrap().trim_end_matches(".json").to_string();
    assert!(team
        .proj("ann")
        .join(DATA)
        .join("purged")
        .join(&id)
        .exists());

    // Bob still has the file and the node. The pull brings the purge.
    team.pull("bob", "ann");
    let list = team.son("bob", &["list", "--dropped"]);
    assert!(list.stderr.contains("1 purged"), "{}", list.stderr);
    assert!(!list.stdout.contains("The deploy note"));
    assert!(list.stdout.contains("Keep this"));
    assert!(!team.nodes("bob").join(&name).exists());
    assert!(!team.run_in("bob", "proj", &["show", &secret]).ok);
    // It does not come back from a clone that still has the file.
    std::fs::write(team.nodes("ann").join(&name), "{}").unwrap();
    team.son("ann", &["list"]);
    assert!(!team.nodes("ann").join(&name).exists());
}

#[test]
fn a_file_that_cannot_be_used_is_skipped_and_named_and_the_command_runs() {
    let team = Team::new("bad");
    team.start();
    team.son("ann", &["remember", "fact", "A good fact"]);
    team.son("ann", &["remember", "fact", "A fact in conflict"]);
    let file = team.file("ann", "A fact in conflict");
    let good = std::fs::read_to_string(&file).unwrap();
    std::fs::write(
        &file,
        format!("<<<<<<< HEAD\n{good}=======\n{good}>>>>>>> theirs\n"),
    )
    .unwrap();
    let recall = team.son("ann", &["recall", "fact"]);
    assert!(recall.stdout.contains("A good fact"));
    assert!(
        recall.stdout.contains("A fact in conflict"),
        "the node stays in the database"
    );
    assert!(
        recall.stderr.contains("skipped .son-data/nodes/"),
        "{}",
        recall.stderr
    );
    assert!(recall.stderr.contains("marks of a git conflict"));
    // The file is left for a person: a write does not replace it.
    team.son("ann", &["remember", "note", "Another write"]);
    assert!(std::fs::read_to_string(&file)
        .unwrap()
        .starts_with("<<<<<<<"));
    let status = team.son("ann", &["files"]);
    assert!(
        status.stdout.contains("skipped: nodes/"),
        "{}",
        status.stdout
    );
    let doctor = team.run_in("ann", "proj", &["doctor"]);
    assert!(doctor.stdout.contains("shared folder"), "{}", doctor.stdout);
    assert!(
        doctor
            .stdout
            .contains(file.file_name().unwrap().to_str().unwrap()),
        "{}",
        doctor.stdout
    );
    // Repaired: it is read again, and the warning stops.
    std::fs::write(&file, &good).unwrap();
    let after = team.son("ann", &["recall", "fact"]);
    assert_eq!(after.stderr, "");

    // Other files that are not used: a wrong id, an unknown type, and
    // text that looks like a secret.
    let other = team.nodes("ann").join("01ZZZZZZZZZZZZZZZZZZZZZZZZ.json");
    std::fs::write(&other, &good).unwrap();
    assert!(team
        .son("ann", &["list"])
        .stderr
        .contains("the id in the file is not the name of the file"));
    std::fs::write(&other, good.replace("\"fact\"", "\"wish\"")).unwrap();
    std::fs::remove_file(&other).unwrap();
    let secret = good
        .replace("A fact in conflict", "A new key")
        .replace(
            &team.node("ann", "A fact in conflict")["id"]
                .as_str()
                .unwrap()
                .to_string(),
            "01ZZZZZZZZZZZZZZZZZZZZZZZY",
        )
        .replace(
            "\"title\"",
            "\"body\": \"password=hunter2hunter2hunter2\",\n  \"title\"",
        );
    let secret_file = team.nodes("ann").join("01ZZZZZZZZZZZZZZZZZZZZZZZY.json");
    std::fs::write(&secret_file, secret).unwrap();
    let list = team.son("ann", &["list"]);
    assert!(
        list.stderr.contains("looks like it holds"),
        "{}",
        list.stderr
    );
    assert!(!list.stderr.contains("hunter2"), "the text is not printed");
    assert!(!list.stdout.contains("A new key"));
}

#[test]
fn a_file_deleted_by_hand_comes_back_and_a_hand_edit_comes_in() {
    let team = Team::new("hand");
    team.start();
    team.son(
        "ann",
        &["remember", "fact", "The port", "--body", "It is 8080."],
    );
    let file = team.file("ann", "The port");
    std::fs::remove_file(&file).unwrap();
    team.son("ann", &["list"]);
    assert!(file.exists(), "only son drop removes a node");

    // A hand edit with a later time comes in.
    let text = std::fs::read_to_string(&file).unwrap();
    let edited: String = text
        .lines()
        .map(|line| {
            if line.contains("\"updated_at\"") {
                "  \"updated_at\": \"2099-01-01T00:00:00.000Z\"".to_string()
            } else {
                line.replace("It is 8080.", "It is 9090.")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&file, edited).unwrap();
    let recall = team.son("ann", &["recall", "port"]);
    assert!(recall.stdout.contains("It is 9090."), "{}", recall.stdout);
    assert!(recall.stderr.contains("1 changed"), "{}", recall.stderr);
    // The file was not in the form son writes (no line end at the end):
    // it is written again, with the same content.
    assert!(std::fs::read_to_string(&file).unwrap().ends_with("}\n"));
    assert_eq!(team.node("ann", "The port")["body"], "It is 9090.");
}

#[test]
fn a_write_from_another_folder_updates_the_files() {
    let team = Team::new("elsewhere");
    team.start();
    let id = team.son("ann", &["remember", "fact", "The region"]);
    let id = id.stdout.split('`').nth(1).unwrap().to_string();
    // From a folder that is not the project: an edit by id, and a new
    // node with --collection.
    let edit = team.run_in("ann", "other", &["edit", &id, "--body", "It is eu-west-1."]);
    assert!(edit.ok, "{}", edit.stderr);
    assert_eq!(team.node("ann", "The region")["body"], "It is eu-west-1.");
    let add = team.run_in(
        "ann",
        "other",
        &["remember", "note", "Made elsewhere", "--collection", "shop"],
    );
    assert!(add.ok, "{}", add.stderr);
    assert_eq!(team.names("ann").len(), 2);
    // A node of another collection is not written here.
    team.run_in("ann", "other", &["remember", "note", "Not of the shop"]);
    assert_eq!(team.names("ann").len(), 2);

    // The project folder is removed: a write still succeeds.
    std::fs::remove_dir_all(team.proj("ann")).unwrap();
    let after = team.run_in("ann", "other", &["edit", &id, "--body", "It moved."]);
    assert!(after.ok, "{}", after.stderr);
    assert_eq!(after.stderr, "");
}

#[test]
fn a_folder_of_a_later_format_is_left_alone() {
    let team = Team::new("format");
    team.start();
    team.son("ann", &["remember", "fact", "A fact"]);
    let collection = team.proj("ann").join(DATA).join("collection.json");
    let text = std::fs::read_to_string(&collection).unwrap();
    std::fs::write(
        &collection,
        text.replace("\"version\": 1", "\"version\": 2"),
    )
    .unwrap();
    let before = snapshot(&team.proj("ann").join(DATA));
    let out = team.son("ann", &["remember", "fact", "Another fact"]);
    assert!(out.stderr.contains("install a newer son"), "{}", out.stderr);
    assert_eq!(snapshot(&team.proj("ann").join(DATA)), before);
}

#[test]
fn the_folder_can_have_another_name_and_must_stay_inside() {
    let team = Team::new("folder");
    team.son("ann", &["remember", "fact", "A fact"]);
    let outside = team.run_in("ann", "proj", &["files", "on", "../elsewhere"]);
    assert!(!outside.ok);
    assert!(outside.stderr.contains("inside"), "{}", outside.stderr);
    assert!(!team.proj("ann").join(".son").exists());
    team.son("ann", &["files", "on", "docs/memory"]);
    let text = std::fs::read_to_string(team.proj("ann").join(".son")).unwrap();
    // The collection is named in the file, so that every clone gets it.
    assert!(text.contains("collection = proj"), "{text}");
    assert!(text.contains("files = docs/memory"), "{text}");
    assert!(team
        .proj("ann")
        .join("docs")
        .join("memory")
        .join("collection.json")
        .exists());
}

#[test]
fn a_command_in_a_large_folder_is_fast_when_nothing_changed() {
    let team = Team::new("large");
    team.start();
    // 2,000 nodes through one import, then their files.
    let nodes: Vec<Value> = (0..2000)
        .map(|n| {
            serde_json::json!({
                "id": format!("01JD3K000000000000000A{n:04}"),
                "type": "fact",
                "title": format!("Fact number {n}"),
                "body": "Some text that a fact would hold, of a usual length.",
                "collection_id": "01JD3K00000000000000000000",
                "created_at": "2026-11-19T15:02:11.000Z",
                "updated_at": "2026-11-19T15:02:11.000Z",
            })
        })
        .chain(std::iter::once(serde_json::json!({
            "id": "01JD3K00000000000000000000",
            "type": "collection",
            "title": "shop",
            "created_at": "2026-11-19T15:02:11.000Z",
            "updated_at": "2026-11-19T15:02:11.000Z",
        })))
        .collect();
    let dump = serde_json::json!({ "version": 2, "exported_at": "2026-11-19T15:02:11.000Z", "nodes": nodes });
    let file = team.root.join("dump.json");
    std::fs::write(&file, dump.to_string()).unwrap();
    team.son("ann", &["import", file.to_str().unwrap()]);
    assert_eq!(team.names("ann").len(), 2000);
    // Past the time in which a new file is read once more.
    std::thread::sleep(std::time::Duration::from_millis(2100));
    team.son("ann", &["recall", "fact"]);
    let started = std::time::Instant::now();
    for _ in 0..5 {
        team.son("ann", &["recall", "fact"]);
    }
    let each = started.elapsed() / 5;
    assert!(
        each < std::time::Duration::from_millis(1500),
        "a recall took {each:?}"
    );
}

#[test]
fn init_for_a_project_turns_it_on_only_when_asked() {
    let team = Team::new("init");
    team.son("ann", &["remember", "fact", "A fact"]);
    // Without a terminal and without --files, nothing is shared.
    team.son(
        "ann",
        &["init", "--agent", "claude-code", "--scope", "project"],
    );
    assert!(!team.proj("ann").join(DATA).exists());
    // --files is for a project folder only.
    let global = team.run_in(
        "ann",
        "proj",
        &[
            "init",
            "--agent",
            "claude-code",
            "--scope",
            "global",
            "--files",
        ],
    );
    assert!(!global.ok);
    assert!(
        global.stderr.contains("--scope project"),
        "{}",
        global.stderr
    );

    let out = team.son(
        "ann",
        &[
            "--json",
            "init",
            "--agent",
            "claude-code",
            "--scope",
            "project",
            "--files",
        ],
    );
    let report: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(report["files"]["on"], true);
    assert_eq!(report["files"]["nodes"], 1);
    assert_eq!(team.names("ann").len(), 1);
    let text = std::fs::read_to_string(team.proj("ann").join(".son")).unwrap();
    assert!(text.contains("files = .son-data"), "{text}");
    // Again: the same state, and nothing is written twice.
    let again = team.son(
        "ann",
        &[
            "init",
            "--agent",
            "claude-code",
            "--scope",
            "project",
            "--files",
        ],
    );
    assert!(
        again.stdout.contains("files: on for collection 'proj'"),
        "{}",
        again.stdout
    );
    assert_eq!(
        std::fs::read_to_string(team.proj("ann").join(".son")).unwrap(),
        text
    );
}

#[test]
fn a_sync_can_be_undone_and_a_node_in_another_collection_is_left_alone() {
    let team = Team::new("undo");
    team.start();
    let id = team.son(
        "ann",
        &["remember", "fact", "The zone", "--body", "It is a."],
    );
    let id = id.stdout.split('`').nth(1).unwrap().to_string();
    team.pull("bob", "ann");
    team.son("bob", &["list"]);
    team.son("bob", &["edit", &id, "--body", "It is b."]);
    team.pull("ann", "bob");
    assert!(team.son("ann", &["show", &id]).stdout.contains("It is b."));
    // Ann undoes what the sync brought. Her undo is the latest change,
    // so it is what Bob gets.
    team.son("ann", &["undo", &id]);
    assert_eq!(team.node("ann", "The zone")["body"], "It is a.");
    team.pull("bob", "ann");
    assert!(team.son("bob", &["show", &id]).stdout.contains("It is a."));

    // Ann moves the node to another collection. Its file stays as it
    // was for the others, and is not read into her database again.
    team.son("ann", &["edit", &id, "--collection", "private"]);
    let file = team.file("ann", "The zone");
    let before = std::fs::read(&file).unwrap();
    team.son("bob", &["edit", &id, "--body", "It is c."]);
    team.pull("ann", "bob");
    let shown = team.son("ann", &["show", &id]);
    assert_eq!(shown.stderr, "");
    assert!(shown.stdout.contains("It is a."), "{}", shown.stdout);
    assert!(shown.stdout.contains("private"));
    assert_ne!(std::fs::read(&file).unwrap(), before, "the file is Bob's");
    let status = team.son("ann", &["files"]);
    assert!(
        status.stdout.contains("in another collection"),
        "{}",
        status.stdout
    );
}

#[test]
fn a_command_that_only_reads_does_not_wait_for_a_database_in_use() {
    let team = Team::new("busy");
    team.start();
    team.son("ann", &["remember", "fact", "Here already"]);
    team.son("bob", &["remember", "fact", "From Bob"]);
    // Another session holds the write lock while Bob's file arrives.
    let other = rusqlite::Connection::open(team.root.join("ann").join("son.db")).unwrap();
    other.execute_batch("BEGIN IMMEDIATE").unwrap();
    team.pull("ann", "bob");
    let started = std::time::Instant::now();
    let list = team.son("ann", &["list"]);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the command waited for the lock"
    );
    assert!(list.stdout.contains("Here already"));
    assert!(!list.stdout.contains("From Bob"));
    // The lock is gone: the next command reads the file.
    other.execute_batch("ROLLBACK").unwrap();
    let list = team.son("ann", &["list"]);
    assert!(list.stdout.contains("From Bob"), "{}", list.stdout);
}
