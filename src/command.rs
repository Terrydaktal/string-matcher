use crate::core::{OperationProfile, bounded_damerau_levenshtein, max_typos};
use crate::ranking::{TypoSortKey, compare_typo_sort_keys, ratio_milli, typo_keys_are_ambiguous};
use crate::text::{has_query_signal, separator_variants, split_whitespace_tokens, to_lowercase};
use crate::token_match::{PositionedToken, aligned_token_distance, partitioned_token_distance};

pub type Score = f64;

#[derive(Clone, Copy, Debug)]
pub struct CommandCandidate<'a> {
    pub command: &'a str,
    pub tokens: &'a [&'a str],
    pub score: Score,
}

#[derive(Clone, Copy, Debug)]
pub struct CommandMatch<'a> {
    pub candidate: CommandCandidate<'a>,
    pub distance: usize,
    pub ratio: f64,
    pub position: usize,
    pub structure: usize,
    pub operations: OperationProfile,
    token_count: usize,
}

impl<'a> CommandMatch<'a> {
    fn sort_key(&self) -> TypoSortKey<'_> {
        TypoSortKey {
            distance: self.distance,
            operations: self.operations,
            ratio_milli: ratio_milli(self.distance, self.compared_len()),
            position: self.position,
            structure: self.structure,
            score: self.candidate.score,
            secondary: self.token_count,
            key: self.candidate.command,
        }
    }

    fn compared_len(&self) -> usize {
        let query_estimate = if self.ratio == 0.0 {
            1
        } else {
            ((self.distance as f64 / self.ratio).round() as usize).max(1)
        };
        query_estimate.max(1)
    }

    pub fn compare(&self, other: &Self) -> std::cmp::Ordering {
        compare_typo_sort_keys(&self.sort_key(), &other.sort_key())
    }
}

pub struct CommandQuery {
    text: String,
    len: usize,
    limit: usize,
    token_count: usize,
    separator_variants: Vec<String>,
}

impl CommandQuery {
    pub fn new(query: &str) -> Option<Self> {
        let query = to_lowercase(query.trim());
        let len = query.chars().count();
        if len <= 1 {
            return None;
        }

        let token_count = split_whitespace_tokens(&query).len();
        Some(Self {
            text: query.clone(),
            len,
            limit: max_typos(len),
            token_count,
            separator_variants: if token_count == 1 {
                separator_variants(&query)
            } else {
                Vec::new()
            },
        })
    }

    pub fn best_match<'a>(&self, candidate: CommandCandidate<'a>) -> Option<CommandMatch<'a>> {
        let lower_command = to_lowercase(candidate.command);
        let lower_tokens = candidate
            .tokens
            .iter()
            .map(to_lowercase)
            .collect::<Vec<_>>();
        let lowered = LoweredCandidate {
            original: candidate,
            command: lower_command,
            tokens: lower_tokens,
        };
        let mut best = best_match_inner(&lowered, &self.text, self.len, self.limit);
        if self.token_count == 1 {
            for variant in &self.separator_variants {
                let Some(mut matched) =
                    best_match_inner(&lowered, variant, self.len, self.limit.saturating_sub(1))
                else {
                    continue;
                };
                let distance = matched.distance + 1;
                if distance > self.limit {
                    continue;
                }
                matched.distance = distance;
                matched.operations = matched.operations.with_insert_delete();
                matched.ratio = matched.ratio.max(distance as f64 / self.len as f64);
                update_best(&mut best, Some(matched));
            }
        }
        best
    }
}

pub fn best_match<'a>(candidate: CommandCandidate<'a>, query: &str) -> Option<CommandMatch<'a>> {
    CommandQuery::new(query)?.best_match(candidate)
}

pub fn parse_command_candidate(command: &str, score: Score) -> ParsedCommandCandidate<'_> {
    ParsedCommandCandidate {
        command,
        tokens: command.split_whitespace().collect(),
        score,
    }
}

pub struct ParsedCommandCandidate<'a> {
    command: &'a str,
    tokens: Vec<&'a str>,
    score: Score,
}

impl<'a> ParsedCommandCandidate<'a> {
    pub fn as_candidate(&self) -> CommandCandidate<'_> {
        CommandCandidate {
            command: self.command,
            tokens: &self.tokens,
            score: self.score,
        }
    }
}

pub fn sort_matches(matches: &mut [CommandMatch<'_>]) {
    matches.sort_unstable_by(CommandMatch::compare);
}

pub fn is_ambiguous(first: &CommandMatch<'_>, second: &CommandMatch<'_>) -> bool {
    typo_keys_are_ambiguous(&first.sort_key(), &second.sort_key())
}

struct LoweredCandidate<'a> {
    original: CommandCandidate<'a>,
    command: String,
    tokens: Vec<String>,
}

fn best_match_inner<'a>(
    candidate: &LoweredCandidate<'a>,
    query: &str,
    query_len: usize,
    limit: usize,
) -> Option<CommandMatch<'a>> {
    if !has_query_signal(split_whitespace_tokens(query), &candidate.command) {
        return None;
    }

    let query_tokens = split_whitespace_tokens(query);
    if query_tokens.is_empty() || candidate.tokens.is_empty() {
        return None;
    }

    let mut best = None;

    if query_tokens.len() == 1 {
        for (idx, token) in candidate.tokens.iter().enumerate() {
            update_best(
                &mut best,
                candidate_for_text(candidate.original, token, idx, query, query_len, limit),
            );
        }

        let ratio = best
            .as_ref()
            .map(|entry: &CommandMatch<'_>| entry.ratio)
            .unwrap_or(1.0);
        if ratio <= 0.2 {
            return best;
        }
    }

    if query_tokens.len() <= candidate.tokens.len() {
        update_best(
            &mut best,
            candidate_for_token_sequence(
                candidate.original,
                &candidate.tokens,
                &query_tokens,
                query_len,
                limit,
            ),
        );
    }

    for (idx, token) in candidate.tokens.iter().enumerate() {
        update_best(
            &mut best,
            candidate_for_compound_token(
                candidate.original,
                token,
                idx,
                &query_tokens,
                query_len,
                limit,
            ),
        );
    }

    best
}

fn candidate_for_text<'a>(
    candidate: CommandCandidate<'a>,
    token: &str,
    token_idx: usize,
    query: &str,
    query_len: usize,
    limit: usize,
) -> Option<CommandMatch<'a>> {
    let token_len = token.chars().count();
    if token.is_empty() || limit == 0 || query_len.abs_diff(token_len) > limit {
        return None;
    }

    let (distance, operations) = bounded_damerau_levenshtein(query, token, limit)?;
    let ratio = distance as f64 / query_len.max(token_len) as f64;
    if ratio > 0.5 {
        return None;
    }

    Some(CommandMatch {
        candidate,
        distance,
        ratio,
        position: token_idx * 3,
        structure: 0,
        operations,
        token_count: candidate.tokens.len(),
    })
}

fn candidate_for_token_sequence<'a>(
    candidate: CommandCandidate<'a>,
    tokens: &[String],
    query_tokens: &[&str],
    query_len: usize,
    limit: usize,
) -> Option<CommandMatch<'a>> {
    let candidate_tokens = tokens
        .iter()
        .enumerate()
        .map(|(idx, token)| PositionedToken {
            token,
            position: idx * 3,
        })
        .collect::<Vec<_>>();
    let (distance, position, structure, operations) =
        aligned_token_distance(query_tokens, &candidate_tokens, limit)?;
    let ratio = distance as f64 / query_len as f64;
    if ratio > 0.5 {
        return None;
    }

    Some(CommandMatch {
        candidate,
        distance,
        ratio,
        position,
        structure,
        operations,
        token_count: candidate.tokens.len(),
    })
}

fn candidate_for_compound_token<'a>(
    candidate: CommandCandidate<'a>,
    token: &str,
    token_idx: usize,
    query_tokens: &[&str],
    query_len: usize,
    limit: usize,
) -> Option<CommandMatch<'a>> {
    if query_tokens.len() < 2 {
        return None;
    }

    let (distance, structure, position_metric, operations) =
        partitioned_token_distance(query_tokens, token, limit)?;
    let ratio = distance as f64 / query_len.max(token.chars().count()) as f64;
    if ratio > 0.5 {
        return None;
    }

    Some(CommandMatch {
        candidate,
        distance,
        ratio,
        position: token_idx * 3 * query_tokens.len() + position_metric,
        structure,
        operations,
        token_count: candidate.tokens.len(),
    })
}

fn update_best<'a>(best: &mut Option<CommandMatch<'a>>, candidate: Option<CommandMatch<'a>>) {
    match (best.as_ref(), candidate) {
        (_, None) => {}
        (None, Some(candidate)) => *best = Some(candidate),
        (Some(current), Some(candidate)) if candidate.compare(current).is_lt() => {
            *best = Some(candidate)
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate<'a>(
        command: &'a str,
        tokens: &'a [&'a str],
        score: Score,
    ) -> CommandCandidate<'a> {
        CommandCandidate {
            command,
            tokens,
            score,
        }
    }

    #[test]
    fn executable_typo_matches_first() {
        let matched = best_match(candidate("git status", &["git", "status"], 10.0), "gti").unwrap();
        assert_eq!(matched.distance, 1);
        assert_eq!(matched.position, 0);
    }

    #[test]
    fn multi_token_query_matches_executable_and_argument() {
        let matched = best_match(
            candidate("git status", &["git", "status"], 10.0),
            "git staus",
        )
        .unwrap();
        assert_eq!(matched.distance, 1);
        assert_eq!(matched.position, 3);
    }

    #[test]
    fn missing_space_query_can_match_command_sequence() {
        let matched = best_match(
            candidate("git status", &["git", "status"], 10.0),
            "gitstaus",
        )
        .unwrap();
        assert_eq!(matched.distance, 2);
    }

    #[test]
    fn executable_match_beats_argument_only_match() {
        let mut matches = vec![
            best_match(candidate("cargo git", &["cargo", "git"], 100.0), "git").unwrap(),
            best_match(candidate("git status", &["git", "status"], 1.0), "git").unwrap(),
        ];
        sort_matches(&mut matches);
        assert_eq!(matches[0].candidate.command, "git status");
        assert!(matches[0].position < matches[1].position);
    }

    #[test]
    fn lower_distance_beats_score() {
        let mut matches = vec![
            best_match(candidate("git status", &["git", "status"], 1.0), "gti").unwrap(),
            best_match(candidate("gist status", &["gist", "status"], 100.0), "gti").unwrap(),
        ];
        sort_matches(&mut matches);
        assert_eq!(matches[0].candidate.command, "git status");
    }

    #[test]
    fn score_breaks_equal_ties() {
        let mut matches = vec![
            best_match(candidate("git status", &["git", "status"], 1.0), "gti").unwrap(),
            best_match(candidate("git stash", &["git", "stash"], 100.0), "gti").unwrap(),
        ];
        sort_matches(&mut matches);
        assert_eq!(matches[0].candidate.command, "git stash");
    }
}
