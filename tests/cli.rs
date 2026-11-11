//! End-to-end tests: run the real `son` binary against a throwaway database
//! and a throwaway home directory.

use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

struct Sandbox {
    root: PathBuf,
}

struct Out {
    ok: bool,
    stdout: String,
    stderr: String,
}

impl Out {
    /// The first `` `id` `` in the output.
    fn id(&self) -> String {
        let start = self.stdout.find('`').expect("output has an id") + 1;
        let end = start + self.stdout[start..].find('`').expect("id is closed");
        self.stdout[start..end].to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).expect("output is JSON")
    }
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("son-cli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["home", "alpha", "beta"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        Self { root }
    }

    fn command(&self, dir: &str, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_son"));
        command
            .args(args)
            .current_dir(self.root.join(dir))
            .env("SON_DB", self.root.join("son.db"))
            .env("HOME", self.root.join("home"))
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("SON_SOURCE");
        command
    }

    fn son(&self, dir: &str, args: &[&str]) -> Out {
        let output = self.command(dir, args).output().unwrap();
        Out {
            ok: output.status.success(),
            stdout: String::from_utf8(output.stdout).unwrap(),
            stderr: String::from_utf8(output.stderr).unwrap(),
        }
    }

    /// Run and require success.
    fn ok(&self, dir: &str, args: &[&str]) -> Out {
        let out = self.son(dir, args);
        assert!(out.ok, "son {args:?} failed: {}", out.stderr);
        out
    }

    fn ok_with_stdin(&self, dir: &str, args: &[&str], input: &str) -> Out {
        let mut child = self
            .command(dir, args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "son {args:?} failed");
        Out {
            ok: true,
            stdout: String::from_utf8(output.stdout).unwrap(),
            stderr: String::from_utf8(output.stderr).unwrap(),
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn position(haystack: &str, needle: &str) -> usize {
    haystack
        .find(needle)
        .unwrap_or_else(|| panic!("'{needle}' is missing from:\n{haystack}"))
}

#[test]
fn recall_searches_every_project_and_lists_the_current_one_first() {
    let sb = Sandbox::new("recall");
    sb.ok(
        "alpha",
        &[
            "remember",
            "pattern",
            "Terraform AWS layout",
            "--body",
            "One module per service.",
            "--tag",
            "terraform,aws",
        ],
    );
    sb.ok("beta", &["remember", "note", "Terraform state for beta"]);

    let from_beta = sb.ok("beta", &["recall", "terraform"]).stdout;
    assert!(
        position(&from_beta, "Terraform state for beta")
            < position(&from_beta, "Terraform AWS layout")
    );
    assert!(from_beta.contains("project: beta (current)"));
    assert!(from_beta.contains("project: alpha"));

    let from_alpha = sb.ok("alpha", &["recall", "terraform"]).stdout;
    assert!(
        position(&from_alpha, "Terraform AWS layout")
            < position(&from_alpha, "Terraform state for beta")
    );

    let only_alpha = sb
        .ok("beta", &["recall", "terraform", "--project", "alpha"])
        .stdout;
    assert!(!only_alpha.contains("Terraform state for beta"));

    let only_notes = sb
        .ok("alpha", &["recall", "terraform", "--type", "note"])
        .stdout;
    assert!(!only_notes.contains("Terraform AWS layout"));
}

#[test]
fn recall_from_a_project_with_nothing_stored_still_finds_other_projects() {
    let sb = Sandbox::new("empty");
    sb.ok(
        "alpha",
        &["remember", "fact", "Staging account alias is stg"],
    );
    let out = sb.ok("beta", &["recall", "staging", "account"]).stdout;
    assert!(out.contains("Staging account alias is stg"));
    assert!(sb
        .ok("beta", &["recall", "nonexistentword"])
        .stdout
        .contains("No matches"));
}

#[test]
fn queries_with_punctuation_do_not_break_the_search() {
    let sb = Sandbox::new("punct");
    sb.ok("alpha", &["remember", "note", "foo bar baz"]);
    let out = sb.ok("alpha", &["recall", "foo-bar: \"baz\" AND (x*", "NOT"]);
    assert!(out.stdout.contains("foo bar baz"));
}

#[test]
fn short_ids_show_edit_and_reindex() {
    let sb = Sandbox::new("edit");
    let id = sb
        .ok(
            "alpha",
            &["remember", "decision", "Use Postgres", "--tag", "db"],
        )
        .id();
    assert_eq!(id.len(), 8);

    let shown = sb.ok("beta", &["show", &id]).stdout;
    assert!(shown.contains("# Use Postgres"));
    assert!(shown.contains("project: alpha"));

    // The full id and a shorter tail both work.
    let full = sb.ok("alpha", &["--json", "show", &id]).json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(sb
        .ok("alpha", &["show", &full])
        .stdout
        .contains("Use Postgres"));
    assert!(sb
        .ok("alpha", &["show", &id[4..]])
        .stdout
        .contains("Use Postgres"));

    sb.ok(
        "alpha",
        &[
            "edit",
            &id,
            "--title",
            "Use SQLite",
            "--body",
            "One file.",
            "--tag",
            "storage",
        ],
    );
    assert!(sb
        .ok("alpha", &["recall", "sqlite"])
        .stdout
        .contains("Use SQLite"));
    assert!(sb
        .ok("alpha", &["recall", "postgres"])
        .stdout
        .contains("No matches"));
    assert!(sb
        .ok("alpha", &["show", &id])
        .stdout
        .contains("tags: storage"));

    assert!(!sb.son("alpha", &["edit", &id]).ok);
    assert!(!sb.son("alpha", &["show", "zzzzzzzz"]).ok);
    assert!(!sb.son("alpha", &["show", "ab"]).ok);

    // Moving a node creates the target project when needed.
    sb.ok("alpha", &["edit", &id, "--project", "gamma"]);
    assert!(sb
        .ok("alpha", &["show", &id])
        .stdout
        .contains("project: gamma"));

    sb.ok("alpha", &["reindex"]);
    assert!(sb
        .ok("alpha", &["recall", "sqlite"])
        .stdout
        .contains("Use SQLite"));
}

#[test]
fn drop_hides_restore_brings_back_purge_removes() {
    let sb = Sandbox::new("drop");
    let id = sb
        .ok("alpha", &["remember", "note", "Temporary scratch idea"])
        .id();

    sb.ok("alpha", &["drop", &id]);
    assert!(sb
        .ok("alpha", &["recall", "scratch"])
        .stdout
        .contains("No matches"));
    assert!(sb.ok("alpha", &["show", &id]).stdout.contains("DROPPED"));
    assert!(!sb.son("alpha", &["drop", &id]).ok);

    sb.ok("alpha", &["restore", &id]);
    assert!(sb
        .ok("alpha", &["recall", "scratch"])
        .stdout
        .contains("Temporary scratch idea"));
    assert!(!sb.son("alpha", &["restore", &id]).ok);

    sb.ok("alpha", &["drop", &id, "--purge"]);
    assert!(!sb.son("alpha", &["show", &id]).ok);

    // A project that still holds nodes cannot be dropped.
    sb.ok("alpha", &["remember", "note", "Keeps the project alive"]);
    let projects = sb.ok("alpha", &["--json", "projects"]).json();
    let project = projects[0]["short_id"].as_str().unwrap().to_string();
    assert!(!sb.son("alpha", &["drop", &project]).ok);
}

#[test]
fn todos_add_list_done() {
    let sb = Sandbox::new("todo");
    let first = sb.ok("alpha", &["todo", "add", "Write the README"]).id();
    sb.ok(
        "beta",
        &["todo", "add", "Rotate the staging key", "--tag", "ops"],
    );

    let alpha = sb.ok("alpha", &["todo", "list"]).stdout;
    assert!(alpha.contains("[ ]") && alpha.contains("Write the README"));
    assert!(!alpha.contains("Rotate the staging key"));

    let all = sb.ok("alpha", &["todo", "list", "--all"]).stdout;
    assert!(all.contains("Write the README") && all.contains("Rotate the staging key"));

    sb.ok("alpha", &["todo", "done", &first]);
    assert!(sb
        .ok("alpha", &["todo", "list"])
        .stdout
        .contains("No todos"));
    assert!(sb
        .ok("alpha", &["todo", "list", "--done"])
        .stdout
        .contains("[x]"));

    // Status is checked, and only todos can be marked done.
    assert!(!sb.son("alpha", &["edit", &first, "--status", "maybe"]).ok);
    sb.ok("alpha", &["edit", &first, "--status", "open"]);
    let note = sb.ok("alpha", &["remember", "note", "Not a todo"]).id();
    assert!(!sb.son("alpha", &["todo", "done", &note]).ok);
}

#[test]
fn son_project_file_names_the_project() {
    let sb = Sandbox::new("marker");
    let nested = sb.root.join("alpha").join("deep").join("er");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(sb.root.join("alpha").join(".son-project"), "billing-api\n").unwrap();

    let out = sb
        .ok("alpha/deep/er", &["remember", "note", "Lives in billing"])
        .stdout;
    assert!(out.contains("(billing-api)"));
    assert!(sb
        .ok("alpha", &["projects"])
        .stdout
        .contains("billing-api (current)"));
}

#[test]
fn body_is_read_from_stdin() {
    let sb = Sandbox::new("stdin");
    let body = "First line.\n\n- a list item\n- another\n";
    let id = sb
        .ok_with_stdin(
            "alpha",
            &["remember", "pattern", "Multi-line body", "--body", "-"],
            body,
        )
        .id();
    let shown = sb.ok("alpha", &["show", &id]).stdout;
    assert!(shown.contains("First line.\n\n- a list item\n- another"));
}

#[test]
fn links_and_superseded_decisions() {
    let sb = Sandbox::new("links");
    let old = sb
        .ok(
            "alpha",
            &["remember", "decision", "Deploy with Jenkins pipelines"],
        )
        .id();
    let new = sb
        .ok(
            "alpha",
            &[
                "remember",
                "decision",
                "Deploy with GitHub Actions pipelines",
                "--link",
                &format!("{old}:supersedes"),
            ],
        )
        .id();
    let related = sb
        .ok(
            "alpha",
            &["remember", "fact", "Runner labels are macos-arm"],
        )
        .id();
    sb.ok("alpha", &["link", &new, "relates_to", &related]);
    assert!(sb
        .ok("alpha", &["link", &new, "relates-to", &related])
        .stdout
        .contains("already linked"));

    let old_shown = sb.ok("alpha", &["show", &old]).stdout;
    assert!(old_shown.contains("SUPERSEDED by"));
    assert!(old_shown.contains("superseded_by"));

    // The newer decision ranks first; the older one is still there, marked.
    let out = sb.ok("alpha", &["recall", "deploy", "pipelines"]).stdout;
    assert!(position(&out, "GitHub Actions") < position(&out, "Jenkins"));
    assert!(out.contains("SUPERSEDED by"));

    // One hop from the hit reaches the linked fact; zero hops does not.
    let hop = sb
        .ok("alpha", &["recall", "github", "actions", "--limit", "1"])
        .stdout;
    assert!(hop.contains("## Linked") && hop.contains("Runner labels are macos-arm"));
    let flat = sb
        .ok(
            "alpha",
            &["recall", "github", "actions", "--limit", "1", "--hops", "0"],
        )
        .stdout;
    assert!(!flat.contains("Runner labels are macos-arm"));

    // If the newer decision is dropped, the older one is current again.
    sb.ok("alpha", &["drop", &new]);
    assert!(!sb
        .ok("alpha", &["show", &old])
        .stdout
        .contains("SUPERSEDED"));
    sb.ok("alpha", &["restore", &new]);

    sb.ok("alpha", &["unlink", &new, "relates_to", &related]);
    assert!(!sb
        .ok("alpha", &["show", &new])
        .stdout
        .contains("Runner labels"));
    assert!(!sb.son("alpha", &["link", &new, "relates_to", &new]).ok);
    assert!(!sb.son("alpha", &["link", &new, "likes", &related]).ok);
}

#[test]
fn context_prints_what_a_project_holds() {
    let sb = Sandbox::new("context");
    sb.ok(
        "alpha",
        &[
            "remember",
            "pattern",
            "Terraform module layout",
            "--body",
            "modules/<service>",
        ],
    );
    let old = sb.ok("alpha", &["remember", "decision", "Old choice"]).id();
    sb.ok(
        "alpha",
        &[
            "remember",
            "decision",
            "New choice",
            "--link",
            &format!("{old}:supersedes"),
        ],
    );
    sb.ok(
        "alpha",
        &["remember", "fact", "Region", "--body", "eu-west-1"],
    );
    sb.ok("alpha", &["todo", "add", "Add remote state"]);
    let done = sb.ok("alpha", &["todo", "add", "Already finished"]).id();
    sb.ok("alpha", &["todo", "done", &done]);

    let out = sb.ok("alpha", &["context"]).stdout;
    assert!(out.contains("# Context: alpha"));
    assert!(out.contains("Terraform module layout") && out.contains("modules/<service>"));
    assert!(out.contains("New choice") && !out.contains("Old choice"));
    assert!(out.contains("Region: eu-west-1"));
    assert!(out.contains("Add remote state") && !out.contains("Already finished"));

    assert!(sb
        .ok("beta", &["context", "--project", "alpha"])
        .stdout
        .contains("# Context: alpha"));
    assert!(sb
        .ok("beta", &["context"])
        .stdout
        .contains("Nothing is stored"));
}

#[test]
fn json_output_is_valid_on_every_command() {
    let sb = Sandbox::new("json");
    let saved = sb
        .ok(
            "alpha",
            &[
                "--json",
                "remember",
                "pattern",
                "JSON shaped",
                "--tag",
                "a,b",
            ],
        )
        .json();
    let id = saved["short_id"].as_str().unwrap().to_string();
    assert_eq!(saved["type"], "pattern");
    assert_eq!(saved["project"], "alpha");
    assert_eq!(saved["tags"], serde_json::json!(["a", "b"]));

    let recall = sb.ok("alpha", &["recall", "json", "--json"]).json();
    assert_eq!(recall["hits"][0]["title"], "JSON shaped");
    assert_eq!(recall["hits"][0]["current_project"], true);

    for args in [
        vec!["show", id.as_str()],
        vec!["projects"],
        vec!["todo", "list"],
        vec!["context"],
        vec!["synonym", "list"],
        vec!["export"],
        vec!["edit", id.as_str(), "--title", "JSON shaped again"],
        vec!["reindex"],
        vec!["drop", id.as_str()],
        vec!["restore", id.as_str()],
        vec!["init", "--no-claude-md"],
    ] {
        let mut full = vec!["--json"];
        full.extend(args);
        sb.ok("alpha", &full).json();
    }
}

#[test]
fn export_and_import_round_trip() {
    let sb = Sandbox::new("export");
    let a = sb
        .ok(
            "alpha",
            &["remember", "decision", "Keep it local", "--tag", "design"],
        )
        .id();
    let b = sb
        .ok(
            "alpha",
            &[
                "remember",
                "pattern",
                "One SQLite file",
                "--body",
                "WAL mode.",
            ],
        )
        .id();
    sb.ok("alpha", &["link", &b, "derived_from", &a]);
    sb.ok("alpha", &["synonym", "add", "sqlite3", "sqlite"]);

    let markdown = sb.ok("alpha", &["export"]).stdout;
    assert!(
        markdown.contains("## Project: alpha")
            && markdown.contains("### [pattern] One SQLite file")
    );

    let dump = sb.ok("alpha", &["export", "--json"]).stdout;
    let file = sb.root.join("dump.json");
    std::fs::write(&file, &dump).unwrap();
    let file = file.to_str().unwrap();

    // Into a fresh database.
    let other = Sandbox::new("export-target");
    let stats = other.ok("alpha", &["--json", "import", file]).json();
    assert_eq!(stats["added"], 3);
    assert_eq!(stats["edges"], 1);
    let shown = other.ok("beta", &["show", &b]).stdout;
    assert!(
        shown.contains("WAL mode.")
            && shown.contains("derived_from")
            && shown.contains("Keep it local")
    );
    assert!(other
        .ok("beta", &["recall", "sqlite3"])
        .stdout
        .contains("One SQLite file"));

    // A second import changes nothing.
    let again = other.ok("alpha", &["--json", "import", file]).json();
    assert_eq!(again["added"], 0);
    assert_eq!(again["unchanged"], 3);

    // Into a database that already has a project of the same name.
    let third = Sandbox::new("export-merge");
    third.ok("alpha", &["remember", "note", "Already here"]);
    let merged = third.ok("alpha", &["--json", "import", file]).json();
    assert_eq!(merged["merged_projects"], 1);
    assert_eq!(
        third
            .ok("alpha", &["--json", "projects"])
            .json()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(third
        .ok("alpha", &["show", &a])
        .stdout
        .contains("project: alpha"));

    assert!(!third.son("alpha", &["import", "/no/such/file.json"]).ok);
}

#[test]
fn init_adds_the_claude_md_block_once_and_keeps_other_text() {
    let sb = Sandbox::new("init");
    let claude_md = sb.root.join("home").join(".claude").join("CLAUDE.md");
    std::fs::create_dir_all(claude_md.parent().unwrap()).unwrap();
    std::fs::write(&claude_md, "# My rules\n\nBe brief.\n").unwrap();

    assert!(sb.ok("alpha", &["init"]).stdout.contains("block added"));
    assert!(sb.ok("alpha", &["init"]).stdout.contains("block unchanged"));
    let text = std::fs::read_to_string(&claude_md).unwrap();
    assert!(text.starts_with("# My rules\n\nBe brief.\n"));
    assert_eq!(text.matches("<!-- son:start -->").count(), 1);
    assert!(text.contains("son recall"));

    // An out-of-date block is replaced in place.
    let stale = text.replace("son recall", "son rekall");
    std::fs::write(&claude_md, format!("{stale}\n## Later section\n")).unwrap();
    assert!(sb.ok("alpha", &["init"]).stdout.contains("block updated"));
    let text = std::fs::read_to_string(&claude_md).unwrap();
    assert!(!text.contains("rekall") && text.ends_with("## Later section\n"));

    // With no file at all, it is created.
    std::fs::remove_file(&claude_md).unwrap();
    assert!(sb.ok("alpha", &["init"]).stdout.contains("block created"));

    // --no-claude-md leaves it alone; --print-block only prints.
    std::fs::remove_file(&claude_md).unwrap();
    sb.ok("alpha", &["init", "--no-claude-md"]);
    assert!(sb
        .ok("alpha", &["init", "--print-block"])
        .stdout
        .starts_with("<!-- son:start -->"));
    assert!(!claude_md.exists());
}

#[test]
fn synonyms_are_used_by_recall() {
    let sb = Sandbox::new("synonym");
    sb.ok(
        "alpha",
        &["remember", "pattern", "Kubernetes ingress setup"],
    );
    // Seeded synonym.
    assert!(sb
        .ok("alpha", &["recall", "k8s"])
        .stdout
        .contains("Kubernetes ingress setup"));
    // User-added synonym.
    assert!(sb
        .ok("alpha", &["recall", "kates"])
        .stdout
        .contains("No matches"));
    sb.ok("alpha", &["synonym", "add", "Kates", "kubernetes"]);
    assert!(sb
        .ok("alpha", &["recall", "kates"])
        .stdout
        .contains("Kubernetes ingress setup"));
    assert!(sb
        .ok("alpha", &["synonym", "list"])
        .stdout
        .contains("kates -> kubernetes"));
    sb.ok("alpha", &["synonym", "remove", "kates"]);
    assert!(sb
        .ok("alpha", &["recall", "kates"])
        .stdout
        .contains("No matches"));
}

#[test]
fn parallel_sessions_do_not_collide() {
    let sb = Sandbox::new("parallel");
    let children: Vec<_> = (0..16)
        .map(|i| {
            sb.command("alpha", &["todo", "add", &format!("Parallel todo {i}")])
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let todos = sb.ok("alpha", &["--json", "todo", "list"]).json();
    assert_eq!(todos.as_array().unwrap().len(), 16);
    // Sixteen first writes at once still make exactly one project.
    assert_eq!(
        sb.ok("alpha", &["--json", "projects"])
            .json()
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
