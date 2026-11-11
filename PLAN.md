# son-of-w: Local Second-Brain Binary for Claude Sessions — Build Plan

Repo: https://github.com/Raxvis/son-of-w

## Goal

A single local binary (`son`) that any Claude Code session or AI agent can call from the shell to store and retrieve shared context: todos, decisions, configuration patterns, project facts. No MCP server, no hooks, no embedded model. Integration happens purely through CLAUDE.md instructions telling Claude when and how to call the binary. All data lives in one local SQLite file.

Example: `son recall terraform aws` returns not generic Terraform knowledge, but how William configures Terraform, the related project nodes, past decisions, and open todos, so a fresh session can reproduce the setup.

## Constraints

* **Pure code:** no LLM and no embedding model. No network calls in version 1. Version 2 adds an optional central store (see Central Store and Sync).
* **Local first:** one SQLite database file, default `~/.son-of-w/son.db`, overridable via `SON_DB` env var. Every command works fully offline against this file. The version 2 central store is optional and only ever adds context; it is never required for a recall to succeed.
* **Shell-invoked:** everything is a subcommand with plain-text/markdown output that Claude reads directly.
* **Fast:** any recall must return in well under 100 ms so Claude calls it freely.
* **No secrets:** passwords, tokens, and keys are never stored. The database is plain text. The CLAUDE.md block carries this rule.
* **macOS only:** one binary, installed through Homebrew or a similar method. No Linux or Windows builds.
* **Single static binary:** Rust, using `rusqlite` with the `bundled` feature so SQLite and FTS5 compile straight into the binary with no system dependency. `clap` for the CLI, `ulid` for ids, `rust-stemmers` for Snowball stemming.

## Data Model

SQLite with three core tables plus an FTS5 virtual table.

### nodes

* `id` (ULID), `type`, `title`, `body` (markdown), `status`, `project_id` (FK to a project node; set on every node except project nodes themselves), `created_at`, `updated_at`, `deleted_at` (nullable, soft delete), `source` (session id or agent name)

**Scope:** every node is stored against exactly one project. There is no separate global store. The global view comes from recall: it searches across all projects, so anything saved in one project is reachable from every other session. The current project's nodes always rank first; other projects' nodes follow. That is how cross-session and cross-project memory exists.

**IDs:** ULIDs are stored in full. Output shows the last 8 characters in lowercase, and every command accepts the full id or any unique run of 4 or more characters from its end. The end is used, not the start, because the start of a ULID is a timestamp: every node made in the same second shares its first 8 characters, while the last 8 are random.

Node types for v1:

* `project` — a codebase, repo, or ongoing effort
* `todo` — actionable item with status `open` / `done` / `dropped`
* `decision` — something chosen, with rationale; may `supersede` an earlier decision
* `pattern` — a reusable how-I-do-it (e.g. "Terraform AWS module layout"), the main answer to "how do we normally set this up"
* `fact` — a stable reference fact (account IDs by alias, paths, conventions). Stored as title/body like every other node; a key/value fact is simply title = key, body = value.
* `note` — anything else

### edges

* `from_id`, `to_id`, `relation`, `created_at`

Relations for v1: `belongs_to`, `relates_to`, `supersedes`, `depends_on`, `derived_from`

### tags

* `node_id`, `tag` — lowercase, free-form; used as both filter and search signal

### nodes_fts (FTS5)

* Indexed columns: `title`, `body`, `tags` (joined), weighted with BM25 column weights (title highest, tags next, body last)
* Tokenizer: `unicode61` with `remove_diacritics`, plus a Rust-side pre-processing step (see Search Pipeline)

### Sync-ready columns (in the schema from M1, unused until version 2)

So that the central store can be added later without a migration of existing data:

* `origin` on nodes and edges — which store the row came from: `local`, or the name of a central store.
* `author` on nodes — the person or agent who wrote it, separate from `source` (the session).
* `visibility` on nodes — `private` (never leaves this machine) or `shared` (may be pushed to the central store). Default `private`.
* `rev` on nodes — a counter increased on every edit, used to detect conflicts.
* Soft deletes (`deleted_at`) act as tombstones, so a delete can be synced.
* ULIDs are already globally unique, so rows from different machines never collide.

## Agreed v1 additions (not yet implemented)

### Evidence categories

Decision confirmed by William on 2026-11-11 for TODO.md V1-02: provenance will distinguish **user statement** (explicitly stated by the user), **observation** (directly observed by an agent), and **inference** (concluded by an agent). Existing memories remain **unknown** unless explicitly classified; migration must not guess their category. These categories describe how knowledge was learned, not guaranteed accuracy. Who may mark a memory verified remains an open decision in TODO.md.

## Project Identity

`son` resolves the current project in this order, first match wins:

1. A `.son-project` file in the current directory or any parent (contains the project name).
2. The repo name from the git remote (`origin`).
3. The directory name of the git repository root.
4. The name of the current directory.

`--project <name>` overrides detection on any command. Because the last step always succeeds, there is always a current project. The project node is created automatically the first time something is saved to it.

## Search Pipeline (no model)

Classic Elasticsearch-style analysis, done in Rust before text hits FTS5 and again on queries:

1. Lowercase, Unicode-normalize.
2. Split on word boundaries, punctuation, and camelCase / snake_case (so `awsProvider` and `aws_provider` both yield `aws provider`).
3. Stop-word removal (small English list, keep technical tokens).
4. Snowball/Porter stemming (`configuring` -> `config`).
5. Synonym expansion from a user-editable table in the DB (`tf` -> `terraform`, `k8s` -> `kubernetes`, `pg` -> `postgres`). Applied at query time so the index stays canonical.

Store the analyzed text in a shadow column that FTS5 indexes, keep the original body for display.

**Ranking:** BM25 from FTS5 (title weighted 10, tags 5, body 1), then adjusted in code: a boost for each query word the title or tags cover (BM25 alone lets a one-line todo beat a full pattern), a small head start for patterns and decisions, a recency boost that halves every 90 days, a boost by node type when the query includes a type word such as `decisions` or `todos`, and a penalty for superseded decisions and closed todos. Query expansion beyond synonyms is deliberately left to Claude, which is told in CLAUDE.md to search with several related terms.

**Prefix and fuzzy matching:** recall runs in two passes. The first takes the query's words as given (stemmed, with synonyms). If it finds fewer nodes than the limit, a second pass also takes:

* **Prefixes:** a query word of 3 or more characters matches stored words that start with it, so `terra` finds `terraform`.
* **Typos:** a query word that the index does not hold at all is compared with every stored word. Words within 1 edit (for 4 to 7 characters) or 2 edits (for 8 or more) are searched too, nearest first, at most 3 per query word. An edit is one inserted, deleted, replaced, or swapped letter, so `terrafrom` finds `terraform`. A word the index does hold is never treated as a typo.

Loose matches are marked `close match` (`"match": "close"` in JSON) and are listed after exact matches within the same project band. The stored words come from an `fts5vocab` table over the index (schema version 2).

**Scope of search:** `recall` searches every project by default. Matches from the current project always come first, ranked among themselves; matches from other projects follow, labelled with their project name. If the current project has no match, the other projects' matches are still shown. `--project <name>` restricts results to one project.

**Stale and superseded nodes:** never hidden, only ranked lower. A superseded decision is shown marked as superseded, with a pointer to its replacement. If the newer node is dropped or turns out to be wrong, the older one is still there to fall back on.

**Graph expansion:** after top-k hits (default 5), walk edges one hop (configurable to two) and collect neighbors, deduplicate, and emit hits first then neighbors grouped by relation. This is what turns a keyword hit on `terraform` into the project, its decisions, its patterns, and open todos.

## Central Store and Sync (version 2)

This is not part of version 1. Version 1 is the local binary, milestones M1 to M4. This section exists so that version 1 does not close the door on it.

The enterprise case: a company runs a central data store that provides shared context (company patterns, standards, decisions, facts) alongside each person's local context.

**Model**

* The local database stays the source of truth for the user and the place every recall reads from. Recall stays under 100 ms because it never waits on the network.
* A central store is a remote with a name and a URL, like a git remote: `son remote add acme https://son.acme.example`. More than one can be configured.
* `son sync` pulls central nodes into the local database (marked with their `origin`) and pushes local nodes marked `shared`. Sync is a separate step, run on demand or on a schedule, never inside `recall`.
* Recall then ranks in three bands: current project first, other local projects next, central-store context after, each labelled with where it came from. Central nodes for the current project rank with the current project.
* Nothing leaves the machine by default. A node is pushed only when it is marked `shared`, with `son share <id>` or `--shared` on a write.
* Central nodes are read-only locally. To change one, a user edits it and pushes; the server decides whether to accept.

**API (sketch)**

A small HTTP API with JSON bodies, using the same shapes as `--json` output:

```
GET  /v1/changes?since=<cursor>[&project=<name>]   # pull nodes, edges, tags, tombstones
POST /v1/changes                                   # push shared nodes and edges
GET  /v1/projects                                  # projects this caller may read
```

Authentication is a bearer token held outside the database (macOS Keychain or an env var), in keeping with the no-secrets rule.

**What version 1 must not do**, so that this stays possible: no auto-increment keys, no hard deletes by default, no assumption that every node was written on this machine, and no output shape in `--json` that could not also be an API payload.

## CLI Surface

```
son init [--agent a,b] [--scope global|project] [--yes] [--skill-only] [--db-only] [--list-agents]
son remember <type> "<title>" [--body -] [--project <name>] [--tag t1,t2] [--link <id>:<relation>]
son recall "<query terms>" [--type todo] [--project <name>] [--hops 1] [--limit 5]
son edit <id> [--title "<title>"] [--body -] [--type <type>] [--status <status>] [--project <name>] [--tag t1,t2]
son drop <id>                             # soft delete; --purge removes the row
son restore <id>                          # undo a drop
son todo add "<title>" [--project <name>]
son todo list [--project <name>] [--all] [--done]
son todo done <id>
son link <from_id> <relation> <to_id>
son unlink <from_id> <relation> <to_id>
son show <id>                             # full node plus edges
son projects                              # list project nodes
son synonym add <term> <canonical>
son synonym list
son synonym remove <term>
son export [--project <name>]             # markdown dump; with --json, the backup format
son import <file|->                       # load a JSON export
son reindex                               # rebuild the search index
son list [--project <name>] [--type <type>] [--dropped] [--limit 20] [--page 1]
son log [--project <name>] [--command <name>] [--errors] [--limit 20] [--page 1]
son stats [--days 30 | --all]
son context [--project <name>]            # one-shot bootstrap block for session start
```

Output is compact markdown by default; `--json` on every command for agents that want structure. `--body -` reads body from stdin so Claude can pipe multi-line markdown.

Every node type has full create, read, update, and delete: `remember` / `todo add`, `show` / `recall`, `edit`, `drop`. Every `<id>` accepts a unique prefix.

Write commands attach the new node to the current project unless `--project` names another. `todo list` shows the current project's open todos; `--all` shows every project's, and `--done` includes done and dropped ones.

`export` prints markdown for reading. `export --json` is the backup format: `import` reads it back, replaces a node only when the incoming copy is newer, and merges a project into a local project of the same name.

## Activity and Stats

Every command is written to a local activity log (the `events` table, schema version 3): the time, the command, whether it reads or writes, the project, the session (`SON_SOURCE`), the node it acted on, the query or title, the number of results, how many of those were close matches, how long it took, and the error if it failed. `son list`, `son log`, `son stats` and `son init` are not logged. The log is never exported and will not be synced. `SON_NO_LOG=1` turns it off.

* `son list` — nodes, most recently changed first, with what last happened to each (created, updated, dropped). Pageable, with filters for project and type.
* `son log` — the activity log, newest first. Pageable, with filters for project, command, and failures.
* `son stats` — what is stored now (nodes by type, todos, links, tags, database size, whether the CLAUDE.md block is installed) and how `son` was used over the last 30 days or all time: commands by day, command and project, the share of recalls that found something, the queries that found nothing, how often a close match was needed, writes for each recall, and median and 95th-percentile recall time. It ends with plain remarks when a number looks wrong, for example "sessions read but never write back".

These are the two numbers from Testing the Hypothesis: how often `recall` is called, and how often a session writes back.

Timestamps are UTC to the millisecond, so that two changes in the same second keep their order.

## Agents and `son init`

`son` serves any AI agent that can run a shell command, not only Claude Code. The binary does nothing unless an agent is told to use it, so `son init` sets that up for each agent the user picks.

**The agent table.** [src/agents.rs](src/agents.rs) lists 73 agents. For each one it holds the skills folder inside a project, the skills folder for all projects, and the folder whose presence shows the agent is installed. These follow the table in [vercel-labs/skills](https://github.com/vercel-labs/skills) (`src/agents.ts`), so a `son` skill lands where `npx skills add` would put one. Many agents (Codex, Cursor, Gemini CLI, Antigravity, Copilot, OpenCode and others) share `.agents/skills` inside a project.

**Two questions.** In a terminal, `son init` asks which agents to set up (detected ones are preselected) and where: global, for every project on the machine, or the current project folder (the git root). `--agent`, `--scope` and `--yes` answer the questions in advance. Without a terminal and without those flags it changes nothing and says what to pass.

**Two files for each agent.**

* The **skill**: `son/SKILL.md` in the agent's skills folder. It has the standard `name` and `description` front matter, and the agent loads it when the description fits the task. The whole file belongs to `son` and is replaced on a rerun.
* The **block**: a short section between marker comments in the agent's always-on instruction file, so the agent calls `son` without being asked. A rerun replaces the block in place; text outside the markers is kept.

| Scope | Claude Code | Gemini CLI | Codex | Antigravity | OpenCode | Every other agent |
| --- | --- | --- | --- | --- | --- | --- |
| Project | `CLAUDE.md` | `GEMINI.md` | `AGENTS.md` | `AGENTS.md` | `AGENTS.md` | `AGENTS.md` |
| Global | `~/.claude/CLAUDE.md` | `~/.gemini/GEMINI.md` | `~/.codex/AGENTS.md` | `~/.gemini/GEMINI.md` | `~/.config/opencode/AGENTS.md` | none: skill only |

Agents such as Cursor keep their global rules in settings, not in a file, so globally they get the skill only. The skill alone is weaker than the block: it is loaded when the agent judges it relevant, not at every session start.

The instruction-file paths, unlike the skills folders, are not from vercel-labs/skills. Claude Code, Codex, Gemini CLI and OpenCode are well established; the Antigravity global file and the use of `AGENTS.md` by each of the less common agents should be checked against that agent's own documentation.

## The Protocol

**Relation to Claude's own memory:** `son` is the main store for project context and for anything the user hands off to be kept ("I need to do this", "remember this", "handle this later"). Each item is saved against the current project and is reachable from every other project through recall. It takes precedence over Claude's built-in memory for those. It does not remove the built-in memory: Claude may still keep its own notes there for things specific to one user and one session's way of working.

The block, roughly:

* At session start, run `son context --project <name>` where the project name matches the repo directory name, and treat the output as prior context.
* Before any non-trivial task, run `son recall` with three to five related terms, including tool names, cloud providers, and the project name.
* Whenever a decision is made, a configuration approach is settled, or a todo is created or completed, record it immediately with `son remember` or `son todo`, linking it to the project.
* Prefer `pattern` nodes for anything that answers "how do we normally set this up".
* Never duplicate: if `recall` already shows an equivalent node, update it with `son edit` or link to it rather than creating a new one.
* When the user says "remember this" or "I need to do this", save it with `son`. It is stored against the current project and found from any project.
* Never store secrets: no passwords, tokens, API keys, or private keys. Store where a secret lives, not the secret.

Per-repo CLAUDE.md files can add the project name explicitly and a short list of tags to always search.

## Milestones

**Status (2026-11-11):** M1 to M4 are built and tested, except the Homebrew formula, which needs a first release to point at. Version 1 is otherwise complete.

### M1 — Skeleton (first session)

* Cargo project, `clap` CLI, `rusqlite` open/migrate with WAL mode, `init`, `remember`, `show`, `edit`, `drop`, `todo add/list/done`.
* Project detection, automatic project nodes, `--project`, short-id prefixes.
* Plain FTS5 search with `unicode61` only, no analyzer yet.
* Manual smoke test from a Claude Code session with a minimal CLAUDE.md block.

### M2 — Search quality

* Rust analyzer: camel/snake splitting, stop words, Snowball stemming via rust-stemmers, synonym table.
* BM25 weights, recency boost, `--type` and `--project` filters.
* Test corpus of 50 or so seeded nodes to compare ranking before and after.

### M3 — Graph

* `link`, typed edges, one- and two-hop expansion in `recall`, `supersedes` handling so old decisions are shown as superseded.
* `context` command that assembles the project bootstrap block.

### M4 — Agent ergonomics

* `son init` sets up the chosen agents: a skill and an instruction block for each (idempotent).
* Homebrew formula or tap for install on macOS (not done yet; GitHub Actions builds release binaries for both Mac architectures when a `v*` tag is pushed).
* `--json` everywhere, stdin bodies, `export`/import, `SON_DB` override, concurrency safety (WAL mode, busy timeout) so parallel sessions don't collide.
* Tighten CLAUDE.md wording based on what Claude actually does in practice.

Version 1 ends at M4.

### Version 2 — Central store and sync

* `son remote add/list/remove`, `son share <id>` / `son unshare <id>`, `son sync`.
* HTTP client for the sync API, token read from the Keychain or an env var.
* Three-band ranking in `recall` with origin labels.
* A reference server (same schema, same analyzer) that a company can run.

## Testing the Hypothesis

The real test is behavioural, not unit tests: open two separate Claude Code sessions in different repos, have one record a Terraform pattern and a couple of decisions, then ask the other to set up Terraform for AWS and observe whether it recalls and follows the stored pattern without prompting beyond the CLAUDE.md rules. Track two numbers: how often Claude calls `recall` before acting, and how often it writes back after a decision. Those drive the CLAUDE.md iteration in M4.

## Kicking Off the Claude Code Session

Start local rather than cloud: the deliverable is a native binary you run on your own machine, and a local Claude Code session can `cargo build --release` and let you test `./target/release/son` immediately. If you use a cloud or remote session instead, have it push to a git repo and pull locally to build, or add a GitHub Actions workflow that builds release binaries for your OS and architecture.

Setup before the first prompt:

1. Install Rust via rustup; confirm `cargo --version`.
2. `mkdir son-of-w && cd son-of-w && git init && git remote add origin https://github.com/Raxvis/son-of-w.git`.
3. Drop this plan in the repo as `PLAN.md`.
4. Create a `CLAUDE.md` in the repo that says: this is a Rust CLI, follow `PLAN.md`, work milestone by milestone, write tests for the analyzer and search ranking, and run `cargo clippy` and `cargo test` before declaring a step done.

Opening prompt for the session, roughly:

> Read PLAN.md. Implement Milestone 1 only: Cargo project with clap and rusqlite (bundled), the schema from the Data Model section, and the init, remember, show, and todo commands with plain FTS5 search. Stop and let me test before starting M2.

First local test loop once M1 builds: `son init`, then `son remember pattern "Terraform AWS layout" --tag terraform,aws`, then `son recall terraform` from a different directory. Then wire the global CLAUDE.md block from the protocol section and open a second Claude Code session in an unrelated repo to see whether it calls `recall` on its own.

## Design Decisions and Rationale (from the planning conversation)

* **Why not MCP or hooks:** the tool should work from any session or agent with nothing more than CLAUDE.md instructions and a shell. Zero integration surface beyond a binary on PATH.
* **Why no embedding model:** the binary must be pure code, no LLM or ML model bundled, no network. Semantic matching is replaced by a classic analyzer (tokenize, split camel/snake, stem, stop words, synonyms) plus BM25, the way Elasticsearch worked before vector search. The model already exists in the calling session, so Claude does query expansion with several related terms instead of the binary doing it.
* **Why a graph on SQLite rather than a graph database:** one file, zero ops, FTS5 built in. Typed edges with one- or two-hop expansion give the context neighbourhood that vectors would otherwise provide.
* **Why Rust over Go:** performance is a wash since SQLite does the hot work either way. Rust was chosen because rusqlite's bundled feature ships FTS5 with no cgo or cross-compile hassle, and as a chance to learn Rust (Go is the more familiar language).
* **Why local first with an optional central store:** a company can provide shared context from a central data store, but recall must stay fast and work offline. So sync copies central context into the local file ahead of time, and recall only ever reads locally.
* **Canonical example:** querying "terraform aws" must return how William personally configures Terraform, the related project, decisions, and open todos, so a fresh session reproduces the setup exactly.
* **The real success metric is behavioural:** does Claude call `recall` before acting and write back after decisions, across two independent sessions.

To bootstrap a Claude Code session with full context, save this doc as `PLAN.md` in the repo and reference it from `CLAUDE.md`.

## Open Questions

All about the version 2 central store; none of these block version 1:

* **Conflicts:** when two people edit the same shared node, does the latest edit win, or is the second push rejected until the user pulls?
* **Permissions:** who may write to the central store — everyone, or only named maintainers, with others read-only? Is access per project?
* **Default visibility:** nodes are `private` unless shared (current draft). Should a company be able to make a project `shared` by default?
* **Project matching:** local project names come from the directory or git remote. The central store needs one stable name per project; the git remote URL is the likely key.
* **Server:** what the reference server is built on (SQLite behind a small Rust service, or Postgres), and whether it is self-hosted only.
* **Personal multi-machine sync:** the same mechanism can sync one person's machines, or `export` markdown in git is enough.
