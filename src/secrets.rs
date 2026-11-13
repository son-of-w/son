//! A check for text that looks like a secret, made before a write.
//!
//! The set of patterns is narrow on purpose: private key headers, token
//! forms that are well known, JSON web tokens, and a long value straight
//! after `password=`, `token=` or `api_key=`. It does not guess from how
//! random a text looks, because that flags commit hashes and ids, and a
//! check that is often wrong is overridden by habit. It finds common
//! cases. It does not find every secret.
//!
//! A match never shows the text that matched: the message names the kind
//! of thing and the field it was in.

use crate::error::coded;
use anyhow::Result;

fn is_alnum(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

fn is_token_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// How many bytes from `at` on pass `keep`.
fn run(bytes: &[u8], at: usize, keep: impl Fn(u8) -> bool) -> usize {
    bytes[at.min(bytes.len())..]
        .iter()
        .take_while(|b| keep(**b))
        .count()
}

/// True when the byte before `at` is not a letter or a digit, so that
/// `at` is the start of a word.
fn starts_word(bytes: &[u8], at: usize) -> bool {
    at == 0 || !is_alnum(bytes[at - 1])
}

/// Every place `mark` starts in `text`.
fn places<'a>(text: &'a str, mark: &'a str) -> impl Iterator<Item = usize> + 'a {
    text.match_indices(mark).map(|(at, _)| at)
}

fn private_key(text: &str) -> bool {
    places(text, "-----BEGIN ").any(|at| {
        let rest = &text.as_bytes()[at..];
        let head = &rest[..rest.len().min(60)];
        head.windows(16).any(|w| w == b"PRIVATE KEY-----")
    })
}

/// `prefix`, then at least `least` bytes that pass `keep`, at the start of
/// a word.
fn prefixed(text: &str, prefix: &str, least: usize, keep: impl Fn(u8) -> bool + Copy) -> bool {
    let bytes = text.as_bytes();
    places(text, prefix)
        .any(|at| starts_word(bytes, at) && run(bytes, at + prefix.len(), keep) >= least)
}

fn aws_key_id(text: &str) -> bool {
    let bytes = text.as_bytes();
    places(text, "AKIA").any(|at| {
        let upper = |b: u8| b.is_ascii_uppercase() || b.is_ascii_digit();
        starts_word(bytes, at)
            && run(bytes, at + 4, upper) == 16
            && run(bytes, at + 4, is_alnum) == 16
    })
}

fn slack_token(text: &str) -> bool {
    let bytes = text.as_bytes();
    places(text, "xoxb-").any(|at| {
        let length = run(bytes, at + 5, is_token_char);
        starts_word(bytes, at)
            && length >= 10
            && bytes[at + 5..at + 5 + length]
                .iter()
                .any(u8::is_ascii_digit)
    })
}

/// Three parts joined by dots, the first two starting with `eyJ` (which is
/// how the base64 of `{"` starts).
fn json_web_token(text: &str) -> bool {
    let bytes = text.as_bytes();
    places(text, "eyJ").any(|at| {
        if !starts_word(bytes, at) {
            return false;
        }
        let first = run(bytes, at, is_token_char);
        let second_at = at + first + 1;
        if first < 10 || bytes.get(at + first) != Some(&b'.') {
            return false;
        }
        if !bytes[second_at.min(bytes.len())..].starts_with(b"eyJ") {
            return false;
        }
        let second = run(bytes, second_at, is_token_char);
        let third_at = second_at + second + 1;
        second >= 10
            && bytes.get(second_at + second) == Some(&b'.')
            && run(bytes, third_at, is_token_char) >= 10
    })
}

/// A value of 12 or more characters straight after `name=`, with a letter
/// and a digit in it. A placeholder (`<your token>`, `$TOKEN`, `{{token}}`)
/// and a plain word are not counted.
fn assigned(text: &str, name: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mark = format!("{name}=");
    let found = places(&lower, &mark).any(|at| {
        if !starts_word(bytes, at) {
            return false;
        }
        let mut from = at + mark.len();
        if matches!(bytes.get(from), Some(b'"' | b'\'')) {
            from += 1;
        }
        if matches!(bytes.get(from), Some(b'<' | b'$' | b'{' | b'%' | b'[')) {
            return false;
        }
        let stop =
            |b: u8| b.is_ascii_whitespace() || matches!(b, b'"' | b'\'' | b'&' | b',' | b';');
        let length = run(bytes, from, |b| !stop(b));
        let value = &bytes[from..from + length];
        length >= 12
            && value.iter().any(u8::is_ascii_digit)
            && value.iter().any(u8::is_ascii_alphabetic)
    });
    found
}

/// What kind of secret `text` looks like it holds, if any.
pub fn find(text: &str) -> Option<&'static str> {
    if private_key(text) {
        return Some("a private key");
    }
    if prefixed(text, "sk_live_", 16, is_alnum) {
        return Some("a Stripe live key");
    }
    if aws_key_id(text) {
        return Some("an AWS access key id");
    }
    if prefixed(text, "ghp_", 36, is_alnum)
        || prefixed(text, "github_pat_", 22, |b| is_alnum(b) || b == b'_')
    {
        return Some("a GitHub token");
    }
    if slack_token(text) {
        return Some("a Slack token");
    }
    if json_web_token(text) {
        return Some("a JSON web token");
    }
    if assigned(text, "password") {
        return Some("a password (a value after `password=`)");
    }
    if assigned(text, "token") {
        return Some("a token (a value after `token=`)");
    }
    if assigned(text, "api_key") {
        return Some("an API key (a value after `api_key=`)");
    }
    None
}

/// The first field of `fields` (name, text) that looks like it holds a
/// secret: its name, and the kind of secret.
pub fn first<'a>(
    fields: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Option<(&'a str, &'static str)> {
    fields
        .into_iter()
        .find_map(|(name, text)| find(text).map(|kind| (name, kind)))
}

/// Stop a write when one of `fields` looks like it holds a secret, unless
/// the caller allowed it. The error does not hold the text.
pub fn refuse<'a>(allow: bool, fields: impl IntoIterator<Item = (&'a str, &'a str)>) -> Result<()> {
    if allow {
        return Ok(());
    }
    match first(fields) {
        None => Ok(()),
        Some((field, kind)) => Err(coded(
            "possible_secret",
            format!(
                "nothing was saved: the {field} looks like it holds {kind}. \
                 Store where the secret lives, not the secret. \
                 If this text is not a secret, run the command again with --allow-secret"
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Made at run time, so that no test file holds a text that a secret
    /// scanner reads as a real key.
    fn join(parts: &[&str]) -> String {
        parts.concat()
    }

    #[test]
    fn each_pattern_matches() {
        let cases = [
            (
                join(&["-----BEGIN RSA ", "PRIVATE KEY-----\nMIIEow"]),
                "a private key",
            ),
            (
                join(&["-----BEGIN OPENSSH ", "PRIVATE KEY-----"]),
                "a private key",
            ),
            (join(&["-----BEGIN ", "PRIVATE KEY-----"]), "a private key"),
            (
                join(&["key: sk_", "live_", "4eC39HqLyjWDarjtT1zdp7dc"]),
                "a Stripe live key",
            ),
            (
                join(&["id AKIA", "IOSFODNN7EXAMPLE here"]),
                "an AWS access key id",
            ),
            (join(&["gh", "p_", &"a1B2".repeat(9)]), "a GitHub token"),
            (
                join(&["github_", "pat_", &"a1B2_".repeat(6)]),
                "a GitHub token",
            ),
            (
                join(&["xox", "b-", "123456789012-abcdefABCDEF"]),
                "a Slack token",
            ),
            (
                join(&[
                    "eyJ",
                    "hbGciOiJIUzI1NiJ9",
                    ".eyJ",
                    "zdWIiOiIxMjM0NTY3ODkwIn0",
                    ".dozjgNryP4J3jVmNHl0w5N",
                ]),
                "a JSON web token",
            ),
            (
                "DATABASE_PASSWORD=correct7horse8battery".to_string(),
                "a password (a value after `password=`)",
            ),
            (
                "curl 'https://x.test/?access_token=abc123def456ghi789'".to_string(),
                "a token (a value after `token=`)",
            ),
            (
                "API_KEY=\"9f8e7d6c5b4a39281706\"".to_string(),
                "an API key (a value after `api_key=`)",
            ),
        ];
        for (text, kind) in cases {
            assert_eq!(find(&text), Some(kind), "{text}");
        }
    }

    #[test]
    fn text_that_is_near_a_pattern_is_not_a_match() {
        let cases = [
            "-----BEGIN PUBLIC KEY-----\nMIIBIjANBg".to_string(),
            "-----BEGIN CERTIFICATE-----".to_string(),
            "the private key is in 1Password, vault 'ops'".to_string(),
            join(&["sk_", "test_", "4eC39HqLyjWDarjtT1zdp7dc"]),
            join(&["the prefix sk_", "live_ marks a live key"]),
            join(&["risk_", "live_", "4eC39HqLyjWDarjtT1zdp7dc"]),
            "AKIA is how an AWS key id starts".to_string(),
            join(&["AKIA", "IOSFODNN7EXAMPLEX1"]),
            join(&["XAKIA", "IOSFODNN7EXAMPLE"]),
            join(&["AKIA", "iosfodnn7example"]),
            join(&["gh", "p_", "short"]),
            join(&["github_", "pat_ is the prefix"]),
            join(&["xox", "b- is the prefix of a Slack bot token"]),
            "eyJ is how a JSON web token starts".to_string(),
            join(&["eyJ", "hbGciOiJIUzI1NiJ9 alone is one part"]),
            "password=<your password>".to_string(),
            "password=$DB_PASSWORD".to_string(),
            "password={{ vault.db_password }}".to_string(),
            "password=hunter2".to_string(),
            "password=correcthorsebatterystaple".to_string(),
            "set token= in the config file, then restart".to_string(),
            "the token is read from the keychain".to_string(),
            "api_key=%API_KEY_FROM_ENV%".to_string(),
            "commit 512767a3f0c1d2e4b5a69788c7d6e5f4a3b2c1d0".to_string(),
            "node 01JERR000000000000000AZZZZ".to_string(),
            "sha256 631c9a5a0e1f2d3c4b5a69788c7d6e5f4a3b2c1d0e9f8a7b6c5d4e3f2a1b82c7".to_string(),
            "tokenizer=unicode61remove2diacritics".to_string(),
            "Passwörter und Schlüssel: 漢字 ✓".to_string(),
        ];
        for text in cases {
            assert_eq!(find(&text), None, "{text}");
        }
    }

    #[test]
    fn the_error_names_the_field_and_not_the_text() {
        let secret = join(&["sk_", "live_", "4eC39HqLyjWDarjtT1zdp7dc"]);
        let err = refuse(false, [("title", "fine"), ("body", secret.as_str())]).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("the body looks like it holds a Stripe live key"));
        assert!(!message.contains("4eC39"));
        assert_eq!(crate::error::code_of(&err), "possible_secret");
        assert!(refuse(true, [("body", secret.as_str())]).is_ok());
        assert!(refuse(false, [("body", "nothing here")]).is_ok());
    }
}
