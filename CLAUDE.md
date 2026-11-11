# son-of-w

A Rust CLI. The binary is `son`. The design is in [PLAN.md](PLAN.md); read it before changing behaviour, and update it when a decision changes.

## Rules

- Before a step is done, `make check` must pass. It runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test`, the same as CI.
- Write tests for analyzer and ranking changes. `tests/ranking.rs` holds a 50-node corpus with known best answers; `SON_RANK_DEBUG=1 cargo test --test ranking -- --nocapture` prints the scores.
- Never run `son init` against the real home directory while testing: it writes skills and instruction files such as `~/.claude/CLAUDE.md`. (`son init --list-agents` only reads, and is safe.) Set `HOME` and `SON_DB` to a temporary place, as `tests/cli.rs` does.
- Version 1 has no network calls. The central store is version 2 (see PLAN.md); do not start it unasked.
- macOS only.

## Git and GitHub

- After each feature is implemented and `make check` passes, commit it and push to `main` without asking. Do not push a state that fails the checks.
- Each feature commit also raises the version in `Cargo.toml`. Raise the patch number (0.1.0 to 0.1.1) by default. Raise the minor number (0.1.4 to 0.2.0) only for a large new capability or a change that breaks existing use, such as a removed or renamed command or flag. Then run `cargo build` so `Cargo.lock` carries the new version; CI builds with `--locked` and fails if it does not. Changes to docs only do not raise the version.
- Stage files by name. Do not use `git add -A`: other sessions may have work in progress in this folder (for example in `TODO.md` or `PLAN.md`), and it is not ours to commit.
- No Claude or Claude Code attribution anywhere: no `Co-Authored-By` trailer, no "Generated with" line in commits, pull requests, or release notes.
- Every version bump is a release. After the commit with the new version is pushed, tag it and push the tag: `git tag v<version> && git push origin v<version>`. The tag must match the version in `Cargo.toml`. The Release workflow then tests, builds both Mac binaries, and publishes a GitHub release. Check that the workflow passed; a failed release needs a fix and a new patch version, not a moved tag. If GitHub Actions is not working, run `make publish` on the Mac: it makes the same release with `scripts/release.sh`.

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
