# son reference

The full detail of how `son` works. The [README](../README.md) has the short version.

- [How it works](#how-it-works)
- [Commands](#commands)
- [For programs](#for-programs)
- [What stays the same in version 1](#what-stays-the-same-in-version-1)

## How it works

### Collections

- **Every node belongs to a collection.** A collection is a group of nodes: a code repository, or any other subject such as `household` or `travel`. `son collections` lists them.
- **The current collection is found for you.** It is, in this order: `--collection <name>` on the command; the `SON_COLLECTION` environment variable; a `.son` file in the directory or a parent, with the line `collection = <name>`; the git remote's repo name; the git root's directory name; the current directory's name. So a coding agent needs to do nothing, and an agent that does not work in a repository names its collection once and gets the same one from any directory.
- **`.son` is the settings file for a folder.** It holds `key = value` lines, with `#` for comments; `collection` is its only key now.
- **Collections organize; they do not restrict.** Every agent that uses the database can read every collection. A collection is not a security boundary.

### Tags

- **A tag filter narrows a read.** `--tag a,b` on `son recall`, `son list`, `son review`, `son todo list` and `son export` keeps only the nodes that have every one of the tags. It works together with `--collection` and `--type`. The match is exact, on the tag as it is stored (lowercase, no leading `#`): no stemming and no synonyms. In a recall the filter only takes nodes away, from every part of the answer; the order of the rest does not change. A tag that no node has gives an empty answer. An export with a tag filter holds the tagged nodes and the collections they are in.
- **`--tag` has two meanings.** On `son remember`, `son todo add` and `son edit` it sets the tags of the node. On the commands above it filters.
- **`son tags` lists the tags in use**, with the number of nodes for each, the most used first; `--collection` limits it. Look there before you make a new tag, so that one subject does not get two spellings.

### Todos

- **A todo is a node with a status**: `open`, `done` or `dropped`. `son todo done <id>` finishes it, and `son todo reopen <id>` opens it again; both are in the history and can be undone. Everything else is the same as for other nodes: `son edit <id>` changes it and `son drop <id>` removes it.
- **A todo has a number that a person can say**, such as `#4`, beside its id. Either one works where a command takes a todo: `son todo done 4`, `son show '#4'`, `son recall 4`. Numbers count for each collection, start at 1, and are never given twice, also after a todo is purged. Todos from before numbers have none, except that a todo whose title started with a number (`#23 Fix the import`) got it as its number at the upgrade.
- **A todo can be a child of another.** `son todo add "<title>" --parent 4` makes `#4.1`, and links it to `#4` with `belongs_to`; a child of that is `#4.1.1`. `son todo list` shows the children under their parent. A todo with open children cannot be done: `son todo done` refuses it and lists them.
- **`son todo backlog` prints the open todos as a backlog**: one line each with the priority, the number and the title, children under their parent, and the most urgent first inside each level.
- **`son todo done <todo> --note "<text>"`** adds what shows that it is done to the body, with the date.
- **A number counts in one collection.** A todo that moves to another collection (`son edit <todo> --collection <name>`) keeps its number when it is free there; a child, or a todo whose number is in use there, gets the next free number, and the command says so. The same holds for an import: the todo that arrives gets the next free number, its children arrive under the new number, and the import lists each change. A merge keeps the number of the node that stays.
- **To bring in a backlog that has numbers already**, give them: `son todo add "<title>" --number 4.1 --status done --priority p1 --body -`. Add a parent before its children.
- **A todo can have a priority**, `p0` (most urgent) to `p4`: `son todo add "<title>" --priority p1`, and `son edit <id> --priority p2` to change it or `--priority none` to take it away. A priority is not required. Only a todo can have one.
- **The most urgent comes first.** `son todo list` and the open todos of `son context` are in priority order, a todo with no priority after `p4`, and the oldest first inside one priority. `son context` shows at most ten open todos, so these are the ten most urgent. `son review` does not look at priority: it lists a todo that is open for a long time, whatever its priority.

### Recall

- **Recall searches every collection.** Matches from the current collection come first; matches from other collections follow. `--collection <name>` keeps only one. This is how memory crosses sessions and subjects.
- **Search uses no model.** Text is split (including `camelCase` and `snake_case`), stop words are removed, words are stemmed, and synonyms such as `tf` → `terraform` are applied to the query. Ranking is BM25 with adjustments for title and tag matches, node type, recency and superseded decisions.
- **Near misses still match.** When the exact words find too few nodes, recall also takes the start of a word (`terra` finds `terraform`) and typos (`terrafrom` finds `terraform`). These are marked `close match` and listed after exact matches.
- **Links connect nodes.** `son link <from> <relation> <to>` with `relates_to`, `supersedes`, `depends_on`, `derived_from` or `belongs_to`. Recall follows links one hop (or two) from its matches. A superseded decision is kept and ranked lower.
- **Recall can say why.** `son recall ... --explain` adds to each match the query words it met and where (`"tf" in title as "terraform" (synonym)`, and `prefix` or `typo` for close matches), and its score as a product: the text match, then each adjustment that was applied (recency, query words in title and tags, type, type named in the query, superseded, closed todo). It says when a match is listed first because it is in the current collection, and why linked nodes are there. The explanation is recorded by the ranking code as it runs, and `--explain` does not change the order.
- **An answer can be given a size limit.** `son recall ... --max-chars <number>` and `son context --max-chars <number>` keep the whole answer within that many characters: matches, linked nodes, the other nodes of the collections, and with `--json` the whole JSON text. A character is one Unicode code point, and line ends count. To fit, `son` leaves out the extra lists first, then cuts bodies, then lists one line for each match, then fewer matches, then ids only. The answer says what was left out (a last line in markdown; `truncated`, `omitted` and `body_cut` in JSON, which is printed on one line), and every id that is shown works with `son show <id>`. The limit is never passed: if not even the ids fit, the command fails and prints nothing. Without the flag nothing changes: no limit on the whole answer, and each body in a recall is cut at 800 characters as before. `son export` is never cut.

### Evidence

- **Every node can say where it came from.** `--evidence` records how it was learned: `user-statement`, `observation` or `inference`; without it the node is "unknown". `--ref <kind>:<value>` points at the evidence (`file`, `url`, `conversation`, `user`, `other`); `son` stores the pointer and never opens it.
- **A check is recorded, not assumed.** `son verify <id> --checked "<what was checked>"` records an explicit check, with its own date. An edit never marks a node verified. These fields describe recorded evidence; `son` does not test whether a node is true.

### Duplicates and review

- **Duplicates are found before they are saved.** `son remember <type> "<title>" --check` (and `son todo add "<title>" --check`) saves nothing: it lists stored nodes that may say the same thing, with the reason (same title, shared title words, shared body words). It reads only: no node, no collection, no log entry, and no database if there is none yet.
- **Nodes are merged only on request.** `son merge <source> <survivor>` joins two nodes of the same type: the survivor gets the tags, the evidence references and the links of both; the source is dropped and points to the survivor, so its old id still leads somewhere. When the two bodies differ, you must give the merged text with `--body`; `son` never picks one or joins them. A merge across collections needs `--cross-collection` and keeps the survivor's collection. `son undo <survivor>` reverses a merge. Alike words are a hint only: nothing is merged automatically.
- **Old knowledge is brought back for a check, never thrown away.** `son review` lists the nodes that need a second look, each with its reasons: a node someone marked stale; a fact not verified for more than 90 days, or a decision for more than 180 (counted from the last `son verify`, or from creation if it was never verified; an edit does not restart the count); a todo open for more than 30 days; a node that may duplicate an older one. Change the limits with `--fact-days`, `--decision-days` and `--todo-days`, or with `SON_REVIEW_FACT_DAYS`, `SON_REVIEW_DECISION_DAYS` and `SON_REVIEW_TODO_DAYS`. Review only lists: it changes nothing, and age alone never makes a node stale, hidden or deleted. Two nodes listed as possible duplicates leave the list when they are merged or linked.
- **Stale is a label, not a deletion.** `son stale <id> --reason "<why>"` marks a node as out of date. A stale node is kept and still found, with the label `STALE` in recall, context and show (`stale_at` and `stale_note` in JSON); it is not ranked lower. The mark stays through edits, export and import, until `son verify` (the node was checked and is right) or `son stale <id> --clear`.
- **Review stays fast on a large database.** It reads the nodes once and compares only the pairs that share a title word, have the same title, or have the same body. A review of 5,000 nodes takes well under a second. `son remember --check` looks at the 50 best matches of the search index for the one new node; a review looks at every pair, so in a large database it can list a possible duplicate that `--check` does not.

### History and undo

- **Nothing is lost by default.** `son drop` is a soft delete; `son restore` brings a node back.
- **Every change is kept.** `son history <id>` lists each change to a node and its links, newest first, with who made it; `--op <number>` shows the node in full before and after one change. History is never pruned.
- **Every change can be undone.** `son undo <id>` reverses the newest change: an edit, a drop, a restore, a link, a verification, an import. It reverses the whole change (every node and link that change touched) or nothing: if a node or link was changed again since, it reports the conflict and leaves everything as it is. An undo is itself a change in the history, so it can be undone too (`son undo <id> --op <number>`).
- **Purge is the one way to erase.** `son drop <id> --purge` removes the node and its history from the database (not from backups you made before); a change that also touched the purged node can then no longer be undone.

### Backup and export

- **Backups are full, and made only when you ask.** `son backup <file>` writes one SQLite file that holds everything at one moment: nodes, links, evidence, history, the activity log and keys. SQLite makes it from one read of the database, so it is consistent while other sessions write. It never overwrites a file; give a folder and it makes `son-backup-<time>.db` in it. No command makes a backup by itself, with one exception (the next point), and old backups are never removed.
- **A schema upgrade copies the database first.** When a newer `son` meets a database of an older schema, the first command that can write upgrades it; `son init` is the command to run after you install a new version, and it says what it did: `created`, `up to date`, or `upgraded from schema N to M` with the place of the copy. Before the upgrade of a database that holds nodes, `son` writes a full copy of it to the `backups` folder beside the database (`~/.son-of-w/backups/son-before-schema-<old version>-<time>.db`). If the copy cannot be written, nothing is upgraded and the command fails with `upgrade_backup_failed`. `son` never removes these copies; `son doctor` lists them with their sizes, and you can delete them. Put one back with `son import <file> --replace`. `SON_NO_UPGRADE_BACKUP=1` upgrades with no copy.
- **A restore is exact.** `son import <backup file>` is a full restore, not a merge, and afterwards the database holds exactly what the backup holds. Into an empty database it needs nothing more; into a database that holds anything (nodes, history, log entries or keys) it is refused unless you add `--replace`. The file is checked first (it is a `son` database, of a schema this `son` knows, and SQLite finds it whole), the work is one transaction, and a failure leaves the database as it was; the backup file is only read. An older backup is brought to the current schema as it is restored.
- **Two formats, two jobs.** The JSON export (`son export --json`) is for moving knowledge: it is readable, it merges on import, and it carries nodes, links and synonyms, without history, the log or keys. The backup is for getting a database back exactly. `son import` takes both and tells them apart by content; `-` reads a JSON export from stdin.
- **An import merges content and marks apart.** For a node that is here and in the file, the content that was edited last stays. The stale mark and the verification move together as one unit, and the copy whose marks changed last wins: a mark that was set and a mark that was cleared both come across. Marks count only for the text they were made for: a verification from the file that is older than the last edit here is not taken. If both copies changed their marks at the same moment, a stale mark wins. The summary of an import counts the nodes whose marks changed, and `son undo` reverses the import. The times come from the clocks of the two machines; a clock that is wrong can make the wrong copy win. An export from a `son` before this rule has no time for a cleared mark, so from such a file a later mark is taken and a clear is not.
- **A JSON import is checked before it is saved.** Every record is looked at first: the id has the form `son` makes (26 digits and uppercase letters), the type, status, evidence category, reference kinds and relations are ones this `son` knows, the times are times, each node is in a collection that is in the file or stored here, and no id is in the file twice. One fault refuses the whole file with `import_failed`, lists the faults with their place in the file (the first 20, and a count), and changes nothing. A field this `son` does not know is ignored; a value it does not know is refused, and can mean the file is from a newer `son`. A backup restore is an exact copy and is not checked record by record; `son doctor` reports stored nodes of a form it does not know, and names `son drop '<id>' --purge` for each.

### Health

- **`son doctor` says what is wrong and how to put it right.** It checks the database file, its schema version, SQLite's integrity and reference checks, that the search index matches the nodes, that every stored node is of a form this `son` knows, and that the skill and instruction files are the ones this `son` writes. It also looks for stored text that looks like a secret, and for a database file that other users can read. Each check is `healthy`, `missing`, `stale`, `warning` or `problem`, with the command that fixes it (`son init`, `son reindex`, `son import <backup file> --replace`). Doctor only looks: it does not create or upgrade the database, and it repairs nothing. It exits with status 1 only when there is a `problem`.
- **Many sessions can use the database at the same time.** A session that finds it locked waits for up to 10 seconds, then fails with a message and changes nothing. Set `SON_BUSY_MS` to change that wait (in milliseconds).

## Commands

```
son init [--agent a,b] [--scope global|project] [--yes]   create the database, set up agents
         [--todo-skill] [--todo-skill-name <name>] [--overwrite-todo-skill]
son uninstall [--agent a,b] [--scope global|project] [--yes] [--dry-run]   remove what init wrote
son remember <type> "<title>" [--body <text>|-] [--collection <name>] [--tag a,b] [--link <id>:<relation>]
                              [--evidence user-statement|observation|inference] [--ref <kind>:<value>]
                              [--check]                    save nothing; list possible duplicates
                              [--key <text>]               a repeat with the same key saves nothing new
son recall <terms...> [--type <type>] [--collection <name>] [--tag a,b] [--limit 5] [--hops 1]
                      [--max-chars <number>] [--explain]
son context [--collection <name>] [--max-chars <number>]   what a collection holds, for the start of a session
son show <id>
son edit <id> [--title] [--body] [--type] [--status] [--priority] [--collection] [--tag] [--evidence] [--ref] [--unref]
son verify <id> --checked "<what was checked>" [--ref <kind>:<value>] | --clear
son stale <id> --reason "<why>" | --clear
son review [--collection <name>] [--type <type>] [--tag a,b] [--fact-days 90] [--decision-days 180] [--todo-days 30]
           [--limit 20] [--page 1]
son merge <source> <survivor> [--body <text>|-] [--title <text>] [--cross-collection]
son drop <id> [--purge]
son restore <id>
son history <id> [--op <number>] [--limit 20] [--page 1]
son undo <id> [--op <number>]
son todo add "<title>" [--priority p0..p4] [--parent <todo>] [--number <n>] [--status <status>]
                       [--collection <name>] [--tag a,b]
son todo list [--collection <name>] [--all] [--done] [--tag a,b]
son todo backlog [--collection <name>]     the open todos, one line each, children under their parent
son todo done <todo> [--note <text>]       <todo> is a number (4.1) or an id
son todo reopen <id>                       open a done or dropped todo again
son link <from> <relation> <to>
son unlink <from> <relation> <to>
son collections
son synonym add <term> <canonical> | list | remove <term>
son export [--collection <name>] [--tag a,b]   markdown; add --json for the portable format
son tags [--collection <name>]             the tags in use, with counts
son seed [<folder>]                        list files you already wrote, to bring into son
son completions <bash|zsh|fish|powershell> print a completion script for your shell
son import <file|-> [--replace]            merge a JSON export, or restore a backup
son backup <file|folder>                   write a full copy of the database
son doctor                                 check the database and the agent setup
son reindex
son list [--collection <name>] [--type <type>] [--tag a,b] [--dropped] [--limit 20] [--page 1]
son log [--collection <name>] [--command <name>] [--errors] [--limit 20] [--page 1]
son stats [--days 30 | --all]
```

Types are `collection`, `todo`, `decision`, `pattern`, `fact` and `note`. Every command takes `--json`. `--body -` reads the body from stdin. `--allow-secret` saves text that the [secret check](../README.md#secrets) refuses. An `<id>` is the 8-character id that `son` prints.

`son list` shows nodes, most recently changed first. `son log` shows every command that was run: what was recalled, how many results it gave, what was saved, and what failed. `son stats` shows how much is stored and how `son` is being used: commands per day, how often a recall finds something, the queries that found nothing, writes for each recall, and recall speed.

### Environment

| Variable | Effect |
| --- | --- |
| `SON_DB` | Use a different database file |
| `SON_COLLECTION` | Name the current collection |
| `SON_BUSY_MS` | How long to wait for a locked database, in milliseconds (default 10 seconds) |
| `SON_SOURCE` | A session name to record in the activity log |
| `SON_AUTHOR` | The name stored in the `author` field of each node you save (default: your login name) |
| `SON_NO_UPGRADE_BACKUP=1` | Upgrade the schema of an older database without first writing a copy of it |
| `SON_NO_LOG=1` | Turn the activity log off. The log is local |
| `SON_REVIEW_FACT_DAYS`, `SON_REVIEW_DECISION_DAYS`, `SON_REVIEW_TODO_DAYS` | The age limits for `son review` |

The activity log never makes a command wait. Its row is written in one attempt after the answer is given; if another session holds the database at that moment, the row is skipped, so the counts in `son log` and `son stats` can be a little low. The time stored for a command is the time of its own work: it does not include starting the program or writing the log row.

## For programs

### Errors

With `--json`, a command that succeeds writes its result to stdout. A command that fails writes nothing to stdout and one JSON object to stderr, and exits with a status that is not 0 (2 for a mistake in the arguments, 1 for everything else):

```json
{"error": {"code": "unknown_id", "message": "no node has an id ending in 'qqqqqq'", "retry": null}}
```

`code` is stable. `message` is for people and can change. `retry` is text when the same command can succeed if it is run again, and `null` when it cannot.

| Code | Meaning |
| --- | --- |
| `invalid_argument` | The command line is wrong: unknown command or flag, bad value, something that is not an id |
| `invalid_request` | The command line is valid, but the request cannot be done as asked (empty title, a node linked to itself, a merge that needs `--body`) |
| `unknown_id` | No node has that id |
| `ambiguous_id` | More than one node has that id; give more characters |
| `unknown_collection` | No collection has that name |
| `database_busy` | Another session held the database for longer than the wait. Run it again |
| `database_error` | SQLite reported another error |
| `unsupported_schema` | The database is from a newer `son`, or a read-only command met an older one |
| `upgrade_backup_failed` | The database is of an older schema, and the copy that is made before an upgrade could not be written. Nothing was changed |
| `upgrade_failed` | A step of a schema upgrade failed. The upgrade was taken back whole: nothing was changed, and the `son` that made the database can still use it |
| `conflict` | `son undo` found a later change, or the change was already undone. Nothing was changed |
| `unavailable` | `son undo` needs history that was purged. Nothing was changed |
| `idempotency_conflict` | The `--key` was already used for a different write. Nothing was saved |
| `import_failed` | The file is not a `son` export, is of a version this `son` does not read, or holds a record that is not valid. Nothing was imported |
| `install_failed` | `son init` could not write all files. Its report on stdout is still complete |
| `possible_secret` | The text looks like it holds a secret. Nothing was saved. Take the secret out, or add `--allow-secret` if it is not one |
| `uninstall_failed` | `son uninstall` left a file as it was. Its report on stdout is still complete |
| `broken_block` | An instruction file holds `son` marks that do not make exactly one block. `son init` changed nothing. Repair the file by hand |
| `io_error` | A file could not be read or written |
| `not_empty` | A backup was not restored because the database holds data and `--replace` was not given |
| `problems_found` | `son doctor` found a problem. Its report on stdout is still complete |

Without `--json`, errors are plain text on stderr.

### Repeated writes

A program that does not know if a write arrived (a timeout, a lost answer) can send it again safely with `--key <text>` on `son remember` and `son todo add`. The first time, the node is saved and the key is stored with it in the same transaction. Sent again with the same key and the same content, nothing new is saved and the first answer is returned again; the activity log shows it as a replay. The same key with different content is refused with `idempotency_conflict`. A key holds for the whole database, across sessions and agents, until the node is purged. A key is not a duplicate check: two writes with two keys save two nodes; use `--check` to look for duplicates.

## What stays the same in version 1

From 1.0.0 on, every 1.x release keeps these as they are. Things are added; nothing is removed and nothing changes its meaning.

- **Commands and flags:** their names and what they do. A command or flag that gets a better name keeps the old name working, hidden from the help, until 2.0.
- **Exit statuses:** 0 for success, 2 for a mistake in the arguments, 1 for every other failure.
- **Error codes:** the `code` values in the table under [Errors](#errors).
- **Environment variables:** the names in the table under [Environment](#environment).
- **JSON output (`--json`):** the names of the fields and what they mean. A field can be added, so a program must ignore fields it does not know. There is no version field in the output of a command, because fields are only added.
- **The export format:** a file written by `son export --json` in any release from 0.1.1 on is read by every 1.x release. The file has a `version` number of its own.
- **Your data:** every 1.x release opens a database written by any release from 0.1.1 on, and upgrades it (after a copy; see [Backup and export](#backup-and-export)).

These are not promised, and can change in any release:

- **The markdown output.** It is for people and for agents that read text. Two things in it do stay: every id that is shown works with `son show <id>`, and an answer that was cut to a size says so. A program must use `--json`.
- **The text of the skill files and of the instruction block.** `son init` brings them up to date.
- **The SQLite schema.** The database is one SQLite file, and you can read it with any SQLite tool, at your own risk: tables and columns change between releases. The stable way to get your data out is `son export --json`.
- **`message` and `retry` in an error**, which are text for people.
- **Two fields that the export holds and commands do not print:** `origin` and `visibility`. They are kept for the central store of version 2 and have no meaning yet.
