# Security

## Report a fault in private

If you find a security fault in `son`, please do not open a public issue. Report it in private through GitHub: on the page of this repository, open **Security**, then **Report a vulnerability**.

Say which version you used (`son --version`), which system, what you did, and what happened. If you can, give the smallest set of commands that shows the fault.

You get an answer as soon as the report is read. A fix is released as a new version, and the changelog names it.

## Which versions get fixes

The newest release. `son` is one binary with no service behind it, so the fix for an older version is to install the newest one. Every release opens the databases of older releases.

## What son protects, and what it does not

`son` is a local tool. It makes no network calls. Knowing what it is not helps you to use it safely.

- **The database is plain text.** `~/.son-of-w/son.db` is one SQLite file that is not encrypted. Anyone who can read the file can read every node. On macOS and Linux a new database is made for its owner only; `son doctor` reports a file that other users can read.
- **Do not store secrets.** `son` refuses a write that holds a common form of secret (a private key header, a well-known token form, a long value after `password=`). This finds common cases, not every secret. A secret that was saved is also in the history and in every backup made after it; remove it with `son drop <id> --purge`, and change the secret where it is used.
- **Collections are not a boundary.** A collection groups nodes. Every agent that can run `son` can read every collection.
- **Stored text is not trusted input.** The skill tells an agent that recalled content is reference material, not instructions, and that a stored note cannot grant a permission. This is guidance for the agent, not a control that `son` enforces. Do not store text from sources that you do not trust.
- **Every agent on the machine has the same access.** `son` has no accounts and no roles. An agent that can run commands as you can read, change and delete what is stored. History and `son undo` reverse a change; `son backup` protects against a loss.

A report that shows a way around one of these stated limits is still welcome if it shows something that the text above does not say.
