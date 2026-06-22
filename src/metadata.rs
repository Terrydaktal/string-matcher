use crate::ranking::{
    AbbreviationRank, DistanceRank, SearchRank, StructuralRank, compare_search_results, ratio_milli,
};
use crate::text::{normalize_compact_alnum, split_search_tokens, to_lowercase};

pub type Score = f64;

#[derive(Clone, Copy, Debug)]
pub struct SearchField<'a> {
    pub priority: u8,
    pub value: &'a str,
}

#[derive(Clone, Copy, Debug)]
pub struct MetadataCandidate<'a> {
    pub key: &'a str,
    pub fields: &'a [SearchField<'a>],
    pub score: Score,
}

#[derive(Clone, Copy, Debug)]
pub struct MetadataMatch<'a> {
    pub candidate: MetadataCandidate<'a>,
    pub rank: &'a SearchRank,
}

pub struct MetadataQuery {
    query: String,
    allow_typo: bool,
}

impl MetadataQuery {
    pub fn new(query: &str) -> Option<Self> {
        let query = query.trim();
        if query.is_empty() {
            return None;
        }
        Some(Self {
            query: query.to_string(),
            allow_typo: false,
        })
    }

    pub fn with_typo_fallback(mut self, allow_typo: bool) -> Self {
        self.allow_typo = allow_typo;
        self
    }

    pub fn search_rank(&self, candidate: MetadataCandidate<'_>) -> Option<SearchRank> {
        if self.allow_typo {
            best_typo_rank(&self.query, candidate.fields).map(SearchRank::Typo)
        } else {
            best_structural_rank(&self.query, candidate.fields)
                .map(SearchRank::Structural)
                .or_else(|| {
                    best_abbreviation_rank(&self.query, candidate.fields)
                        .map(SearchRank::Abbreviation)
                })
                .or_else(|| best_fuzzy_rank(&self.query, candidate.fields).map(SearchRank::Fuzzy))
        }
    }
}

pub fn sort_matches(matches: &mut [(MetadataCandidate<'_>, SearchRank)]) {
    matches.sort_unstable_by(
        |(left_candidate, left_rank), (right_candidate, right_rank)| {
            compare_search_results(
                left_rank,
                left_candidate.score,
                left_candidate.key,
                right_rank,
                right_candidate.score,
                right_candidate.key,
            )
        },
    );
}

pub fn dedup_push_search_field<'a>(
    fields: &mut Vec<SearchField<'a>>,
    priority: u8,
    value: Option<&'a str>,
) {
    let Some(value) = value else {
        return;
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return;
    }
    if fields
        .iter()
        .any(|field| field.priority == priority && field.value.eq_ignore_ascii_case(trimmed))
    {
        return;
    }
    fields.push(SearchField {
        priority,
        value: trimmed,
    });
}

fn fuzzy_match_details(query: &str, target: &str) -> Option<(usize, usize)> {
    let query_lower = to_lowercase(query);
    let target_lower = to_lowercase(target);

    let q_len = query_lower.chars().count();
    if q_len == 0 {
        return Some((0, 0));
    }

    let t_len = target_lower.chars().count();
    let limit = q_len / 2;

    if t_len < q_len.saturating_sub(limit) {
        return None;
    }

    let inf = limit + 1;
    let query_chars: Vec<char> = query_lower.chars().collect();
    let target_chars: Vec<char> = target_lower.chars().collect();

    let mut prev_prev = vec![inf; t_len + 1];
    let mut prev = vec![0; t_len + 1];
    let mut curr = vec![inf; t_len + 1];

    for i in 1..=q_len {
        curr.fill(inf);
        curr[0] = i;

        let mut row_min = inf;
        for j in 1..=t_len {
            let cost = usize::from(query_chars[i - 1] != target_chars[j - 1]);
            let deletion = prev[j] + 1;
            let insertion = curr[j - 1] + 1;
            let substitution = prev[j - 1] + cost;
            let mut cell = deletion.min(insertion).min(substitution);

            if i > 1
                && j > 1
                && query_chars[i - 1] == target_chars[j - 2]
                && query_chars[i - 2] == target_chars[j - 1]
            {
                cell = cell.min(prev_prev[j - 2] + 1);
            }

            curr[j] = cell;
            row_min = row_min.min(cell);
        }

        if row_min > limit {
            return None;
        }

        std::mem::swap(&mut prev_prev, &mut prev);
        std::mem::swap(&mut prev, &mut curr);
    }

    let mut min_distance = inf;
    let mut best_end_idx = 0;
    let search_start = q_len.saturating_sub(limit).max(1);
    for (j, value) in prev.iter().enumerate().take(t_len + 1).skip(search_start) {
        if *value < min_distance {
            min_distance = *value;
            best_end_idx = j;
        }
    }

    (min_distance <= limit).then_some((min_distance, best_end_idx.saturating_sub(q_len)))
}

fn is_search_token_separator(c: char) -> bool {
    !c.is_alphanumeric()
}

fn structural_match_details(
    target: &str,
    start_byte_idx: usize,
    matched_char_len: usize,
) -> (u8, usize, usize, usize, usize) {
    let chars: Vec<char> = target.chars().collect();
    let start_idx = target[..start_byte_idx].chars().count();
    let end_idx = (start_idx + matched_char_len).min(chars.len());
    let start_boundary = start_idx == 0
        || chars
            .get(start_idx.saturating_sub(1))
            .is_none_or(|c| is_search_token_separator(*c));
    let end_boundary = end_idx == chars.len()
        || chars
            .get(end_idx)
            .is_none_or(|c| is_search_token_separator(*c));

    let mut token_start = start_idx;
    while token_start > 0 && !is_search_token_separator(chars[token_start - 1]) {
        token_start -= 1;
    }

    let mut token_end = end_idx;
    while token_end < chars.len() && !is_search_token_separator(chars[token_end]) {
        token_end += 1;
    }

    let mut token_index = 0usize;
    let mut in_token = false;
    for (idx, ch) in chars.iter().enumerate() {
        if is_search_token_separator(*ch) {
            in_token = false;
            continue;
        }
        if !in_token {
            if idx >= token_start {
                break;
            }
            token_index += 1;
            in_token = true;
        }
    }

    let position_class = if start_idx == 0 && end_idx == chars.len() {
        0
    } else if start_boundary && end_boundary {
        1
    } else if start_idx == 0 {
        2
    } else if start_boundary {
        3
    } else if end_boundary {
        4
    } else {
        5
    };

    (
        position_class,
        token_index,
        (token_end - token_start).abs_diff(matched_char_len),
        start_idx,
        chars.len(),
    )
}

fn best_structural_rank(query: &str, fields: &[SearchField<'_>]) -> Option<StructuralRank> {
    let query_lower = to_lowercase(query.trim());
    if query_lower.is_empty() {
        return None;
    }

    let query_len = query_lower.chars().count();
    let mut best_rank = None;

    for field in fields {
        let target = field.value.trim();
        if target.is_empty() {
            continue;
        }

        let target_lower = to_lowercase(target);
        for (start_byte_idx, _) in target_lower.match_indices(&query_lower) {
            let (position_class, token_index, token_span_delta, start_idx, field_len) =
                structural_match_details(&target_lower, start_byte_idx, query_len);
            let rank = StructuralRank {
                field_priority: field.priority,
                position_class,
                token_index,
                token_span_delta,
                start_idx,
                field_len,
            };
            if best_rank.as_ref().is_none_or(|current| rank < *current) {
                best_rank = Some(rank);
            }
        }
    }

    best_rank
}

fn subsequence_match_details(query: &str, target: &str) -> Option<(usize, usize, usize, usize)> {
    let query_chars: Vec<char> = query.chars().collect();
    let target_chars: Vec<char> = target.chars().collect();
    if query_chars.is_empty() || target_chars.is_empty() {
        return None;
    }

    let mut q_idx = 0usize;
    let mut first_idx = None;
    let mut last_idx = 0usize;
    let mut gap_count = 0usize;

    for (t_idx, ch) in target_chars.iter().enumerate() {
        if q_idx < query_chars.len() && *ch == query_chars[q_idx] {
            if let Some(prev_idx) = first_idx.or(Some(last_idx))
                && q_idx > 0
                && t_idx > prev_idx + 1
            {
                gap_count += 1;
            }
            first_idx.get_or_insert(t_idx);
            last_idx = t_idx;
            q_idx += 1;
            if q_idx == query_chars.len() {
                let start_idx = first_idx?;
                let span = last_idx - start_idx + 1;
                return Some((
                    start_idx,
                    last_idx,
                    span.saturating_sub(query_chars.len()),
                    gap_count,
                ));
            }
        }
    }

    None
}

fn best_abbreviation_rank(query: &str, fields: &[SearchField<'_>]) -> Option<AbbreviationRank> {
    let query_lower = to_lowercase(query.trim());
    if query_lower.is_empty() {
        return None;
    }

    let query_compact = normalize_compact_alnum(&query_lower);
    if query_compact.chars().count() < 2 {
        return None;
    }

    let mut best_rank = None;

    for field in fields {
        let target = field.value.trim();
        if target.is_empty() {
            continue;
        }

        let target_lower = to_lowercase(target);
        let tokens = split_search_tokens(&target_lower);
        let mut candidates: Vec<(u8, String)> = Vec::new();

        for token in &tokens {
            candidates.push((0, (*token).to_string()));
        }
        candidates.push((1, target_lower.clone()));
        if tokens.len() > 1 {
            let collapsed = tokens.concat();
            if !collapsed.is_empty() {
                candidates.push((2, collapsed));
            }
        }

        for (variant_scope, candidate_target) in candidates {
            let Some((start_idx, last_idx, gap_span, gap_count)) =
                subsequence_match_details(&query_compact, &candidate_target)
            else {
                continue;
            };
            if gap_span == 0 {
                continue;
            }
            let matched_span = last_idx - start_idx + 1;
            let (position_class, token_index, _, _, field_len) =
                structural_match_details(&candidate_target, start_idx, matched_span);
            let rank = AbbreviationRank {
                field_priority: field.priority,
                variant_scope,
                position_class,
                token_index,
                gap_span,
                gap_count,
                start_idx,
                field_len,
            };
            if best_rank.as_ref().is_none_or(|current| rank < *current) {
                best_rank = Some(rank);
            }
        }
    }

    best_rank
}

fn best_fuzzy_rank(query: &str, fields: &[SearchField<'_>]) -> Option<DistanceRank> {
    let query_lower = to_lowercase(query.trim());
    if query_lower.is_empty() {
        return None;
    }

    let query_collapsed = normalize_compact_alnum(&query_lower);
    let allow_collapsed =
        !query_collapsed.is_empty() && !query_lower.chars().any(is_search_token_separator);
    let mut best_rank = None;

    for field in fields {
        let target = field.value.trim();
        if target.is_empty() {
            continue;
        }

        let target_lower = to_lowercase(target);
        let tokens = split_search_tokens(&target_lower);
        let mut candidates: Vec<(u8, String, String)> =
            vec![(0, query_lower.clone(), target_lower.clone())];

        for token in &tokens {
            candidates.push((1, query_lower.clone(), (*token).to_string()));
        }

        if allow_collapsed && tokens.len() > 1 {
            let collapsed = tokens.concat();
            if !collapsed.is_empty() {
                candidates.push((2, query_collapsed.clone(), collapsed));
            }
        }

        for (variant_scope, candidate_query, candidate_target) in candidates {
            let Some((distance, start_idx)) =
                fuzzy_match_details(&candidate_query, &candidate_target)
            else {
                continue;
            };
            let compared_len = candidate_query
                .chars()
                .count()
                .max(candidate_target.chars().count())
                .max(1);
            let ratio_milli = ratio_milli(distance, compared_len);
            let matched_len = candidate_query.chars().count();
            let (position_class, token_index, token_span_delta, _, field_len) =
                structural_match_details(&candidate_target, start_idx, matched_len);
            let rank = DistanceRank {
                distance,
                ratio_milli,
                field_priority: field.priority,
                variant_scope,
                position_class,
                token_index,
                token_span_delta,
                start_idx,
                field_len,
            };
            if best_rank.as_ref().is_none_or(|current| rank < *current) {
                best_rank = Some(rank);
            }
        }
    }

    best_rank
}

fn best_typo_rank(query: &str, fields: &[SearchField<'_>]) -> Option<DistanceRank> {
    let query_lower = to_lowercase(query.trim());
    if query_lower.is_empty() {
        return None;
    }

    let query_collapsed = normalize_compact_alnum(&query_lower);
    let allow_collapsed =
        !query_collapsed.is_empty() && !query_lower.chars().any(is_search_token_separator);
    let mut best_rank = None;

    for field in fields {
        let target = field.value.trim();
        if target.is_empty() {
            continue;
        }

        let target_lower = to_lowercase(target);
        let tokens = split_search_tokens(&target_lower);
        let mut candidates: Vec<(u8, String, String)> =
            vec![(0, query_lower.clone(), target_lower.clone())];

        for token in &tokens {
            candidates.push((1, query_lower.clone(), (*token).to_string()));
        }

        if allow_collapsed && tokens.len() > 1 {
            let collapsed = tokens.concat();
            if !collapsed.is_empty() {
                candidates.push((2, query_collapsed.clone(), collapsed));
            }
        }

        for (variant_scope, candidate_query, candidate_target) in candidates {
            let Some((distance, start_idx)) =
                fuzzy_match_details(&candidate_query, &candidate_target)
            else {
                continue;
            };
            let compared_len = candidate_query
                .chars()
                .count()
                .max(candidate_target.chars().count())
                .max(1);
            let ratio_milli = ratio_milli(distance, compared_len);
            let matched_len = candidate_query.chars().count();
            let (position_class, token_index, token_span_delta, _, field_len) =
                structural_match_details(&candidate_target, start_idx, matched_len);
            let rank = DistanceRank {
                distance,
                ratio_milli,
                field_priority: field.priority,
                variant_scope,
                position_class,
                token_index,
                token_span_delta,
                start_idx,
                field_len,
            };
            if best_rank.as_ref().is_none_or(|current| rank < *current) {
                best_rank = Some(rank);
            }
        }
    }

    best_rank
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_name_field_beats_exec_field() {
        let fields = [
            SearchField {
                priority: 0,
                value: "Application Launcher",
            },
            SearchField {
                priority: 1,
                value: "applauncher",
            },
        ];
        let candidate = MetadataCandidate {
            key: "applicationlauncher.desktop",
            fields: &fields,
            score: 1.0,
        };
        let rank = MetadataQuery::new("app")
            .unwrap()
            .search_rank(candidate)
            .unwrap();
        match rank {
            SearchRank::Structural(rank) => assert_eq!(rank.field_priority, 0),
            _ => panic!("expected structural rank"),
        }
    }

    #[test]
    fn typo_fallback_matches_collapsed_identifier() {
        let fields = [SearchField {
            priority: 0,
            value: "Application Launcher",
        }];
        let candidate = MetadataCandidate {
            key: "applicationlauncher.desktop",
            fields: &fields,
            score: 1.0,
        };
        let rank = MetadataQuery::new("applaunch")
            .unwrap()
            .with_typo_fallback(true)
            .search_rank(candidate)
            .unwrap();
        match rank {
            SearchRank::Typo(rank) => {
                assert!(rank.distance <= 4);
                assert!(rank.variant_scope <= 2);
            }
            _ => panic!("expected typo rank"),
        }
    }

    #[test]
    fn window_title_beats_class_when_both_match() {
        let fields = [
            SearchField {
                priority: 0,
                value: "CopyQ - Clipboard",
            },
            SearchField {
                priority: 1,
                value: "copyq",
            },
        ];
        let candidate = MetadataCandidate {
            key: "window:1",
            fields: &fields,
            score: 1.0,
        };
        let rank = MetadataQuery::new("copyq")
            .unwrap()
            .search_rank(candidate)
            .unwrap();
        match rank {
            SearchRank::Structural(rank) => assert_eq!(rank.field_priority, 0),
            _ => panic!("expected structural rank"),
        }
    }

    #[test]
    fn sort_uses_rank_then_score_then_key() {
        let fields = [SearchField {
            priority: 0,
            value: "git status",
        }];
        let a = MetadataCandidate {
            key: "a",
            fields: &fields,
            score: 1.0,
        };
        let b = MetadataCandidate {
            key: "b",
            fields: &fields,
            score: 2.0,
        };
        let mut matches = vec![
            (
                a,
                MetadataQuery::new("git").unwrap().search_rank(a).unwrap(),
            ),
            (
                b,
                MetadataQuery::new("git").unwrap().search_rank(b).unwrap(),
            ),
        ];
        sort_matches(&mut matches);
        assert_eq!(matches[0].0.key, "b");
    }
}
