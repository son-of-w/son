use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "son",
    version,
    about = "Local second brain for Claude sessions",
    long_about = "son stores decisions, patterns, facts, notes and todos in one local SQLite file \
and finds them again from any project. Every node belongs to a project; recall searches \
all projects and lists the current project's matches first."
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
    Project,
    Todo,
    Decision,
    Pattern,
    Fact,
    Note,
}

impl NodeType {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeType::Project => "project",
            NodeType::Todo => "todo",
            NodeType::Decision => "decision",
            NodeType::Pattern => "pattern",
            NodeType::Fact => "fact",
            NodeType::Note => "note",
        }
    }
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
    /// Create the database and add the son block to ~/.claude/CLAUDE.md
    Init {
        /// Do not touch CLAUDE.md
        #[arg(long)]
        no_claude_md: bool,
        /// Only print the CLAUDE.md block
        #[arg(long)]
        print_block: bool,
    },
    /// Save a node: decision, pattern, fact, note, todo or project
    Remember {
        #[arg(value_enum)]
        kind: NodeType,
        title: String,
        /// Body text, or "-" to read it from stdin
        #[arg(long)]
        body: Option<String>,
        /// Project to save to (default: the detected project)
        #[arg(long)]
        project: Option<String>,
        /// Comma-separated tags
        #[arg(long, value_delimiter = ',')]
        tag: Vec<String>,
        /// Link the new node to another: <id>:<relation> (may repeat)
        #[arg(long, value_name = "ID:RELATION")]
        link: Vec<String>,
    },
    /// Search all projects; the current project's matches come first
    Recall {
        #[arg(required = true)]
        query: Vec<String>,
        /// Only this node type
        #[arg(long = "type", value_enum)]
        kind: Option<NodeType>,
        /// Only this project
        #[arg(long)]
        project: Option<String>,
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
        /// Move the node to this project
        #[arg(long)]
        project: Option<String>,
        /// Replace the tags (comma-separated; "" clears them)
        #[arg(long, value_delimiter = ',')]
        tag: Option<Vec<String>>,
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
    /// List projects
    Projects,
    /// Manage query synonyms (tf -> terraform)
    Synonym {
        #[command(subcommand)]
        command: SynonymCommand,
    },
    /// Dump nodes as markdown, or as JSON with --json (the backup format)
    Export {
        /// Only this project
        #[arg(long)]
        project: Option<String>,
    },
    /// Load a JSON export ("-" reads stdin)
    Import { file: String },
    /// Print what is stored for a project, for the start of a session
    Context {
        /// Project to show (default: the detected project)
        #[arg(long)]
        project: Option<String>,
    },
    /// Rebuild the search index
    Reindex,
    /// List nodes, most recently changed first
    List {
        /// Only this project (default: every project)
        #[arg(long)]
        project: Option<String>,
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
        /// Only commands run for this project
        #[arg(long)]
        project: Option<String>,
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
        /// Project to save to (default: the detected project)
        #[arg(long)]
        project: Option<String>,
        /// Comma-separated tags
        #[arg(long, value_delimiter = ',')]
        tag: Vec<String>,
    },
    /// List open todos for the current project
    List {
        /// Only this project
        #[arg(long)]
        project: Option<String>,
        /// Every project, not only the current one
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
