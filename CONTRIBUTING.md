# Contributing to son

Thank you for your interest. `son` is a small tool with a narrow purpose: one local memory that every AI agent on a machine reads and writes. Contributions that keep it small, local and dependable are welcome.

## Before you start

- **A small fix** (a bug, a wrong word in the help, a missing test): open a pull request.
- **A larger change** (a new command, a new flag, a change to the schema or to the ranking): open an issue first and describe what you want and why. It saves your time if the change does not fit.
- **A security fault:** do not open an issue. See [SECURITY.md](SECURITY.md).

## What is out of scope

- **The central store and sync.** Sharing memory between machines or people is planned as version 2. It is not built in this repository's version 1, and pull requests that start it are not taken.
- **Network calls.** Version 1 makes none: no telemetry, no update check, no model calls.
- **Removing or renaming** a command, a flag, an error code, an environment variable or a JSON field. From 1.0.0 on these only grow. See "What stays the same in version 1" in the [README](README.md).

## Build and check

You need a [Rust toolchain](https://rustup.rs).

```bash
make build    # a debug binary at target/debug/son
make check    # formatting, clippy with warnings as errors, and all tests
```

`make check` must pass before a pull request is merged. CI runs the same checks on macOS, Linux and Windows.

## Rules for a change

- **It works on macOS, Linux and Windows.** Do not use paths or APIs of one system only.
- **It has tests.** A change to the analyzer or the ranking needs a case in `tests/ranking.rs`. A new command, flag, error code or JSON field needs its line in `tests/compat/` (the failing test tells you which).
- **Tests never touch your real setup.** `son init` writes into agent folders such as `~/.claude`. Run it only with `HOME` and `SON_DB` set to a temporary place, as `tests/cli.rs` does.
- **Existing data keeps working.** A change to the schema needs an upgrade step and a test that upgrades an older database. An older export must still import.
- **The README says what the code does.** Change it in the same pull request.
- **The changelog gets a line.** Add it under "Unreleased" in [CHANGELOG.md](CHANGELOG.md) when a user can see the change.
- **Plain words.** Help text, messages and documentation use short sentences and common words.

## License

`son` is licensed under the [MIT License](LICENSE). By sending a contribution, you agree that it is licensed under the same terms.
