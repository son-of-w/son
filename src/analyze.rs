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
    /// The same terms, grouped by the query word they came from.
    pub groups: Vec<Group>,
    pub type_hint: Option<&'static str>,
}

/// One word of the query: its own stem, and every term that stands for it
/// (the stem plus its synonyms). Used to count how many of the query's words
/// a node covers, and as the starting point for prefix and fuzzy matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub stem: String,
    pub terms: Vec<String>,
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

        let mut groups: Vec<Group> = Vec::new();
        for word in words {
            let stem = self.stem(word);
            let mut group = vec![stem.clone()];
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
            if !group.is_empty() && !groups.iter().any(|g| g.terms == group) {
                groups.push(Group { stem, terms: group });
            }
        }
        let mut terms: Vec<String> = Vec::new();
        for token in groups.iter().flat_map(|g| &g.terms) {
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

/// How many single-character edits turn `a` into `b`: an insert, a delete,
/// a replacement, or a swap of two neighbours (so `terrafrom` is 1 away from
/// `terraform`). Returns `None` as soon as the answer must be above `max`.
pub fn edit_distance(a: &str, b: &str, max: usize) -> Option<usize> {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > max {
        return None;
    }
    // Three rows of the usual table: two back, one back, and the current.
    let mut before: Vec<usize> = vec![0; b.len() + 1];
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current: Vec<usize> = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        current[0] = i;
        let mut row_best = current[0];
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (previous[j] + 1)
                .min(current[j - 1] + 1)
                .min(previous[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(before[j - 2] + 1);
            }
            current[j] = best;
            row_best = row_best.min(best);
        }
        if row_best > max {
            return None;
        }
        std::mem::swap(&mut before, &mut previous);
        std::mem::swap(&mut previous, &mut current);
    }
    let distance = previous[b.len()];
    (distance <= max).then_some(distance)
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
    fn edit_distance_counts_typos() {
        assert_eq!(edit_distance("terraform", "terraform", 2), Some(0));
        assert_eq!(edit_distance("terrafrom", "terraform", 2), Some(1)); // swap
        assert_eq!(edit_distance("teraform", "terraform", 2), Some(1)); // missing letter
        assert_eq!(edit_distance("terraformm", "terraform", 2), Some(1)); // extra letter
        assert_eq!(edit_distance("tarraform", "terraform", 2), Some(1)); // wrong letter
        assert_eq!(edit_distance("tarrafrom", "terraform", 2), Some(2));
        assert_eq!(edit_distance("postgres", "terraform", 2), None);
        assert_eq!(edit_distance("ab", "abcde", 2), None);
        assert_eq!(edit_distance("", "ab", 2), Some(2));
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
