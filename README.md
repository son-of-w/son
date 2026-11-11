# son-of-w

`son` is a local second brain for Claude Code sessions and other AI agents. It is one binary and one SQLite file. A session calls it from the shell to save and find decisions, patterns, facts, notes and todos, so that a new session in any project can pick up what earlier sessions learned.

There is no server, no MCP, no hook and no model. Claude uses it because a block in `~/.claude/CLAUDE.md` tells it to.

```
$ son recall terraform aws
# Recall: terraform aws (current project: billing-api)

## `gf3b5fke` [pattern] Terraform AWS layout
project: infra · tags: aws, terraform · updated 2026-11-11

One module per service. Remote state in S3 with a DynamoDB lock.
```

## Install

macOS only. Download the archive for your Mac from the [releases page](https://github.com/Raxvis/son-of-w/releases), unpack it, and put `son` on your `PATH`:

```bash
tar -xzf son-v*-aarch64-apple-darwin.tar.gz && mv son /usr/local/bin/
```

Or build it from source with Rust:

```bash
cargo install --git https://github.com/Raxvis/son-of-w
```

Then run:

```bash
son init
```

`son init` creates the database at `~/.son-of-w/son.db` and adds a block of instructions to `~/.claude/CLAUDE.md`. Running it again updates the block in place. Use `son init --no-claude-md` to leave that file alone, or `son init --print-block` to only print the block.

## How it works

- **Every node belongs to a project.** The project is found from a `.son-project` file in the directory or a parent, else the git remote's repo name, else the git root's directory name, else the current directory's name. `--project <name>` overrides it.
- **Recall searches every project.** Matches from the current project come first; matches from other projects follow. This is how memory crosses sessions and projects.
- **Search uses no model.** Text is split (including `camelCase` and `snake_case`), stop words are removed, words are stemmed, and synonyms such as `tf` → `terraform` are applied to the query. Ranking is BM25 with adjustments for title and tag matches, node type, recency and superseded decisions.
- **Links connect nodes.** `son link <from> <relation> <to>` with `relates_to`, `supersedes`, `depends_on`, `derived_from` or `belongs_to`. Recall follows links one hop (or two) from its matches. A superseded decision is kept and ranked lower.
- **Nothing is lost by default.** `son drop` is a soft delete; `son restore` brings a node back.

## Commands

```
son init                                   create the database, install the CLAUDE.md block
son remember <type> "<title>" [--body <text>|-] [--project <name>] [--tag a,b] [--link <id>:<relation>]
son recall <terms...> [--type <type>] [--project <name>] [--limit 5] [--hops 1]
son show <id>
son edit <id> [--title] [--body] [--type] [--status] [--project] [--tag]
son drop <id> [--purge]
son restore <id>
son todo add "<title>" [--project <name>] [--tag a,b]
son todo list [--project <name>] [--all] [--done]
son todo done <id>
son link <from> <relation> <to>
son unlink <from> <relation> <to>
son projects
son context [--project <name>]             what a project holds, for the start of a session
son synonym add <term> <canonical> | list | remove <term>
son export [--project <name>]              markdown; add --json for the backup format
son import <file|->                        load a JSON export
son reindex
```

Types are `project`, `todo`, `decision`, `pattern`, `fact` and `note`. Every command takes `--json`. `--body -` reads the body from stdin. An `<id>` is the 8-character id that `son` prints.

Set `SON_DB` to use a different database file.

Do not store secrets. The database is plain text.

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

[PLAN.md](PLAN.md) holds the design and the plan for version 2 (an optional central store with sync).
