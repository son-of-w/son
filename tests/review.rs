//! `son review` finds possible duplicates for all nodes in one pass. These
//! tests hold it to what the search for one node gives, and to a time.

use son_of_w::analyze::Analyzer;
use son_of_w::db;
use son_of_w::search;
use son_of_w::store::{self, Meta, NewNode};
use std::path::PathBuf;

struct Corpus {
    conn: rusqlite::Connection,
    an: Analyzer,
    meta: Meta,
    path: PathBuf,
    collection: String,
}

impl Corpus {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("son-review-{name}-{}.db", std::process::id()));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        let conn = db::open(&path).unwrap();
        let an = Analyzer::new();
        let meta = Meta::from_env();
        let collection = store::ensure_project(&conn, &an, &meta, "corpus").unwrap();
        Self {
            conn,
            an,
            meta,
            path,
            collection,
        }
    }

    fn add(&self, kind: &str, title: &str, body: &str) -> String {
        store::insert_node(
            &self.conn,
            &self.an,
            &self.meta,
            &NewNode {
                kind,
                title,
                body,
                status: (kind == "todo").then_some("open"),
                project_id: Some(&self.collection),
                tags: &[],
                evidence: None,
                refs: &[],
                priority: None,
                number: None,
            },
        )
        .unwrap()
    }
}

impl Drop for Corpus {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.path.display()));
        }
    }
}

/// A small number generator with a fixed start, so that a corpus is the
/// same on every run.
struct Dice(u64);

impl Dice {
    fn next(&mut self, below: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % below
    }

    fn words(&mut self, vocabulary: &[&str], count: usize) -> String {
        (0..count)
            .map(|_| vocabulary[self.next(vocabulary.len())])
            .collect::<Vec<_>>()
            .join(" ")
    }
}

const WORDS: [&str; 40] = [
    "deploy",
    "terraform",
    "postgres",
    "cache",
    "queue",
    "billing",
    "invoice",
    "login",
    "token",
    "session",
    "router",
    "handler",
    "schema",
    "migration",
    "backup",
    "restore",
    "index",
    "search",
    "rank",
    "synonym",
    "agent",
    "skill",
    "memory",
    "recall",
    "context",
    "collection",
    "review",
    "verify",
    "merge",
    "link",
    "priority",
    "export",
    "import",
    "release",
    "homebrew",
    "windows",
    "config",
    "environment",
    "retry",
    "timeout",
];

#[test]
fn one_pass_gives_what_the_search_for_each_node_gives() {
    let corpus = Corpus::new("same");
    let mut dice = Dice(7);
    let kinds = ["fact", "decision", "pattern", "note", "todo"];
    // Titles of two or three words from a small vocabulary, so that many
    // share words; some bodies and some titles are repeated on purpose.
    // Bodies use other words than titles, and the corpus is small enough
    // that no title's words are in more than 50 nodes: the search for one
    // node looks at its 50 best matches only, and beyond that the two
    // ways can differ (the one pass then finds more).
    let body_words: Vec<String> = (0..200).map(|i| format!("bodyword{i}")).collect();
    let body_words: Vec<&str> = body_words.iter().map(String::as_str).collect();
    for i in 0..120 {
        let words = 2 + dice.next(2);
        let title = dice.words(&WORDS, words);
        let body = if i % 9 == 0 {
            "exactly this body again".to_string()
        } else if i % 4 == 0 {
            String::new()
        } else {
            dice.words(&body_words, 12)
        };
        corpus.add(kinds[dice.next(kinds.len())], &title, &body);
    }
    for (kind, title, body) in [
        ("fact", "API port is 8443", "The port moved from 8080."),
        ("fact", "api port is 8443", ""),
        ("note", "API port is 8443", "The port moved from 8080."),
        (
            "fact",
            "Staging account alias",
            "Billing uses the stg alias for staging.",
        ),
        (
            "fact",
            "Unrelated title words here",
            "Billing uses the stg alias for staging.",
        ),
        ("decision", "Each route in its own file in routes", ""),
        ("pattern", "One file per route in routes", ""),
        ("note", "The", "only stop words in the title"),
        ("note", "!!!", "no words in the title"),
        ("note", "!!!", "no words in the title"),
    ] {
        corpus.add(kind, title, body);
    }

    let all = search::all_duplicates(&corpus.conn, &corpus.an, 3).unwrap();
    let nodes = store::list_nodes(&corpus.conn, None, None, false).unwrap();
    let mut compared = 0;
    let mut with_candidates = 0;
    for node in nodes.iter().filter(|node| node.kind != "collection") {
        let one = search::duplicates(
            &corpus.conn,
            &corpus.an,
            &node.title,
            &node.body,
            &node.kind,
            Some(&node.id),
            3,
        )
        .unwrap();
        let shape = |list: &[search::Candidate]| -> Vec<(String, String, Vec<String>)> {
            list.iter()
                .map(|c| {
                    (
                        c.node.id.clone(),
                        format!("{:.6}", c.score),
                        c.reasons.clone(),
                    )
                })
                .collect()
        };
        let pass = all.get(&node.id).map(Vec::as_slice).unwrap_or_default();
        assert_eq!(
            shape(&one),
            shape(pass),
            "for [{}] {} / {}",
            node.kind,
            node.title,
            node.body
        );
        compared += 1;
        with_candidates += usize::from(!one.is_empty());
    }
    assert_eq!(compared, 130);
    assert!(
        with_candidates > 30,
        "the corpus must have duplicates to compare: {with_candidates}"
    );
}

#[test]
fn a_review_of_thousands_of_nodes_takes_seconds_not_minutes() {
    let corpus = Corpus::new("time");
    let mut dice = Dice(11);
    // The worst case for the comparison: every title is five words from
    // forty, so each node shares a word with most others.
    corpus.conn.execute_batch("BEGIN").unwrap();
    for _ in 0..3000 {
        let title = dice.words(&WORDS, 5);
        let body = dice.words(&WORDS, 30);
        corpus.add("fact", &title, &body);
    }
    corpus.conn.execute_batch("COMMIT").unwrap();

    let started = std::time::Instant::now();
    let items = son_of_w::review::items(
        &corpus.conn,
        &corpus.an,
        &son_of_w::review::Limits::from_env(),
        &son_of_w::review::Filter {
            collection_id: None,
            kind: None,
            tags: &[],
        },
    )
    .unwrap();
    let took = started.elapsed();
    assert!(!items.is_empty());
    // Asking the search index once for each node took more than a minute
    // for this corpus in a test build. The limit is wide, so that a slow
    // machine does not fail it; it still fails if that way comes back.
    assert!(
        took.as_secs() < 20,
        "review of 3000 nodes took {took:?}; it must not grow with the square of the search"
    );
}
