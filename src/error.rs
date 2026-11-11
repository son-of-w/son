//! Errors that a program can read.
//!
//! With `--json`, a failed command writes one JSON object to stderr and
//! nothing to stdout:
//!
//! ```json
//! {"error": {"code": "unknown_id", "message": "...", "retry": null}}
//! ```
//!
//! `code` is stable: a program can branch on it. `message` is for people
//! and may change. `retry` says what to do when doing the same thing again
//! can help, and is null when it cannot. The exit status is not zero.

use serde_json::{json, Value};
use std::fmt;

/// Every code `son` can give.
pub const CODES: [&str; 16] = [
    "invalid_argument",
    "invalid_request",
    "unknown_id",
    "ambiguous_id",
    "unknown_collection",
    "database_busy",
    "database_error",
    "unsupported_schema",
    "conflict",
    "unavailable",
    "idempotency_conflict",
    "import_failed",
    "install_failed",
    "io_error",
    "not_empty",
    "problems_found",
];

/// An error with a code. Put it in an `anyhow::Error` with [`coded`].
#[derive(Debug)]
pub struct Coded {
    pub code: &'static str,
    pub message: String,
}

impl fmt::Display for Coded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Coded {}

pub fn coded(code: &'static str, message: impl Into<String>) -> anyhow::Error {
    debug_assert!(CODES.contains(&code), "unknown error code {code}");
    anyhow::Error::new(Coded {
        code,
        message: message.into(),
    })
}

/// The code for an error: the one it was given, else one worked out from
/// what kind of error it is. An error that `son` raised itself without a
/// code is a request that cannot be done as asked.
pub fn code_of(err: &anyhow::Error) -> &'static str {
    for cause in err.chain() {
        if let Some(coded) = cause.downcast_ref::<Coded>() {
            return coded.code;
        }
        if let Some(sqlite) = cause.downcast_ref::<rusqlite::Error>() {
            return if crate::db::is_busy(sqlite) {
                "database_busy"
            } else {
                "database_error"
            };
        }
        if cause.downcast_ref::<std::io::Error>().is_some() {
            return "io_error";
        }
    }
    "invalid_request"
}

/// What to do about an error of this code, when trying again can help.
pub fn retry_of(code: &str) -> Option<&'static str> {
    match code {
        "database_busy" => Some(
            "Another session held the database. Run the same command again. \
             With --key on a write, a repeat cannot save twice.",
        ),
        "io_error" => Some("Check the path and its permissions, then run the command again."),
        "unsupported_schema" => {
            Some("Install a newer son, or run `son init --db-only` to upgrade the database.")
        }
        _ => None,
    }
}

pub fn json_of(code: &str, message: &str) -> Value {
    json!({ "error": { "code": code, "message": message, "retry": retry_of(code) } })
}

/// The JSON error object for a failed command.
pub fn to_json(err: &anyhow::Error) -> Value {
    json_of(code_of(err), &format!("{err:#}"))
}
