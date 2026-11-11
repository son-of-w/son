//! Text analysis without a model: the same steps run on stored text and on
//! queries, so both sides meet on the same tokens.
//!
//! 1. Unicode-normalize (NFKC)
//! 2. split on anything that is not a letter or digit, and on camelCase
//! 3. lowercase
//! 4. drop stop words
//! 5. Snowball (English) stemming
//!
//! Synonyms are applied to queries only, so the index stays canonical.

use rust_stemmers::{Algorithm, Stemmer};
use std::collections::HashMap;
use unicode_normalization::UnicodeNormalization;

const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "for", "from", "had", "has", "have",
    "how", "i", "if", "in", "into", "is", "it", "its", "of", "on", "or", "our", "so", "than",
    "that", "the", "their", "then", "there", "these", "this", "to", "was", "we", "were", "what",
    "when", "which", "will", "with", "you", "your",
];

/// Words in a query that name a node type. They boost that type in ranking.
const TYPE_HINTS: &[(&str, &str)] = &[
    ("todo", "todo"),
    ("todos", "todo"),
    ("decision", "decision"),
    ("decisions", "decision"),
    ("pattern", "pattern"),
    ("patterns", "pattern"),
    ("fact", "fact"),
    ("facts", "fact"),
    ("note", "note"),
    ("notes", "note"),
    ("project", "project"),
    ("projects", "project"),
];

pub struct Analyzer {
    stemmer: Stemmer,
}

impl Default for Analyzer {
    fn default() -> Self {
        Self::new()
    }
}

/// A query after analysis: stemmed terms to OR together, plus a type hint.
pub struct Query {
    /// Every stemmed term, without repeats.
    pub terms: Vec<String>,
    /// The same terms, grouped by the query word they came from: the word
    /// itself plus its synonyms. Used to count how many of the query's words
    /// a node covers.
    pub groups: Vec<Vec<String>>,
    pub type_hint: Option<&'static str>,
}

impl Query {
    /// The FTS5 MATCH expression, or `None` when there is nothing to search.
    pub fn fts(&self) -> Option<String> {
        if self.terms.is_empty() {
            return None;
        }
        let quoted: Vec<String> = self.terms.iter().map(|t| format!("\"{t}\"")).collect();
        Some(quoted.join(" OR "))
    }
}

impl Analyzer {
    pub fn new() -> Self {
        Self {
            stemmer: Stemmer::create(Algorithm::English),
        }
    }

    /// Lowercase words with camelCase and snake_case split apart. A camelCase
    /// word also keeps its joined form, so `awsProvider` gives
    /// `aws provider awsprovider`.
    pub fn words(text: &str) -> Vec<String> {
        let normalized: String = text.nfkc().collect();
        let mut words = Vec::new();
        for chunk in normalized.split(|c: char| !c.is_alphanumeric()) {
            if chunk.is_empty() {
                continue;
            }
            let parts = split_camel(chunk);
            if parts.len() > 1 {
                words.extend(
                    parts
                        .iter()
                        .filter(|p| p.chars().count() > 1)
                        .map(|p| p.to_lowercase()),
                );
            }
            words.push(chunk.to_lowercase());
        }
        words
    }

    fn stem(&self, word: &str) -> String {
        self.stemmer.stem(word).into_owned()
    }

    /// Words with stop words removed, then stemmed.
    pub fn tokens(&self, text: &str) -> Vec<String> {
        Self::words(text)
            .iter()
            .filter(|w| !STOP_WORDS.contains(&w.as_str()))
            .map(|w| self.stem(w))
            .collect()
    }

    /// The text that goes into the FTS5 index.
    pub fn index_text(&self, text: &str) -> String {
        self.tokens(text).join(" ")
    }

    /// Analyze a query. `synonyms` maps a term to its canonical form; a query
    /// word is expanded to its canonical form and to every other term that
    /// shares it, so `tf`, `terraform` and any other alias all find each other.
    pub fn query(&self, text: &str, synonyms: &HashMap<String, String>) -> Query {
        let all_words = Self::words(text);
        let type_hint = all_words.iter().find_map(|w| {
            TYPE_HINTS
                .iter()
                .find(|(word, _)| word == w)
                .map(|(_, kind)| *kind)
        });

        let mut words: Vec<&String> = all_words
            .iter()
            .filter(|w| !STOP_WORDS.contains(&w.as_str()))
            .collect();
        if words.is_empty() {
            // A query made only of stop words: search for them as they are.
            words = all_words.iter().collect();
        }

        let mut groups: Vec<Vec<String>> = Vec::new();
        for word in words {
            let mut group = vec![self.stem(word)];
            let canonical = synonyms.get(word).unwrap_or(word);
            group.extend(self.tokens(canonical));
            for (term, target) in synonyms {
                if target == canonical && term != word {
                    group.extend(self.tokens(term));
                }
            }
            group.retain(|token| !token.is_empty());
            group.sort();
            group.dedup();
            if !group.is_empty() && !groups.contains(&group) {
                groups.push(group);
            }
        }
        let mut terms: Vec<String> = Vec::new();
        for token in groups.iter().flatten() {
            if !terms.contains(token) {
                terms.push(token.clone());
            }
        }
        Query {
            terms,
            groups,
            type_hint,
        }
    }
}

/// `awsProvider` -> `aws`, `Provider`; `HTTPServer` -> `HTTP`, `Server`.
fn split_camel(chunk: &str) -> Vec<String> {
    let chars: Vec<char> = chunk.chars().collect();
    let mut parts = Vec::new();
    let mut start = 0;
    for i in 1..chars.len() {
        let (prev, cur) = (chars[i - 1], chars[i]);
        let lower_to_upper = prev.is_lowercase() && cur.is_uppercase();
        let acronym_end = prev.is_uppercase()
            && cur.is_uppercase()
            && chars.get(i + 1).is_some_and(|next| next.is_lowercase());
        if lower_to_upper || acronym_end {
            parts.push(chars[start..i].iter().collect());
            start = i;
        }
    }
    parts.push(chars[start..].iter().collect());
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synonyms() -> HashMap<String, String> {
        [
            ("tf", "terraform"),
            ("k8s", "kubernetes"),
            ("kube", "kubernetes"),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
    }

    #[test]
    fn camel_and_snake_case_split_the_same_way() {
        let camel = Analyzer::words("awsProvider");
        let snake = Analyzer::words("aws_provider");
        assert_eq!(snake, ["aws", "provider"]);
        assert_eq!(camel, ["aws", "provider", "awsprovider"]);
    }

    #[test]
    fn acronyms_split_before_the_next_word() {
        assert_eq!(
            Analyzer::words("HTTPServer"),
            ["http", "server", "httpserver"]
        );
        assert_eq!(Analyzer::words("GitHub"), ["git", "hub", "github"]);
    }

    #[test]
    fn digits_stay_inside_words() {
        assert_eq!(Analyzer::words("k8s s3 v2"), ["k8s", "s3", "v2"]);
    }

    #[test]
    fn stop_words_are_removed_and_words_are_stemmed() {
        let an = Analyzer::new();
        assert_eq!(
            an.tokens("How we are configuring the modules"),
            ["configur", "modul"]
        );
        assert_eq!(an.tokens("configuration"), an.tokens("configured"));
    }

    #[test]
    fn punctuation_never_reaches_the_index() {
        let an = Analyzer::new();
        assert_eq!(an.index_text("foo-bar: \"baz\" (qux)*"), "foo bar baz qux");
    }

    #[test]
    fn synonyms_expand_in_both_directions() {
        let an = Analyzer::new();
        let from_alias = an.query("tf", &synonyms()).terms;
        assert!(from_alias.contains(&"tf".to_string()));
        assert!(from_alias.contains(&"terraform".to_string()));

        let from_canonical = an.query("kubernetes", &synonyms()).terms;
        assert!(from_canonical.contains(&"k8s".to_string()));
        assert!(from_canonical.contains(&"kube".to_string()));
    }

    #[test]
    fn type_words_become_a_hint() {
        let an = Analyzer::new();
        assert_eq!(
            an.query("terraform decisions", &synonyms()).type_hint,
            Some("decision")
        );
        assert_eq!(an.query("terraform aws", &synonyms()).type_hint, None);
    }

    #[test]
    fn stop_word_only_queries_still_search() {
        let an = Analyzer::new();
        assert!(an.query("the", &synonyms()).fts().is_some());
        assert!(an.query("!!!", &synonyms()).fts().is_none());
    }
}
