# son-of-w

A Rust CLI. The binary is `son`. Read [README.md](README.md) for current usage and [TODO.md](TODO.md) for the active backlog index. Full context and agreed decisions live in `todos/<number>.md`; completed details move to `todos/archived/<number>.md`. Keep TODO.md to one numbered, prioritized line per active todo, sorted by priority with children beneath their parent. Record decisions in the relevant detail file; do not maintain or recreate the initial launch plan or duplicate the backlog in a separate design plan.

## Rules

- Before a step is done, `make check` must pass. It runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test`, the same as CI.
- Write tests for analyzer and ranking changes. `tests/ranking.rs` holds a 50-node corpus with known best answers; `SON_RANK_DEBUG=1 cargo test --test ranking -- --nocapture` prints the scores.
- Never run `son init` against the real home directory while testing: it writes skills and instruction files such as `~/.claude/CLAUDE.md`. (`son init --list-agents` only reads, and is safe.) Set `HOME` and `SON_DB` to a temporary place, as `tests/cli.rs` does.
- Version 1 has no network calls. The central store is version 2 (see [todo #13](todos/13.md)); do not start it unasked.
- `son` must work on macOS, Linux and Windows. Do not use Unix-only paths or APIs; find the home directory with `agents::home()`. CI runs the checks on all three.

## Git and GitHub

- After each feature is implemented and `make check` passes, commit it and push to `main` without asking. Do not push a state that fails the checks.
- Each feature commit also raises the version in `Cargo.toml`. Raise the patch number (0.1.0 to 0.1.1) by default. Raise the minor number (0.1.4 to 0.2.0) only for a large new capability or a change that breaks existing use, such as a removed or renamed command or flag. Then run `cargo build` so `Cargo.lock` carries the new version; CI builds with `--locked` and fails if it does not. Changes to docs only do not raise the version.
- Stage files by name. Do not use `git add -A`: other sessions may have work in progress in this folder (for example in `TODO.md` or files under `todos/`), and it is not ours to commit.
- No Claude or Claude Code attribution anywhere: no `Co-Authored-By` trailer, no "Generated with" line in commits, pull requests, or release notes.
- Every version bump is a release. After the commit with the new version is pushed, tag it and push the tag: `git tag v<version> && git push origin v<version>`. The tag must match the version in `Cargo.toml`. The Release workflow then tests, builds both Mac binaries, and publishes a GitHub release. Check that the workflow passed; if it failed before anything was made public (the release is still a draft), fix the cause, move the tag to the fix, and run it again. Once a release is public, never move its tag: a fix then needs a new patch version. If GitHub Actions is not working, run `make publish` on the Mac: it uses the same `scripts/release.sh`, but the release then holds only the binaries the Mac can build (no Windows).

## Layout

- `src/cli.rs` — the command line (clap)
- `src/app.rs` — runs one command
- `src/db.rs` — open, schema, migration
- `src/project.rs` — which collection a command works in (`SON_COLLECTION`, a marker file, the repository, the directory)
- `src/analyze.rs` — tokenize, split, stem, synonyms
- `src/store.rs` — nodes, tags, edges, synonyms
- `src/search.rs` — recall: ranking and graph expansion
- `src/render.rs` — markdown and JSON output
- `src/activity.rs` — the activity log, paging, and the numbers for `son stats`
- `src/review.rs` — `son review`: which nodes need a second look, and why
- `src/report.rs` — output for `son list`, `son log`, `son stats`
- `src/error.rs` — error codes, and the JSON error object for `--json`
- `src/history.rs` — change history: what each command changed, and undo
- `src/agents.rs` — the table of AI agents: skills folders, instruction files, detection
- `src/setup.rs` — `son init`: choose agents and scope, then install
- `src/instructions.rs` — the skill file and the instruction block, and how they are written
- `src/transfer.rs` — JSON export and import
- `src/backup.rs` — `son backup`, and restoring a backup through `son import`
- `src/doctor.rs` — `son doctor`: read-only checks of the database and the agent setup

## Backlog implementation

- Todos #1–#11 have settled product decisions; #12 carries forward Homebrew packaging with a potentially deferred publishing destination. Follow the decisions in each detail file without re-asking them.
- Start with concurrency reliability (#1) and provenance (#2). History/undo (#7) is required before completing merges (#4); duplicate preview can proceed independently of history. Review (#3) reuses provenance and duplicate detection. Complete persistent schema changes before integrating full backup/import (#9), then run the cross-agent evaluation (#11). The instruction-safety rule (#10) can be implemented independently.
- Priority does not override dependencies. Preserve local/offline behavior and existing data, with migration/export/import coverage where relevant. The pre-v1 public naming changes in #5 intentionally remove legacy aliases.
