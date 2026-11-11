use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "son",
    version,
    about = "Local second brain for Claude sessions",
    long_about = "son stores decisions, patterns, facts, notes and todos in one local SQLite file \
and finds them again from anywhere. Every node belongs to a collection: a code repository, \
or any other group such as \"household\". Recall searches all collections and lists the \
current collection's matches first."
)]
pub struct Cli {
    /// Print JSON instead of markdown
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum NodeType {
    Collection,
    Todo,
    Decision,
    Pattern,
    Fact,
    Note,
}

impl NodeType {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeType::Collection => "collection",
            NodeType::Todo => "todo",
            NodeType::Decision => "decision",
            NodeType::Pattern => "pattern",
            NodeType::Fact => "fact",
            NodeType::Note => "note",
        }
    }
}

/// How a piece of knowledge was learned.
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Evidence {
    /// The user said it
    #[value(name = "user-statement", alias = "user_statement", alias = "user")]
    UserStatement,
    /// The agent saw it: in a file, in command output, on a page
    Observation,
    /// The agent concluded it from other things
    Inference,
    /// Not known (what every node is until it is classified)
    Unknown,
}

impl Evidence {
    /// The stored value; `None` for unknown.
    pub fn stored(self) -> Option<&'static str> {
        match self {
            Evidence::UserStatement => Some("user_statement"),
            Evidence::Observation => Some("observation"),
            Evidence::Inference => Some("inference"),
            Evidence::Unknown => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ScopeArg {
    Global,
    Project,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Relation {
    #[value(name = "belongs_to", alias = "belongs-to")]
    BelongsTo,
    #[value(name = "relates_to", alias = "relates-to")]
    RelatesTo,
    #[value(name = "supersedes")]
    Supersedes,
    #[value(name = "depends_on", alias = "depends-on")]
    DependsOn,
    #[value(name = "derived_from", alias = "derived-from")]
    DerivedFrom,
}

impl Relation {
    pub fn as_str(self) -> &'static str {
        match self {
            Relation::BelongsTo => "belongs_to",
            Relation::RelatesTo => "relates_to",
            Relation::Supersedes => "supersedes",
            Relation::DependsOn => "depends_on",
            Relation::DerivedFrom => "derived_from",
        }
    }
}

#[derive(Subcommand)]
pub enum Command {
    /// Create the database and set son up for your AI agents
    ///
    /// In a terminal this asks which agents to set up and whether to do it
    /// for every project (global) or only this one. Give --agent and --scope,
    /// or --yes, to skip the questions.
    Init {
        /// Agents to set up, by name (comma-separated). See --list-agents
        #[arg(long, short = 'a', value_delimiter = ',')]
        agent: Vec<String>,
        /// Set up for every project (global) or only this project folder
        #[arg(long, value_enum)]
        scope: Option<ScopeArg>,
        /// The same as --scope global
        #[arg(long, short = 'g', conflicts_with = "scope")]
        global: bool,
        /// Do not ask: use the detected agents and the global scope unless given
        #[arg(long, short = 'y')]
        yes: bool,
        /// Write the skill only, and leave instruction files alone
        #[arg(long)]
        skill_only: bool,
        /// Only create the database
        #[arg(long)]
        db_only: bool,
        /// List the agents son knows, and which are detected here
        #[arg(long)]
        list_agents: bool,
        /// Only print the instruction block
        #[arg(long)]
        print_block: bool,
        /// Only print the skill file
        #[arg(long)]
        print_skill: bool,
    },
    /// Save a node: decision, pattern, fact, note, todo or collection
    Remember {
        #[arg(value_enum)]
        kind: NodeType,
        title: String,
        /// Body text, or "-" to read it from stdin
        #[arg(long)]
        body: Option<String>,
        /// Collection to save to (default: the current collection)
        #[arg(long)]
        collection: Option<String>,
        /// Comma-separated tags
        #[arg(long, value_delimiter = ',')]
        tag: Vec<String>,
        /// Link the new node to another: <id>:<relation> (may repeat)
        #[arg(long, value_name = "ID:RELATION")]
        link: Vec<String>,
        /// How this was learned
        #[arg(long, value_enum)]
        evidence: Option<Evidence>,
        /// Where the evidence is: file:<path>, url:<address>,
        /// conversation:<id>, user:<what they said>, other:<text> (may repeat)
        #[arg(long = "ref", value_name = "KIND:VALUE")]
        refs: Vec<String>,
        /// Save nothing: only list stored nodes that this may duplicate
        #[arg(long)]
        check: bool,
    },
    /// Search all collections; the current collection's matches come first
    Recall {
        #[arg(required = true)]
        query: Vec<String>,
        /// Only this node type
        #[arg(long = "type", value_enum)]
        kind: Option<NodeType>,
        /// Only this collection
        #[arg(long)]
        collection: Option<String>,
        /// How many matches to show
        #[arg(long, default_value_t = 5)]
        limit: usize,
        /// How far to follow links from the matches (0 to 2)
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(0..=2))]
        hops: u8,
    },
    /// Print one node in full, with its links
    Show { id: String },
    /// Change a node
    Edit {
        id: String,
        #[arg(long)]
        title: Option<String>,
        /// Body text, or "-" to read it from stdin
        #[arg(long)]
        body: Option<String>,
        #[arg(long = "type", value_enum)]
        kind: Option<NodeType>,
        #[arg(long)]
        status: Option<String>,
        /// Move the node to this collection
        #[arg(long)]
        collection: Option<String>,
        /// Replace the tags (comma-separated; "" clears them)
        #[arg(long, value_delimiter = ',')]
        tag: Option<Vec<String>>,
        /// How this was learned
        #[arg(long, value_enum)]
        evidence: Option<Evidence>,
        /// Add a reference to evidence: <kind>:<value> (may repeat)
        #[arg(long = "ref", value_name = "KIND:VALUE")]
        refs: Vec<String>,
        /// Remove a reference to evidence: <kind>:<value> (may repeat)
        #[arg(long, value_name = "KIND:VALUE")]
        unref: Vec<String>,
    },
    /// Record that you checked a node against its evidence
    ///
    /// Say what you checked with --checked. Reading a node again, or
    /// editing it, is not a check.
    Verify {
        id: String,
        /// What you checked, for example "read Cargo.toml: version is 0.2.0"
        #[arg(long, required_unless_present = "clear")]
        checked: Option<String>,
        /// Add a reference to the evidence you checked (may repeat)
        #[arg(long = "ref", value_name = "KIND:VALUE")]
        refs: Vec<String>,
        /// Take the verification away
        #[arg(long, conflicts_with_all = ["checked", "refs"])]
        clear: bool,
    },
    /// Delete a node (it can be restored unless --purge is given)
    Drop {
        id: String,
        /// Remove the row for good
        #[arg(long)]
        purge: bool,
    },
    /// Undo a drop
    Restore { id: String },
    /// Merge one node into another: <source> is dropped, <survivor> stays
    ///
    /// The survivor gets the tags, the evidence references and the links of
    /// both. When the two bodies differ you must give the merged text with
    /// --body: son never picks one or joins them. `son undo <survivor>`
    /// reverses the merge.
    Merge {
        /// The node that goes away (it is dropped, and points to the survivor)
        source: String,
        /// The node that stays
        survivor: String,
        /// The body of the merged node, or "-" to read it from stdin.
        /// Needed when the two bodies differ
        #[arg(long)]
        body: Option<String>,
        /// The title of the merged node (default: the survivor's)
        #[arg(long)]
        title: Option<String>,
        /// Allow a merge of nodes from two collections; the survivor's
        /// collection is kept
        #[arg(long)]
        cross_collection: bool,
    },
    /// Show every change to a node and its links, newest first
    History {
        id: String,
        /// Show one change in full: the node before it and after it
        #[arg(long, value_name = "NUMBER")]
        op: Option<i64>,
        /// Changes on one page
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=500))]
        limit: u32,
        /// Which page to show, from 1
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
        page: u32,
    },
    /// Reverse the last change to a node (an edit, a drop, a link, a merge)
    ///
    /// The whole change is reversed: every node and link it touched. If
    /// one of them changed again since, nothing is undone and the conflict
    /// is reported. The undo is itself a change, listed by `son history`.
    Undo {
        id: String,
        /// Reverse this change (a number from `son history`), not the last
        #[arg(long, value_name = "NUMBER")]
        op: Option<i64>,
    },
    /// Manage todos
    Todo {
        #[command(subcommand)]
        command: TodoCommand,
    },
    /// Connect two nodes: <from> <relation> <to>
    Link {
        from: String,
        #[arg(value_enum)]
        relation: Relation,
        to: String,
    },
    /// Remove a connection between two nodes
    Unlink {
        from: String,
        #[arg(value_enum)]
        relation: Relation,
        to: String,
    },
    /// List collections: the groups that nodes belong to
    Collections,
    /// Manage query synonyms (tf -> terraform)
    Synonym {
        #[command(subcommand)]
        command: SynonymCommand,
    },
    /// Dump nodes as markdown, or as JSON with --json (the backup format)
    Export {
        /// Only this collection
        #[arg(long)]
        collection: Option<String>,
    },
    /// Load a JSON export ("-" reads stdin)
    Import { file: String },
    /// Print what is stored for a collection, for the start of a session
    Context {
        /// Collection to show (default: the current collection)
        #[arg(long)]
        collection: Option<String>,
    },
    /// Rebuild the search index
    Reindex,
    /// List nodes, most recently changed first
    List {
        /// Only this collection (default: every collection)
        #[arg(long)]
        collection: Option<String>,
        /// Only this node type
        #[arg(long = "type", value_enum)]
        kind: Option<NodeType>,
        /// Include dropped nodes
        #[arg(long)]
        dropped: bool,
        /// Nodes on one page
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=500))]
        limit: u32,
        /// Which page to show, from 1
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
        page: u32,
    },
    /// Show the commands that were run, newest first
    Log {
        /// Only commands run for this collection
        #[arg(long)]
        collection: Option<String>,
        /// Only this command, for example recall or "todo add"
        #[arg(long)]
        command: Option<String>,
        /// Only commands that failed
        #[arg(long)]
        errors: bool,
        /// Commands on one page
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=500))]
        limit: u32,
        /// Which page to show, from 1
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
        page: u32,
    },
    /// Show how much is stored and how son is being used
    Stats {
        /// How many days of activity to count
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u32).range(1..))]
        days: u32,
        /// Count all activity, not only the last days
        #[arg(long, conflicts_with = "days")]
        all: bool,
    },
}

#[derive(Subcommand)]
pub enum TodoCommand {
    /// Add an open todo
    Add {
        title: String,
        /// Body text, or "-" to read it from stdin
        #[arg(long)]
        body: Option<String>,
        /// Collection to save to (default: the current collection)
        #[arg(long)]
        collection: Option<String>,
        /// Comma-separated tags
        #[arg(long, value_delimiter = ',')]
        tag: Vec<String>,
        /// How this was learned
        #[arg(long, value_enum)]
        evidence: Option<Evidence>,
        /// Where the evidence is: <kind>:<value> (may repeat)
        #[arg(long = "ref", value_name = "KIND:VALUE")]
        refs: Vec<String>,
        /// Save nothing: only list stored todos that this may duplicate
        #[arg(long)]
        check: bool,
    },
    /// List open todos for the current collection
    List {
        /// Only this collection
        #[arg(long)]
        collection: Option<String>,
        /// Every collection, not only the current one
        #[arg(long)]
        all: bool,
        /// Include done and dropped todos
        #[arg(long)]
        done: bool,
    },
    /// Mark a todo as done
    Done { id: String },
}

#[derive(Subcommand)]
pub enum SynonymCommand {
    /// Make <term> also search for <canonical>
    Add { term: String, canonical: String },
    /// List all synonyms
    List,
    /// Remove a synonym
    Remove { term: String },
}
