//! Search quality: a corpus of about 50 nodes, and queries whose best
//! answer is known. These guard the analyzer and the ranking rules.

use rusqlite::Connection;
use son_of_w::analyze::Analyzer;
use son_of_w::db;
use son_of_w::search::{self, Options};
use son_of_w::store::{self, Meta, NewNode};
use std::path::PathBuf;

struct Corpus {
    conn: Connection,
    an: Analyzer,
    meta: Meta,
    path: PathBuf,
}

impl Drop for Corpus {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.path.display()));
        }
    }
}

impl Corpus {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("son-rank-{name}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let corpus = Self {
            conn: db::open(&path).unwrap(),
            an: Analyzer::new(),
            meta: Meta {
                source: None,
                author: None,
            },
            path,
        };
        corpus.seed();
        corpus
    }

    fn add(&self, project: &str, kind: &str, title: &str, body: &str, tags: &[&str]) -> String {
        let project_id = store::ensure_project(&self.conn, &self.an, &self.meta, project).unwrap();
        let tags: Vec<String> = tags.iter().map(|t| t.to_string()).collect();
        store::insert_node(
            &self.conn,
            &self.an,
            &self.meta,
            &NewNode {
                kind,
                title,
                body,
                status: (kind == "todo").then_some("open"),
                project_id: Some(&project_id),
                tags: &tags,
                evidence: None,
                refs: &[],
            },
        )
        .unwrap()
    }

    /// The nodes with a known best query, plus filler so that ranking has
    /// something to rank against.
    fn seed(&self) {
        self.add(
            "infra",
            "pattern",
            "Terraform AWS module layout",
            "One module per service under modules/. Remote state in S3 with a DynamoDB lock table. \
             The aws_provider block pins the region and assumes a deploy role.",
            &["terraform", "aws"],
        );
        self.add(
            "infra",
            "decision",
            "Pin provider versions in every module",
            "We were bitten by a silent provider upgrade. Terraform providers are pinned with ~>.",
            &["terraform"],
        );
        self.add(
            "infra",
            "todo",
            "Move terraform state bucket to the shared account",
            "",
            &["terraform", "aws"],
        );
        self.add(
            "infra",
            "fact",
            "AWS account aliases",
            "prod is acme-prod, staging is acme-stg. Never store the account keys here.",
            &["aws"],
        );
        self.add(
            "platform",
            "pattern",
            "Kubernetes deployment manifests",
            "Each service has a Deployment, a Service and an HPA. Rollouts are configured with maxSurge 1.",
            &["kubernetes"],
        );
        self.add(
            "platform",
            "note",
            "Meeting notes about hiring",
            "We talked about terraform once, in passing, and then about interviews.",
            &[],
        );
        self.add(
            "billing",
            "decision",
            "Use Postgres for the ledger",
            "Strong consistency matters more than write throughput for ledger entries.",
            &["postgres", "database"],
        );
        self.add(
            "billing",
            "pattern",
            "Database migrations",
            "Migrations run before the deploy step. Every migration is reversible.",
            &["postgres"],
        );
        self.add(
            "web",
            "pattern",
            "Configuring the build pipeline",
            "The pipeline lints, tests, then builds a container image.",
            &["ci"],
        );
        self.add(
            "web",
            "fact",
            "Feature flags live in LaunchDarkly",
            "Flag keys use the form team.feature.",
            &["flags"],
        );

        let topics = [
            ("logging", "Structured logs go to stdout as JSON lines"),
            (
                "metrics",
                "Counters and histograms are exported for Prometheus",
            ),
            ("tracing", "Spans are sampled at one percent in production"),
            ("caching", "Redis holds session data with a one hour expiry"),
            (
                "queues",
                "Background jobs are retried five times with backoff",
            ),
            (
                "email",
                "Transactional mail is sent through a single provider",
            ),
            ("search", "The product index is rebuilt every night"),
            ("payments", "Card data never touches our servers"),
            ("onboarding", "New engineers get a laptop image and a buddy"),
            ("releases", "Release branches are cut every second Tuesday"),
        ];
        let kinds = ["pattern", "decision", "fact", "note"];
        for (i, (topic, sentence)) in topics.iter().enumerate() {
            for (j, kind) in kinds.iter().enumerate() {
                let project = ["web", "billing", "platform"][(i + j) % 3];
                self.add(
                    project,
                    kind,
                    &format!("{topic} {kind} number {j}"),
                    sentence,
                    &[topic],
                );
            }
        }
    }

    fn search(&self, terms: &str, current: Option<&str>, kind: Option<&str>) -> Vec<String> {
        let current = current.map(|name| store::find_project(&self.conn, name).unwrap().unwrap());
        search::recall(
            &self.conn,
            &self.an,
            terms,
            &Options {
                kind,
                only_project: None,
                current_project: current.as_deref(),
                limit: 5,
                hops: 0,
            },
        )
        .unwrap()
        .hits
        .into_iter()
        .map(|hit| {
            // SON_RANK_DEBUG=1 cargo test --test ranking -- --nocapture
            if std::env::var_os("SON_RANK_DEBUG").is_some() {
                eprintln!("{terms:>24} | {:8.3} | {}", hit.score, hit.node.title);
            }
            hit.node.title
        })
        .collect()
    }

    fn top(&self, terms: &str) -> String {
        self.search(terms, None, None)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("no hit for '{terms}'"))
    }
}

#[test]
fn corpus_is_the_size_the_plan_asks_for() {
    let corpus = Corpus::new("size");
    let count: i64 = corpus
        .conn
        .query_row(
            "SELECT count(*) FROM nodes WHERE type != 'collection'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 50);
}

#[test]
fn the_canonical_query_finds_how_terraform_is_set_up() {
    let corpus = Corpus::new("canonical");
    assert_eq!(corpus.top("terraform aws"), "Terraform AWS module layout");
}

#[test]
fn a_title_match_outranks_a_passing_mention_in_a_body() {
    let corpus = Corpus::new("title");
    let hits = corpus.search("terraform", None, None);
    assert!(!hits.is_empty());
    assert_ne!(hits[0], "Meeting notes about hiring");
    let mention = hits.iter().position(|t| t == "Meeting notes about hiring");
    assert!(
        mention.is_none_or(|p| p >= 3),
        "a passing mention ranked too high: {hits:?}"
    );
}

#[test]
fn stemming_joins_word_forms() {
    let corpus = Corpus::new("stem");
    assert_eq!(
        corpus.top("configure builds"),
        "Configuring the build pipeline"
    );
    assert_eq!(corpus.top("migrating databases"), "Database migrations");
}

#[test]
fn synonyms_reach_the_canonical_word() {
    let corpus = Corpus::new("synonym");
    assert_eq!(corpus.top("tf aws"), "Terraform AWS module layout");
    assert_eq!(corpus.top("k8s"), "Kubernetes deployment manifests");
    assert_eq!(corpus.top("pg ledger"), "Use Postgres for the ledger");
}

#[test]
fn camel_case_and_snake_case_find_each_other() {
    let corpus = Corpus::new("camel");
    // The body says `aws_provider`.
    assert_eq!(corpus.top("awsProvider"), "Terraform AWS module layout");
    // The body says `maxSurge`.
    assert_eq!(corpus.top("max_surge"), "Kubernetes deployment manifests");
    assert_eq!(corpus.top("maxsurge"), "Kubernetes deployment manifests");
}

#[test]
fn a_type_word_in_the_query_lifts_that_type() {
    let corpus = Corpus::new("hint");
    assert_eq!(
        corpus.top("terraform decision"),
        "Pin provider versions in every module"
    );
    assert_eq!(
        corpus.top("terraform todos"),
        "Move terraform state bucket to the shared account"
    );
    let only = corpus.search("terraform", None, Some("fact"));
    assert!(only.is_empty(), "no fact mentions terraform: {only:?}");
}

#[test]
fn the_current_project_always_comes_first() {
    let corpus = Corpus::new("current");
    let hits = corpus.search("terraform", Some("platform"), None);
    assert_eq!(hits[0], "Meeting notes about hiring");
    assert!(hits.contains(&"Terraform AWS module layout".to_string()));

    let hits = corpus.search("terraform", Some("billing"), None);
    assert_eq!(hits[0], "Terraform AWS module layout");
}

#[test]
fn newer_nodes_rank_above_older_twins() {
    let corpus = Corpus::new("recency");
    let old = corpus.add("web", "note", "Zebra crossing rules", "identical text", &[]);
    let new = corpus.add("web", "note", "Zebra crossing rules", "identical text", &[]);
    corpus
        .conn
        .execute(
            "UPDATE nodes SET updated_at = '2020-01-01T00:00:00Z' WHERE id = ?1",
            [&old],
        )
        .unwrap();
    let result = search::recall(
        &corpus.conn,
        &corpus.an,
        "zebra crossing",
        &Options {
            kind: None,
            only_project: None,
            current_project: None,
            limit: 5,
            hops: 0,
        },
    )
    .unwrap();
    assert_eq!(result.hits[0].node.id, new);
    assert_eq!(result.hits[1].node.id, old);
}

#[test]
fn a_superseded_decision_ranks_below_its_replacement_but_stays() {
    let corpus = Corpus::new("superseded");
    // The older decision matches the query better on words alone.
    let old = corpus.add(
        "infra",
        "decision",
        "Quokka hosting hosting hosting",
        "quokka hosting on bare metal",
        &["quokka"],
    );
    let new = corpus.add(
        "infra",
        "decision",
        "Quokka hosting moves to the cloud",
        "",
        &[],
    );
    store::link(&corpus.conn, &new, "supersedes", &old).unwrap();

    let result = search::recall(
        &corpus.conn,
        &corpus.an,
        "quokka hosting",
        &Options {
            kind: None,
            only_project: None,
            current_project: None,
            limit: 5,
            hops: 0,
        },
    )
    .unwrap();
    assert_eq!(result.hits[0].node.id, new);
    assert_eq!(result.hits[1].node.id, old);
    assert_eq!(result.hits[1].superseded_by, Some(store::short_id(&new)));
}

#[test]
fn recall_adds_the_rest_of_the_project_around_a_hit() {
    let corpus = Corpus::new("also");
    let result = search::recall(
        &corpus.conn,
        &corpus.an,
        "account aliases",
        &Options {
            kind: None,
            only_project: None,
            current_project: None,
            limit: 1,
            hops: 1,
        },
    )
    .unwrap();
    assert_eq!(result.hits[0].node.title, "AWS account aliases");
    let also: Vec<&str> = result.also.iter().map(|n| n.title.as_str()).collect();
    assert!(also.contains(&"Terraform AWS module layout"));
    assert!(also.contains(&"Pin provider versions in every module"));
    assert!(also.contains(&"Move terraform state bucket to the shared account"));
}

impl Corpus {
    fn recall(&self, terms: &str, limit: usize) -> search::Recall {
        search::recall(
            &self.conn,
            &self.an,
            terms,
            &Options {
                kind: None,
                only_project: None,
                current_project: None,
                limit,
                hops: 0,
            },
        )
        .unwrap()
    }
}

#[test]
fn the_start_of_a_word_finds_the_word() {
    let corpus = Corpus::new("prefix");
    assert_eq!(corpus.top("terra"), "Terraform AWS module layout");
    assert_eq!(corpus.top("kubern"), "Kubernetes deployment manifests");
    assert_eq!(corpus.top("migr"), "Database migrations");
    // Two letters are too few to count as the start of a word.
    assert!(corpus.search("te", None, None).is_empty());
}

#[test]
fn a_mistyped_word_finds_the_word() {
    let corpus = Corpus::new("fuzzy");
    assert_eq!(corpus.top("terrafrom"), "Terraform AWS module layout"); // letters swapped
    assert_eq!(corpus.top("teraform"), "Terraform AWS module layout"); // letter missing
    assert_eq!(corpus.top("kubernettes"), "Kubernetes deployment manifests"); // letter added
    assert_eq!(corpus.top("postgras ledger"), "Use Postgres for the ledger"); // wrong letter
                                                                              // Too far from any stored word.
    assert!(corpus.search("xyzzyplugh", None, None).is_empty());
}

#[test]
fn loose_matches_are_marked_and_come_after_exact_ones() {
    let corpus = Corpus::new("loose-order");
    corpus.add("web", "pattern", "Terraformation of the garden", "", &[]);
    let result = corpus.recall("terra", 50);
    assert!(
        result.hits.iter().all(|hit| hit.close),
        "nothing holds the word 'terra' itself"
    );

    corpus.add("web", "note", "Terra firma", "", &[]);
    let result = corpus.recall("terra", 50);
    assert_eq!(result.hits[0].node.title, "Terra firma");
    assert!(!result.hits[0].close);
    assert!(result.hits[1..].iter().all(|hit| hit.close));
    assert!(result.hits.len() > 1);
}

#[test]
fn enough_exact_matches_means_no_loose_ones() {
    let corpus = Corpus::new("loose-skip");
    corpus.add("web", "note", "Loggingham village fete", "", &[]);
    // Five nodes hold the word itself, so the limit of 3 is met exactly.
    let few = corpus.recall("logging", 3);
    assert!(few.hits.iter().all(|hit| !hit.close));
    // With room to spare, the word that only starts the same way appears, last.
    let many = corpus.recall("logging", 50);
    let last = many.hits.last().unwrap();
    assert_eq!(last.node.title, "Loggingham village fete");
    assert!(last.close);
}

#[test]
fn a_word_the_index_knows_is_not_treated_as_a_typo() {
    let corpus = Corpus::new("known");
    corpus.add("web", "note", "Ledgar the office cat", "", &[]);
    let titles: Vec<String> = corpus
        .recall("ledger", 50)
        .hits
        .into_iter()
        .map(|hit| hit.node.title)
        .collect();
    assert!(titles.contains(&"Use Postgres for the ledger".to_string()));
    assert!(!titles.contains(&"Ledgar the office cat".to_string()));
    // The other way round, the unknown word does reach the known one.
    assert_eq!(corpus.top("ledgor"), "Use Postgres for the ledger");
}
