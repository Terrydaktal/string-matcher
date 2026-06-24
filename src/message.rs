use std::cmp::{Ordering, Reverse};
use std::collections::HashMap;

use crate::text::{split_search_tokens, to_lowercase};

pub type Score = f64;

#[derive(Clone, Copy, Debug)]
pub struct MessageCandidate<'a> {
    pub key: &'a str,
    pub text: &'a str,
    pub score: Score,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MessageQuery {
    tokens: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct PreparedMessageCandidate<'a> {
    pub key: &'a str,
    pub text: &'a str,
    pub score: Score,
    lowered: String,
    tokens: Vec<String>,
    term_counts: HashMap<String, usize>,
}

#[derive(Clone, Copy, Debug)]
pub struct MessageMatch<'a> {
    pub key: &'a str,
    pub text: &'a str,
    pub score: Score,
    pub phrase_occurrences: usize,
    pub matched_terms: usize,
    pub total_occurrences: usize,
}

impl<'a> MessageCandidate<'a> {
    pub fn prepare(self) -> PreparedMessageCandidate<'a> {
        let lowered = to_lowercase(self.text);
        let tokens = split_search_tokens(&lowered)
            .into_iter()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        let mut term_counts = HashMap::new();
        for token in &tokens {
            *term_counts.entry(token.clone()).or_insert(0) += 1;
        }

        PreparedMessageCandidate {
            key: self.key,
            text: self.text,
            score: self.score,
            lowered,
            tokens,
            term_counts,
        }
    }
}

impl MessageQuery {
    pub fn new(query: &str) -> Option<Self> {
        let lowered = to_lowercase(query);
        let mut tokens = Vec::new();
        for token in split_search_tokens(&lowered) {
            if !tokens.iter().any(|existing| existing == token) {
                tokens.push(token.to_owned());
            }
        }

        (!tokens.is_empty()).then_some(Self { tokens })
    }

    pub fn search_rank<'a>(&self, candidate: MessageCandidate<'a>) -> Option<MessageMatch<'a>> {
        let prepared = candidate.prepare();
        self.rank_from_parts(prepared.key, prepared.text, prepared.score, &prepared.tokens, &prepared.term_counts)
    }

    pub fn search_rank_prepared<'a>(
        &self,
        candidate: &'a PreparedMessageCandidate<'a>,
    ) -> Option<MessageMatch<'a>> {
        self.rank_from_parts(
            candidate.key,
            candidate.text,
            candidate.score,
            &candidate.tokens,
            &candidate.term_counts,
        )
    }

    fn rank_from_parts<'a>(
        &self,
        key: &'a str,
        text: &'a str,
        score: Score,
        candidate_tokens: &[String],
        term_counts: &HashMap<String, usize>,
    ) -> Option<MessageMatch<'a>> {
        if candidate_tokens.is_empty() {
            return None;
        }

        let counts = self
            .tokens
            .iter()
            .map(|query_token| term_counts.get(query_token).copied().unwrap_or(0))
            .collect::<Vec<_>>();

        let matched_terms = counts.iter().filter(|count| **count > 0).count();
        if matched_terms == 0 {
            return None;
        }

        let total_occurrences = counts.iter().sum();
        let phrase_occurrences = count_phrase_occurrences(&self.tokens, candidate_tokens);

        Some(MessageMatch {
            key,
            text,
            score,
            phrase_occurrences,
            matched_terms,
            total_occurrences,
        })
    }
}

pub fn sort_matches(matches: &mut [MessageMatch<'_>]) {
    matches.sort_by(|left, right| compare_matches(*left, *right));
}

fn compare_matches(left: MessageMatch<'_>, right: MessageMatch<'_>) -> Ordering {
    (
        Reverse(left.phrase_occurrences > 0),
        Reverse(left.matched_terms),
        Reverse(left.phrase_occurrences),
        Reverse(left.total_occurrences),
    )
        .cmp(&(
            Reverse(right.phrase_occurrences > 0),
            Reverse(right.matched_terms),
            Reverse(right.phrase_occurrences),
            Reverse(right.total_occurrences),
        ))
        .then_with(|| right.score.total_cmp(&left.score))
        .then_with(|| left.key.cmp(right.key))
}

pub fn contains_query_signal(query: &MessageQuery, candidate: &PreparedMessageCandidate<'_>) -> bool {
    query
        .tokens
        .iter()
        .any(|token| candidate.term_counts.contains_key(token))
        || candidate.lowered.contains(query.tokens[0].as_str())
}

fn count_phrase_occurrences(query_tokens: &[String], candidate_tokens: &[String]) -> usize {
    if query_tokens.is_empty() || query_tokens.len() > candidate_tokens.len() {
        return 0;
    }

    candidate_tokens
        .windows(query_tokens.len())
        .filter(|window| {
            window
                .iter()
                .zip(query_tokens)
                .all(|(candidate, query)| candidate == query)
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::{
        MessageCandidate, MessageQuery, PreparedMessageCandidate, contains_query_signal, sort_matches,
    };

    fn matched_keys<'a>(query: &str, candidates: &'a [PreparedMessageCandidate<'a>]) -> Vec<&'a str> {
        let query = MessageQuery::new(query).unwrap();
        let mut matches = candidates
            .iter()
            .filter_map(|candidate| query.search_rank_prepared(candidate))
            .collect::<Vec<_>>();
        sort_matches(&mut matches);
        matches.into_iter().map(|matched| matched.key).collect()
    }

    fn prepare<'a>(key: &'a str, text: &'a str, score: f64) -> PreparedMessageCandidate<'a> {
        MessageCandidate { key, text, score }.prepare()
    }

    #[test]
    fn phrase_match_beats_non_phrase_match() {
        let candidates = [
            prepare("phrase", "word1 word2 together", 0.0),
            prepare("split", "word1 between other word2", 0.0),
        ];

        assert_eq!(matched_keys("word1 word2", &candidates), vec!["phrase", "split"]);
    }

    #[test]
    fn full_coverage_beats_partial_with_more_occurrences() {
        let candidates = [
            prepare("both", "word1 word2", 0.0),
            prepare(
                "one-many",
                "word1 word1 word1 word1 word1 word1 word1 word1 word1 word1",
                0.0,
            ),
        ];

        assert_eq!(matched_keys("word1 word2", &candidates), vec!["both", "one-many"]);
    }

    #[test]
    fn more_occurrences_break_ties_within_same_coverage() {
        let candidates = [
            prepare("many", "word1 word2 word1 word2", 0.0),
            prepare("few", "word1 word2", 0.0),
        ];

        assert_eq!(matched_keys("word1 word2", &candidates), vec!["many", "few"]);
    }

    #[test]
    fn more_occurrences_of_single_matched_word_break_partial_ties() {
        let candidates = [
            prepare("many-word1", "word1 word1 word1", 0.0),
            prepare("one-word1", "word1", 0.0),
        ];

        assert_eq!(matched_keys("word1 word2", &candidates), vec!["many-word1", "one-word1"]);
    }

    #[test]
    fn score_breaks_remaining_ties() {
        let candidates = [
            prepare("lower", "word1 word2", 1.0),
            prepare("higher", "word1 word2", 2.0),
        ];

        assert_eq!(matched_keys("word1 word2", &candidates), vec!["higher", "lower"]);
    }

    #[test]
    fn signal_check_is_exact_token_or_fast_substring_hit() {
        let query = MessageQuery::new("word1 word2").unwrap();
        let matching = prepare("matching", "word2 appears here", 0.0);
        let partial = prepare("partial", "this contains word1x only", 0.0);

        assert!(contains_query_signal(&query, &matching));
        assert!(contains_query_signal(&query, &partial));
    }
}
