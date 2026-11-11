# son-of-w

A Rust CLI. The binary is `son`. The design is in [PLAN.md](PLAN.md); read it before changing behaviour, and update it when a decision changes.

## Rules

- Before a step is done, all three must pass: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
- Write tests for analyzer and ranking changes. `tests/ranking.rs` holds a 50-node corpus with known best answers; `SON_RANK_DEBUG=1 cargo test --test ranking -- --nocapture` prints the scores.
- Never run `son init` against the real home directory while testing: it writes skills and instruction files such as `~/.claude/CLAUDE.md`. (`son init --list-agents` only reads, and is safe.) Set `HOME` and `SON_DB` to a temporary place, as `tests/cli.rs` does.
- Version 1 has no network calls. The central store is version 2 (see PLAN.md); do not start it unasked.
- macOS only.

## Git and GitHub

- No Claude or Claude Code attribution anywhere: no `Co-Authored-By` trailer, no "Generated with" line in commits, pull requests, or release notes.
- A release is made only when William asks for one. To release: set the version in `Cargo.toml`, commit, then push a tag `v<version>`. The Release workflow builds both Mac binaries and publishes them.

## Layout

- `src/cli.rs` — the command line (clap)
- `src/app.rs` — runs one command
- `src/db.rs` — open, schema, migration
- `src/project.rs` — which project the current directory is
- `src/analyze.rs` — tokenize, split, stem, synonyms
- `src/store.rs` — nodes, tags, edges, synonyms
- `src/search.rs` — recall: ranking and graph expansion
- `src/render.rs` — markdown and JSON output
- `src/activity.rs` — the activity log, paging, and the numbers for `son stats`
- `src/report.rs` — output for `son list`, `son log`, `son stats`
- `src/agents.rs` — the table of AI agents: skills folders, instruction files, detection
- `src/setup.rs` — `son init`: choose agents and scope, then install
- `src/instructions.rs` — the skill file and the instruction block, and how they are written
- `src/transfer.rs` — export and import
