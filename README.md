# son-of-w

`son` is a local second brain for AI coding agents: Claude Code, Codex, Cursor, Antigravity, Gemini CLI and about 70 more. It is one binary and one SQLite file. A session calls it from the shell to save and find decisions, patterns, facts, notes and todos, so that a new session in any project can pick up what earlier sessions learned.

There is no server, no MCP, no hook and no model. An agent uses it because `son init` gives the agent a skill and a short block in its instruction file that tell it to.

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

Or build it from source with Rust. From a clone of this repository:

```bash
make install
```

That puts `son` in `~/.cargo/bin`. To choose another place, give a prefix (the binary goes in its `bin` folder):

```bash
make install PREFIX=/usr/local
```

Without a clone:

```bash
cargo install --git https://github.com/Raxvis/son-of-w
```

Then run:

```bash
son init
```

`son init` creates the database at `~/.son-of-w/son.db`, then asks two questions:

1. **Which agents?** The agents found on your machine are already selected.
2. **Where?** Global (every project on this machine) or this project folder only.

For each agent it writes two things:

- **A skill**, `son/SKILL.md`, in that agent's skills folder: for example `~/.claude/skills`, `~/.cursor/skills`, or the shared `.agents/skills` in a project. These are the same folders `npx skills add` uses.
- **A block** in the agent's always-on instruction file, so the agent uses `son` without being asked: `CLAUDE.md` for Claude Code, `GEMINI.md` for Gemini CLI, `AGENTS.md` for the rest. Text outside the block is kept. Globally, only Claude Code, Codex, Gemini CLI, Antigravity and OpenCode have such a file; other agents get the skill only.

Running it again updates these files in place and never adds them twice.

To skip the questions (an agent or a script has to):

```bash
son init --agent claude-code,codex,cursor --scope global
```

```bash
son init --yes
```

`--yes` takes the detected agents and the global scope. `--scope project` installs into the current project (the git root). Other flags: `--list-agents`, `--skill-only`, `--db-only`, `--print-skill`, `--print-block`.

## How it works

- **Every node belongs to a project.** The project is found from a `.son-project` file in the directory or a parent, else the git remote's repo name, else the git root's directory name, else the current directory's name. `--project <name>` overrides it.
- **Recall searches every project.** Matches from the current project come first; matches from other projects follow. This is how memory crosses sessions and projects.
- **Search uses no model.** Text is split (including `camelCase` and `snake_case`), stop words are removed, words are stemmed, and synonyms such as `tf` → `terraform` are applied to the query. Ranking is BM25 with adjustments for title and tag matches, node type, recency and superseded decisions.
- **Near misses still match.** When the exact words find too few nodes, recall also takes the start of a word (`terra` finds `terraform`) and typos (`terrafrom` finds `terraform`). These are marked `close match` and listed after exact matches.
- **Links connect nodes.** `son link <from> <relation> <to>` with `relates_to`, `supersedes`, `depends_on`, `derived_from` or `belongs_to`. Recall follows links one hop (or two) from its matches. A superseded decision is kept and ranked lower.
- **Nothing is lost by default.** `son drop` is a soft delete; `son restore` brings a node back.

## Commands

```
son init [--agent a,b] [--scope global|project] [--yes]   create the database, set up agents
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
son list [--project <name>] [--type <type>] [--dropped] [--limit 20] [--page 1]
son log [--project <name>] [--command <name>] [--errors] [--limit 20] [--page 1]
son stats [--days 30 | --all]
```

`son list` shows nodes, most recently changed first. `son log` shows every command that was run: what was recalled, how many results it gave, what was saved, and what failed. `son stats` shows how much is stored and how `son` is being used: commands per day, how often a recall finds something, the queries that found nothing, writes for each recall, and recall speed. The log is local; set `SON_NO_LOG=1` to turn it off, and set `SON_SOURCE` to a session name to have it recorded.

Types are `project`, `todo`, `decision`, `pattern`, `fact` and `note`. Every command takes `--json`. `--body -` reads the body from stdin. An `<id>` is the 8-character id that `son` prints.

Set `SON_DB` to use a different database file.

Do not store secrets. The database is plain text.

## Development

```bash
make check
```

`make check` runs what CI runs: the format check, clippy, and all tests. `make` on its own lists every target: `build`, `release`, `test`, `fmt`, `lint`, `check`, `install`, `uninstall`, `clean`.

[PLAN.md](PLAN.md) holds the design and the plan for version 2 (an optional central store with sync).
