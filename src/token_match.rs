use crate::core::{OperationProfile, bounded_damerau_levenshtein};

#[derive(Clone, Copy)]
pub struct PositionedToken<'a> {
    pub token: &'a str,
    pub position: usize,
}

pub fn aligned_token_distance(
    query_tokens: &[&str],
    candidate_tokens: &[PositionedToken<'_>],
    limit: usize,
) -> Option<(usize, usize, usize, OperationProfile)> {
    if candidate_tokens.len() < query_tokens.len() {
        return None;
    }

    let mut best = None;
    for start in 0..=candidate_tokens.len() - query_tokens.len() {
        let mut distance = 0;
        let mut position = 0;
        let mut structure = 0;
        let mut operations = OperationProfile::default();
        let mut failed = false;

        for (query_token, candidate) in query_tokens
            .iter()
            .zip(candidate_tokens[start..start + query_tokens.len()].iter())
        {
            let Some(remaining) = limit.checked_sub(distance) else {
                failed = true;
                break;
            };
            let Some((cost, penalty, position_rank, _, token_operations)) =
                best_token_match(query_token, candidate.token, remaining)
            else {
                failed = true;
                break;
            };
            distance += cost;
            position += candidate.position + position_rank;
            structure += penalty;
            operations.substitutions += token_operations.substitutions;
            operations.insert_delete += token_operations.insert_delete;
            operations.transpositions += token_operations.transpositions;
            if distance > limit {
                failed = true;
                break;
            }
        }

        if failed {
            continue;
        }

        let total = (distance, position, structure, operations);
        best = Some(match best {
            None => total,
            Some(current) if total < current => total,
            Some(current) => current,
        });
        if best == Some((0, 0, 0, OperationProfile::default())) {
            break;
        }
    }

    best
}

pub fn partitioned_token_distance(
    query_tokens: &[&str],
    candidate: &str,
    limit: usize,
) -> Option<(usize, usize, usize, OperationProfile)> {
    let mut boundaries: Vec<usize> = candidate.char_indices().map(|(idx, _)| idx).collect();
    boundaries.push(candidate.len());
    if boundaries.len() <= query_tokens.len() {
        return None;
    }

    partitioned_token_distance_impl(query_tokens, candidate, &boundaries, 0, 0, limit)
}

fn partitioned_token_distance_impl(
    query_tokens: &[&str],
    candidate: &str,
    boundaries: &[usize],
    token_idx: usize,
    start_boundary: usize,
    remaining: usize,
) -> Option<(usize, usize, usize, OperationProfile)> {
    let last_token = token_idx + 1 == query_tokens.len();
    let min_end_boundary = start_boundary + 1;
    let max_end_boundary = boundaries.len() - (query_tokens.len() - token_idx);
    let mut best = None;

    for end_boundary in min_end_boundary..=max_end_boundary {
        if !last_token && end_boundary == boundaries.len() - 1 {
            break;
        }

        let end_boundary = if last_token {
            boundaries.len() - 1
        } else {
            end_boundary
        };
        let segment = &candidate[boundaries[start_boundary]..boundaries[end_boundary]];
        let (cost, structure, position_rank, _, operations) =
            match best_token_match(query_tokens[token_idx], segment, remaining) {
                Some(values) => values,
                None => continue,
            };
        if cost > remaining {
            continue;
        }

        let total = if last_token {
            (cost, structure, position_rank, operations)
        } else {
            let Some((tail_cost, tail_structure, tail_position, tail_operations)) =
                partitioned_token_distance_impl(
                    query_tokens,
                    candidate,
                    boundaries,
                    token_idx + 1,
                    end_boundary,
                    remaining - cost,
                )
            else {
                continue;
            };
            (
                cost + tail_cost,
                structure + tail_structure,
                position_rank + tail_position,
                OperationProfile {
                    substitutions: operations.substitutions + tail_operations.substitutions,
                    insert_delete: operations.insert_delete + tail_operations.insert_delete,
                    transpositions: operations.transpositions + tail_operations.transpositions,
                },
            )
        };

        best = Some(match best {
            None => total,
            Some(current) if total < current => total,
            Some(current) => current,
        });
        if best == Some((0, 0, 0, OperationProfile::default())) {
            break;
        }

        if last_token {
            break;
        }
    }

    best
}

pub fn best_token_match(
    query: &str,
    candidate: &str,
    limit: usize,
) -> Option<(usize, usize, usize, usize, OperationProfile)> {
    if query.is_empty() || candidate.is_empty() {
        return None;
    }

    let candidate_len = candidate.chars().count();
    let query_len = query.chars().count();
    let mut boundaries: Vec<usize> = candidate.char_indices().map(|(idx, _)| idx).collect();
    boundaries.push(candidate.len());

    let mut best = None;
    for start in 0..candidate_len {
        let min_len = 1usize.max(query_len.saturating_sub(limit));
        let max_len = (query_len + limit).min(candidate_len - start);
        for len in min_len..=max_len {
            let end = start + len;
            let segment = &candidate[boundaries[start]..boundaries[end]];
            let Some((distance, operations)) = bounded_damerau_levenshtein(query, segment, limit)
            else {
                continue;
            };
            let penalty = start + (candidate_len - end);
            let position_rank = if start == 0 {
                0
            } else if end == candidate_len {
                1
            } else {
                2
            };
            let total = (distance, penalty, position_rank, len, operations);
            best = Some(match best {
                None => total,
                Some(current) if total < current => total,
                Some(current) => current,
            });
            if best == Some((0, 0, 0, candidate_len, OperationProfile::default())) {
                return best;
            }
        }
    }

    best
}
