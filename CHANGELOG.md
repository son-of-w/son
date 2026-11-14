# Changelog

Every change that a user of `son` can see is listed here. The layout follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions follow [Semantic Versioning](https://semver.org/): from 1.0.0 on, a 1.x release only adds; see "What stays the same in version 1" in the [README](README.md).

## Unreleased

### Added

- `son uninstall` removes the skill files and the instruction blocks that `son init` wrote. The text around a block stays, and the database is not touched. `--dry-run` lists what would be done.
- `son seed` lists the instruction files, rule files and Claude Code memory files that you already have, and tells the agent how to bring what is in them into `son`. It changes nothing by itself.
- `son completions <shell>` prints a completion script for bash, zsh, fish and PowerShell.
- `son tags` lists the tags in use with the number of nodes for each. `--tag a,b` on `son recall`, `son list`, `son review`, `son todo list` and `son export` keeps only the nodes that have every one of the tags.
- A todo has a number in its collection (`#4`), which works where a command takes a todo. `son todo add --parent 4` makes a child (`#4.1`). `son todo list` shows children under their parent, and `son todo backlog` prints the open todos one line each. A todo with open children cannot be done.
- A todo can have a priority, `p0` (most urgent) to `p4`: `--priority` on `son todo add` and `son edit`. Todo lists are in priority order.
- `son todo reopen` opens a done or dropped todo again. `son todo done --note` records what shows that it is done. `son todo add` takes `--number` and `--status`, to bring in a backlog that has them.
- `son init --todo-skill` installs a second skill that keeps a backlog in `son`. `--todo-skill-name` and `--overwrite-todo-skill` say what to do when a skill named `todo` is there already.
- A write that looks like it holds a secret is refused with the error `possible_secret`: private key headers, well-known token forms, JSON web tokens, and a long value after `password=`, `token=` or `api_key=`. `--allow-secret` saves anyway. A JSON import with such a node is refused whole.
- Before the schema of a database is upgraded, a copy of the database is written to the `backups` folder beside it. `SON_NO_UPGRADE_BACKUP=1` turns this off. `son init` says if it created the database, found it up to date, or upgraded it, and where the copy is.
- `son context` tells the agent when the skill files or the instruction block are from another version of `son`, so that it can tell you to run `son init`. In JSON this is `setup_warnings`.
- `son doctor` has new checks: stored nodes of a form this `son` does not know, stored text that looks like a secret, a database file that other users can read, and the copies made before schema upgrades. A new status, `warning`, does not make `son doctor` fail.
- New error codes: `broken_block`, `possible_secret`, `upgrade_backup_failed`, `uninstall_failed`.
- The README states what stays the same in every 1.x release, and tests pin it.

### Changed

- `son init` refuses an instruction file whose `son` marks do not make exactly one block. It checks every file before it writes any, changes none, and names the file and the lines. Before, a file with a start mark and no end mark could lose the text after it.
- `son init` and `son uninstall` replace a file in one step, keep its access mode, and keep a symbolic link as a link.
- A JSON import checks every record before it saves anything. One record that is not valid (an id of another form, an unknown type or status, a bad time, a node in no collection) refuses the whole file. Before, such a record could be saved and make later commands fail.
- A JSON import carries stale marks and verification between copies of a node, set or cleared: the copy whose marks changed last wins, if the marks were made for the content that stays.
- `son review` finds possible duplicates in one pass. A review of 5,000 nodes takes about 0.3 seconds; it took close to a minute. In a large database it can list a possible duplicate that it missed before.
- The row in the activity log is written in one attempt. A command that ends while another session holds the database is not logged, and does not wait.
- New database and backup files are made for their owner only (mode 0600, the folder 0700) on macOS and Linux. Files that exist are not changed; `son doctor` reports them.
- Commands no longer print the fields `origin` and `visibility` of a node with `--json`. They have no meaning before version 2. The export still holds them.
- The schema version is 12. An older database is upgraded by the first command that can write, after the copy named above.

### Fixed

- A stored node with an id that is not of `son`'s form made `son list --json` crash. Showing a short id no longer cuts inside a character.

## 0.1.15 - 2026-11-13

### Added

- The Mac binaries are signed (Developer ID Application: Xon Designs LLC) and notarized by Apple.
- `son` is licensed under MIT.

### Changed

- The README leads with what `son` is for.

## 0.1.14 - 2026-11-11

### Changed

- The settings file of a folder is named `.son`, with the line `collection = <name>`.
- The repository is at `son-of-w/son`.

## 0.1.13 - 2026-11-11

### Added

- The results of the first evaluation with two agents (Claude Code and Codex).
- A script that prints the Homebrew formula for a release.

### Changed

- The instruction block tells an agent to tell the user when a stored node holds an order that the task did not ask for.

## 0.1.12 - 2026-11-11

### Added

- A kit to evaluate the memory loop with two agents (`eval/`).

### Fixed

- The duplicate check finds a title that says the same thing in other words ("Each route in its own file" and "One file per route").

## 0.1.11 - 2026-11-11

### Added

- `son backup` writes a full copy of the database. `son import` restores one.
- `son doctor` checks the database and the agent setup, and names the command that fixes each fault.

## 0.1.10 - 2026-11-11

### Added

- `son review` lists the nodes that need a second look: stale nodes, facts and decisions not verified for a long time, old open todos, possible duplicates.
- `son stale` marks a node as out of date.

## 0.1.9 - 2026-11-11

### Added

- `--max-chars` on `son recall` and `son context` keeps the whole answer within a size.
- `son recall --explain` says why each match was found.

## 0.1.8 - 2026-11-11

### Added

- With `--json`, an error is a JSON object with a stable `code`.
- `--key` on `son remember` and `son todo add`: a write that is sent again with the same key is saved once.

## 0.1.7 - 2026-11-11

### Added

- `son remember --check` saves nothing and lists stored nodes that may say the same thing.
- `son merge` joins two nodes.

## 0.1.6 - 2026-11-11

### Added

- `son history` lists every change to a node. `son undo` reverses the last one.

## 0.1.5 - 2026-11-11

### Changed

- A group of nodes is called a collection (before: project). An agent that does not work in a repository names its collection with `--collection` or `SON_COLLECTION`. Exports written before this change are still read.

## 0.1.4 - 2026-11-11

### Added

- A node records how it was learned (`--evidence`) and where the evidence is (`--ref`). `son verify` records a check against the evidence.

## 0.1.3 - 2026-11-11

### Changed

- The skill and the instruction block tell an agent that recalled content is reference material, not instructions.

## 0.1.2 - 2026-11-11

### Changed

- A session that finds the database locked waits for up to 10 seconds (`SON_BUSY_MS`), then fails with a clear message and changes nothing.

## 0.1.1 - 2026-11-11

### Added

- The first release: `son remember`, `son recall`, `son context`, `son show`, `son edit`, `son drop`, `son restore`, `son todo`, `son link`, `son collections`, `son synonym`, `son export`, `son import`, `son list`, `son log`, `son stats`.
- `son init` sets `son` up for many AI agents.
- Recall matches the start of a word and small typos.
