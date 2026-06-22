use std::fmt::{self, Display, Formatter};
use std::path;

use crate::core::{OperationProfile, bounded_damerau_levenshtein, max_typos};
use crate::ranking::{
    PathTypoSortKey, compare_path_typo_sort_keys, path_typo_keys_are_ambiguous, ratio_milli,
};
use crate::text::{has_query_signal, separator_variants, split_path_tokens, to_lowercase};
use crate::token_match::{
    PositionedToken, aligned_token_distance, best_token_match, partitioned_token_distance,
};

pub type Score = f64;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum MatchScope {
    Basename = 0,
    BasenameToken = 1,
    OtherComponent = 2,
    OtherComponentToken = 3,
    FullPath = 4,
}

impl Display for MatchScope {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Basename => write!(f, "basename"),
            Self::BasenameToken => write!(f, "basename-token"),
            Self::OtherComponent => write!(f, "component"),
            Self::OtherComponentToken => write!(f, "component-token"),
            Self::FullPath => write!(f, "path"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum MatchQuality {
    Contains = 0,
    Suffix = 1,
    Prefix = 2,
    Exact = 3,
}

#[derive(Clone, Copy, Debug)]
pub struct PathMatch<'a> {
    pub path: &'a str,
    pub distance: usize,
    pub ratio: f64,
    pub scope: MatchScope,
    pub path_position: usize,
    pub structure: usize,
    pub operations: OperationProfile,
    pub score: Score,
    pub path_depth: usize,
}

impl<'a> PathMatch<'a> {
    fn sort_key(&self) -> PathTypoSortKey<'_, MatchScope> {
        PathTypoSortKey {
            distance: self.distance,
            operations: self.operations,
            ratio_milli: ratio_milli(self.distance, self.compared_len()),
            scope: self.scope,
            position: self.path_position,
            structure: self.structure,
            score: self.score,
            path_depth: self.path_depth,
            key: self.path,
        }
    }

    fn compared_len(&self) -> usize {
        if self.ratio == 0.0 {
            1
        } else {
            ((self.distance as f64 / self.ratio).round() as usize).max(1)
        }
    }

    pub fn compare(&self, other: &Self) -> std::cmp::Ordering {
        compare_path_typo_sort_keys(&self.sort_key(), &other.sort_key())
    }
}

pub struct TypoQuery {
    text: String,
    len: usize,
    limit: usize,
    token_count: usize,
    separator_variants: Vec<String>,
}

impl TypoQuery {
    pub fn new(query: &str) -> Option<Self> {
        let len = query.chars().count();
        if len <= 1 {
            return None;
        }

        let token_count = split_path_tokens(query).len();
        Some(Self {
            text: query.to_owned(),
            len,
            limit: max_typos(len),
            token_count,
            separator_variants: if token_count == 1 {
                separator_variants(query)
            } else {
                Vec::new()
            },
        })
    }

    pub fn best_match<'a>(&self, path: &'a str, score: Score) -> Option<PathMatch<'a>> {
        best_match_with_query(path, score, self)
    }

    pub fn best_basename_match<'a>(&self, path: &'a str, score: Score) -> Option<PathMatch<'a>> {
        best_basename_match_with_query(path, score, self)
    }
}

pub fn best_match<'a>(path: &'a str, score: Score, query: &str) -> Option<PathMatch<'a>> {
    let query = TypoQuery::new(query)?;
    best_match_with_query(path, score, &query)
}

pub fn sort_matches(matches: &mut [PathMatch<'_>]) {
    matches.sort_unstable_by(PathMatch::compare);
}

pub fn is_ambiguous(first: &PathMatch<'_>, second: &PathMatch<'_>) -> bool {
    path_typo_keys_are_ambiguous(&first.sort_key(), &second.sort_key())
}

pub fn typo_query(keywords: &[String]) -> Option<String> {
    if keywords.is_empty() || keywords.iter().any(String::is_empty) {
        return None;
    }

    Some(
        keywords
            .iter()
            .map(to_lowercase)
            .collect::<Vec<_>>()
            .join(" "),
    )
}

pub fn match_penalty(path: &str, keywords: &[String]) -> Option<usize> {
    let (keywords_last, keywords) = match keywords.split_last() {
        Some(split) => split,
        None => return Some(0),
    };

    let path = to_lowercase(path);
    let mut path = path.as_str();
    let mut penalty = 0;

    let idx = path.rfind(keywords_last)?;
    if path[idx + keywords_last.len()..].contains(path::is_separator) {
        return None;
    }
    penalty += match_position_penalty(path, idx, keywords_last.len());
    path = &path[..idx];

    for keyword in keywords.iter().rev() {
        let idx = path.rfind(keyword)?;
        penalty += match_position_penalty(path, idx, keyword.len());
        path = &path[..idx];
    }

    Some(penalty)
}

pub fn match_path_position(path: &str, keywords: &[String]) -> Option<usize> {
    let (keywords_last, keywords) = match keywords.split_last() {
        Some(split) => split,
        None => return Some(0),
    };

    let full_path = to_lowercase(path);
    let mut search = full_path.as_str();
    let mut position = 0;

    let idx = search.rfind(keywords_last)?;
    if search[idx + keywords_last.len()..].contains(path::is_separator) {
        return None;
    }
    position += path_position_class(&full_path, idx, keywords_last.len());
    search = &search[..idx];

    for keyword in keywords.iter().rev() {
        let idx = search.rfind(keyword)?;
        position += path_position_class(&full_path, idx, keyword.len());
        search = &search[..idx];
    }

    Some(position)
}

pub fn match_qualities(path: &str, keywords: &[String]) -> Option<Vec<MatchQuality>> {
    let (keywords_last, keywords) = match keywords.split_last() {
        Some(split) => split,
        None => return Some(Vec::new()),
    };

    let path = to_lowercase(path);
    let mut path = path.as_str();
    let mut qualities = Vec::with_capacity(keywords.len() + 1);

    let idx = path.rfind(keywords_last)?;
    if path[idx + keywords_last.len()..].contains(path::is_separator) {
        return None;
    }
    qualities.push(match_quality(path, idx, keywords_last.len()));
    path = &path[..idx];

    for keyword in keywords.iter().rev() {
        let idx = path.rfind(keyword)?;
        qualities.push(match_quality(path, idx, keyword.len()));
        path = &path[..idx];
    }

    Some(qualities)
}

fn match_quality(path: &str, idx: usize, keyword_len: usize) -> MatchQuality {
    let end = idx + keyword_len;
    let starts_token = idx == 0
        || path[..idx]
            .chars()
            .next_back()
            .is_some_and(is_path_token_separator);
    let ends_token = end == path.len()
        || path[end..]
            .chars()
            .next()
            .is_some_and(is_path_token_separator);

    if starts_token && ends_token {
        MatchQuality::Exact
    } else if starts_token {
        MatchQuality::Prefix
    } else if ends_token {
        MatchQuality::Suffix
    } else {
        MatchQuality::Contains
    }
}

fn match_position_penalty(path: &str, idx: usize, keyword_len: usize) -> usize {
    let end = idx + keyword_len;
    let token_start = path[..idx]
        .rfind(is_path_token_separator)
        .map_or(0, |position| position + 1);
    let token_end = path[end..]
        .find(is_path_token_separator)
        .map_or(path.len(), |position| end + position);
    idx - token_start + (token_end - end)
}

fn path_position_class(path: &str, idx: usize, keyword_len: usize) -> usize {
    let component_distance = path[idx..]
        .chars()
        .filter(|ch| path::is_separator(*ch))
        .count();

    component_distance * 3 + position_rank(path, idx, keyword_len)
}

fn position_rank(path: &str, idx: usize, keyword_len: usize) -> usize {
    match match_quality(path, idx, keyword_len) {
        MatchQuality::Exact | MatchQuality::Prefix => 0,
        MatchQuality::Suffix => 1,
        MatchQuality::Contains => 2,
    }
}

fn is_path_token_separator(c: char) -> bool {
    matches!(c, '/' | '\\' | '-' | '_' | '.') || c.is_whitespace()
}

fn best_match_with_query<'a>(
    path: &'a str,
    score: Score,
    query: &TypoQuery,
) -> Option<PathMatch<'a>> {
    let mut best = best_match_inner(path, score, &query.text, query.len, query.limit);
    if query.token_count == 1 {
        for variant in &query.separator_variants {
            let Some(mut candidate) = best_match_inner(
                path,
                score,
                variant,
                query.len,
                query.limit.saturating_sub(1),
            ) else {
                continue;
            };
            let distance = candidate.distance + 1;
            if distance > query.limit {
                continue;
            }
            candidate.distance = distance;
            candidate.operations = candidate.operations.with_insert_delete();
            candidate.ratio = candidate.ratio.max(distance as f64 / query.len as f64);
            update_best(&mut best, Some(candidate));
        }
    }

    best
}

fn best_basename_match_with_query<'a>(
    path: &'a str,
    score: Score,
    query: &TypoQuery,
) -> Option<PathMatch<'a>> {
    let mut best = best_basename_match_inner(
        path,
        score,
        &query.text,
        query.len,
        query.limit,
        query.limit,
    );
    if query.token_count == 1 {
        for variant in &query.separator_variants {
            let Some(mut candidate) = best_basename_match_inner(
                path,
                score,
                variant,
                query.len,
                query.limit.saturating_sub(1),
                query.limit,
            ) else {
                continue;
            };
            let distance = candidate.distance + 1;
            if distance > query.limit {
                continue;
            }
            candidate.distance = distance;
            candidate.operations = candidate.operations.with_insert_delete();
            candidate.ratio = candidate.ratio.max(distance as f64 / query.len as f64);
            update_best(&mut best, Some(candidate));
        }
    }
    best
}

fn best_match_inner<'a>(
    path: &'a str,
    score: Score,
    query: &str,
    query_len: usize,
    limit: usize,
) -> Option<PathMatch<'a>> {
    let lower_path = to_lowercase(path);
    if !has_query_signal(split_path_tokens(query), &lower_path) {
        return None;
    }

    let components = path_components(&lower_path);
    if components.is_empty() {
        return None;
    }
    let path_depth = components.len();

    if split_path_tokens(query).len() == 1 {
        return best_match_single_token(
            path,
            score,
            query,
            query_len,
            limit,
            &lower_path,
            &components,
            path_depth,
        );
    }

    let basename = *components.last().unwrap();
    let basename_idx = components.len() - 1;
    let ctx = MatchContext {
        path,
        score,
        query,
        query_len,
        limit,
        path_depth,
    };

    let mut best = candidate_for_text(&ctx, basename, MatchScope::Basename, 0);
    update_best(
        &mut best,
        candidate_for_token_sequence(&ctx, basename, MatchScope::Basename, 0),
    );
    update_best(
        &mut best,
        candidate_for_compound_component(&ctx, basename, MatchScope::Basename, 0),
    );
    update_best(
        &mut best,
        candidate_for_component_sequence(&ctx, &components, MatchScope::OtherComponent),
    );

    for token in split_path_tokens(basename) {
        update_best(
            &mut best,
            candidate_for_text(&ctx, token, MatchScope::BasenameToken, 0),
        );
    }

    for (idx, component) in components[..components.len().saturating_sub(1)]
        .iter()
        .enumerate()
    {
        let path_position = basename_idx - idx;
        update_best(
            &mut best,
            candidate_for_text(&ctx, component, MatchScope::OtherComponent, path_position),
        );
        update_best(
            &mut best,
            candidate_for_compound_component(
                &ctx,
                component,
                MatchScope::OtherComponent,
                path_position,
            ),
        );
        for token in split_path_tokens(component) {
            update_best(
                &mut best,
                candidate_for_text(&ctx, token, MatchScope::OtherComponentToken, path_position),
            );
        }
    }

    update_best(
        &mut best,
        candidate_for_text(&ctx, &lower_path, MatchScope::FullPath, basename_idx + 1),
    );
    best
}

fn best_basename_match_inner<'a>(
    path: &'a str,
    score: Score,
    query: &str,
    query_len: usize,
    limit: usize,
    full_limit: usize,
) -> Option<PathMatch<'a>> {
    let lower_path = to_lowercase(path);
    if !has_query_signal(split_path_tokens(query), &lower_path) {
        return None;
    }

    let components = path_components(&lower_path);
    if components.is_empty() {
        return None;
    }

    let path_depth = components.len();
    let basename = *components.last().unwrap();
    let ctx = MatchContext {
        path,
        score,
        query,
        query_len,
        limit,
        path_depth,
    };
    let mut best = candidate_for_text(&ctx, basename, MatchScope::Basename, 0);
    for token in split_path_tokens(basename) {
        update_best(
            &mut best,
            candidate_for_text(&ctx, token, MatchScope::BasenameToken, 0),
        );
    }
    best.filter(|candidate| candidate.distance <= full_limit)
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
fn best_match_single_token<'a>(
    path: &'a str,
    score: Score,
    query: &str,
    query_len: usize,
    limit: usize,
    lower_path: &str,
    components: &[&str],
    path_depth: usize,
) -> Option<PathMatch<'a>> {
    let basename = *components.last().unwrap();
    let basename_idx = components.len() - 1;
    let ctx = MatchContext {
        path,
        score,
        query,
        query_len,
        limit,
        path_depth,
    };

    let mut best = candidate_for_text(&ctx, basename, MatchScope::Basename, 0);
    if best
        .as_ref()
        .is_some_and(|candidate| candidate.distance <= 1)
    {
        return best;
    }
    for token in split_path_tokens(basename) {
        update_best(
            &mut best,
            candidate_for_text(&ctx, token, MatchScope::BasenameToken, 0),
        );
    }
    if best
        .as_ref()
        .is_some_and(|candidate| candidate.distance <= 1)
    {
        return best;
    }

    for (idx, component) in components[..components.len().saturating_sub(1)]
        .iter()
        .enumerate()
    {
        let path_position = basename_idx - idx;
        update_best(
            &mut best,
            candidate_for_text(&ctx, component, MatchScope::OtherComponent, path_position),
        );
        for token in split_path_tokens(component) {
            update_best(
                &mut best,
                candidate_for_text(&ctx, token, MatchScope::OtherComponentToken, path_position),
            );
        }
    }

    update_best(
        &mut best,
        candidate_for_text(&ctx, lower_path, MatchScope::FullPath, basename_idx + 1),
    );
    best
}

fn update_best<'a>(best: &mut Option<PathMatch<'a>>, candidate: Option<PathMatch<'a>>) {
    match (best.as_ref(), candidate) {
        (_, None) => {}
        (None, Some(candidate)) => *best = Some(candidate),
        (Some(current), Some(candidate)) if candidate.compare(current).is_lt() => {
            *best = Some(candidate)
        }
        _ => {}
    }
}

struct MatchContext<'a, 'q> {
    path: &'a str,
    score: Score,
    query: &'q str,
    query_len: usize,
    limit: usize,
    path_depth: usize,
}

fn candidate_for_text<'a>(
    ctx: &MatchContext<'a, '_>,
    candidate: &str,
    scope: MatchScope,
    path_position: usize,
) -> Option<PathMatch<'a>> {
    if candidate.is_empty() {
        return None;
    }

    let candidate_len = candidate.chars().count();
    if ctx.limit == 0 || ctx.query_len.abs_diff(candidate_len) > ctx.limit {
        return None;
    }

    let (distance, operations) = bounded_damerau_levenshtein(ctx.query, candidate, ctx.limit)?;
    let max_len = ctx.query_len.max(candidate_len) as f64;
    let ratio = distance as f64 / max_len;
    if ratio > 0.5 {
        return None;
    }

    Some(PathMatch {
        path: ctx.path,
        distance,
        ratio,
        scope,
        path_position: path_position * 3,
        structure: 0,
        operations,
        score: ctx.score,
        path_depth: ctx.path_depth,
    })
}

fn candidate_for_token_sequence<'a>(
    ctx: &MatchContext<'a, '_>,
    candidate: &str,
    scope: MatchScope,
    path_position: usize,
) -> Option<PathMatch<'a>> {
    let query_tokens = split_path_tokens(ctx.query);
    let candidate_tokens = split_path_tokens(candidate);
    if query_tokens.len() < 2 || query_tokens.len() != candidate_tokens.len() {
        return None;
    }

    let mut distance = 0;
    let mut path_metric = 0;
    let mut structure = 0;
    let mut operations = OperationProfile::default();
    for (query_token, candidate_token) in query_tokens.into_iter().zip(candidate_tokens) {
        let remaining = ctx.limit.checked_sub(distance)?;
        let (cost, penalty, position_rank, _, token_operations) =
            best_token_match(query_token, candidate_token, remaining)?;
        distance += cost;
        path_metric += path_position * 3 + position_rank;
        structure += penalty;
        operations.substitutions += token_operations.substitutions;
        operations.insert_delete += token_operations.insert_delete;
        operations.transpositions += token_operations.transpositions;
    }

    let ratio = distance as f64 / ctx.query_len as f64;
    if ratio > 0.5 {
        return None;
    }

    Some(PathMatch {
        path: ctx.path,
        distance,
        ratio,
        scope,
        path_position: path_metric,
        structure,
        operations,
        score: ctx.score,
        path_depth: ctx.path_depth,
    })
}

fn candidate_for_compound_component<'a>(
    ctx: &MatchContext<'a, '_>,
    candidate: &str,
    scope: MatchScope,
    path_position: usize,
) -> Option<PathMatch<'a>> {
    let query_tokens = split_path_tokens(ctx.query);
    let candidate_tokens = split_path_tokens(candidate);
    if query_tokens.len() < 2 || candidate_tokens.len() != 1 {
        return None;
    }

    let (distance, structure, position_metric, operations) =
        partitioned_token_distance(&query_tokens, candidate, ctx.limit)?;
    let ratio = distance as f64 / ctx.query_len.max(candidate.chars().count()) as f64;
    if ratio > 0.5 {
        return None;
    }

    Some(PathMatch {
        path: ctx.path,
        distance,
        ratio,
        scope,
        path_position: path_position * 3 * query_tokens.len() + position_metric,
        structure,
        operations,
        score: ctx.score,
        path_depth: ctx.path_depth,
    })
}

fn candidate_for_component_sequence<'a>(
    ctx: &MatchContext<'a, '_>,
    components: &[&str],
    scope: MatchScope,
) -> Option<PathMatch<'a>> {
    let query_tokens = split_path_tokens(ctx.query);
    if query_tokens.len() < 2 {
        return None;
    }

    let basename_idx = components.len() - 1;
    let candidate_tokens: Vec<_> = components
        .iter()
        .enumerate()
        .flat_map(|(idx, component)| {
            let path_position = basename_idx - idx;
            split_path_tokens(component)
                .into_iter()
                .map(move |token| PositionedToken {
                    token,
                    position: path_position * 3,
                })
        })
        .collect();
    if candidate_tokens.len() < query_tokens.len() {
        return None;
    }

    let (distance, path_position, structure, operations) =
        aligned_token_distance(&query_tokens, &candidate_tokens, ctx.limit)?;
    let ratio = distance as f64 / ctx.query_len as f64;
    if ratio > 0.5 {
        return None;
    }

    Some(PathMatch {
        path: ctx.path,
        distance,
        ratio,
        scope,
        path_position,
        structure,
        operations,
        score: ctx.score,
        path_depth: ctx.path_depth,
    })
}

fn path_components(path: &str) -> Vec<&str> {
    path.split(['/', '\\'])
        .filter(|component| !component.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typo_matches_basename() {
        let candidate = best_match("/home/lewis/xfce4-terminal", 10.0, "xfce4-terinal").unwrap();
        assert_eq!(candidate.scope, MatchScope::Basename);
        assert_eq!(candidate.distance, 1);
    }

    #[test]
    fn basename_token_prefixes_do_not_count_as_typos() {
        let candidate = best_match("/home/lewis/xfce4-terminal", 10.0, "xzfce-ter").unwrap();
        assert_eq!(candidate.scope, MatchScope::Basename);
        assert_eq!(candidate.distance, 1);
    }

    #[test]
    fn spaced_query_tokens_can_match_single_compound_token() {
        let candidate = best_match("/home/lewis/applicationlauncher", 10.0, "app laucnh").unwrap();
        assert_eq!(candidate.scope, MatchScope::Basename);
        assert_eq!(candidate.path_position, 0);
        assert_eq!(candidate.distance, 1);
        assert!(candidate.structure > 0);
    }

    #[test]
    fn missing_space_query_can_match_single_compound_token() {
        let candidate = best_match("/home/lewis/applicationlauncher", 10.0, "applaunch").unwrap();
        assert_eq!(candidate.scope, MatchScope::Basename);
        assert_eq!(candidate.path_position, 0);
        assert_eq!(candidate.distance, 1);
        assert!(candidate.structure > 0);
    }

    #[test]
    fn spaced_query_tokens_can_match_component_sequence() {
        let candidate = best_match("/home/lewis/tasks/config", 10.0, "tasks cinfig").unwrap();
        assert_eq!(candidate.scope, MatchScope::OtherComponent);
        assert_eq!(candidate.path_position, 3);
        assert_eq!(candidate.distance, 1);
        assert_eq!(candidate.structure, 0);
    }

    #[test]
    fn structure_penalty_tracks_token_positions() {
        assert_eq!(
            best_token_match("cinfig", "redragonmouseconfig", 3),
            Some((
                1,
                13,
                1,
                6,
                OperationProfile {
                    substitutions: 1,
                    insert_delete: 0,
                    transpositions: 0
                },
            ))
        );
    }

    #[test]
    fn component_sequence_can_match_substring_inside_token() {
        let candidate = best_match(
            "/home/lewis/tasks/redragonmouseconfig",
            10.0,
            "tasks cinfig",
        )
        .unwrap();
        assert_eq!(candidate.scope, MatchScope::OtherComponent);
        assert_eq!(candidate.path_position, 4);
        assert_eq!(candidate.distance, 1);
        assert_eq!(candidate.structure, 13);
    }

    #[test]
    fn multiple_tokens_can_match_the_same_non_basename_component() {
        let candidate = best_match(
            "/home/lewis/Dev/applicationlauncher/target/release",
            10.0,
            "ap laun",
        )
        .unwrap();
        assert_eq!(candidate.scope, MatchScope::OtherComponent);
        assert_eq!(candidate.distance, 0);
        assert_eq!(candidate.path_position, 12);
    }

    #[test]
    fn fewer_substitutions_beat_missing_letters_at_equal_distance() {
        let mut matches = [
            best_match("/home/lewis/abdef", 10.0, "abcdef").unwrap(),
            best_match("/home/lewis/abqdef", 10.0, "abcdef").unwrap(),
        ];
        sort_matches(&mut matches);
        assert_eq!(matches[0].path, "/home/lewis/abdef");
        assert_eq!(matches[0].distance, matches[1].distance);
    }

    #[test]
    fn long_queries_use_half_length_typo_limit() {
        let candidate = best_match("/home/lewis/xfce4-terminal", 10.0, "xgce4-tremriianl").unwrap();
        assert_eq!(candidate.scope, MatchScope::Basename);
        assert_eq!(candidate.distance, 5);
    }

    #[test]
    fn typo_matches_component_token() {
        let candidate = best_match("/home/lewis/xfce4-terminal", 10.0, "x4ce4").unwrap();
        assert_eq!(candidate.scope, MatchScope::BasenameToken);
        assert_eq!(candidate.distance, 1);
    }

    #[test]
    fn five_character_queries_allow_half_length_typos() {
        let candidate = best_match("/home/lewis/xfce4-terminal", 10.0, "zgce4").unwrap();
        assert_eq!(candidate.scope, MatchScope::BasenameToken);
        assert_eq!(candidate.distance, 2);
        assert_eq!(candidate.ratio, 0.4);
    }

    #[test]
    fn single_character_queries_are_not_corrected() {
        assert!(best_match("/home/lewis/foobar", 10.0, "f").is_none());
    }

    #[test]
    fn short_queries_allow_one_typo() {
        let candidate = best_match("/home/lewis/foo", 10.0, "foa").unwrap();
        assert_eq!(candidate.distance, 1);
    }

    #[test]
    fn ambiguous_equal_distance_matches_are_rejected() {
        let mut matches = vec![
            best_match("/home/lewis/terminal", 10.0, "terminak").unwrap(),
            best_match("/home/lewis/terminap", 10.0, "terminak").unwrap(),
        ];
        sort_matches(&mut matches);
        assert!(is_ambiguous(&matches[0], &matches[1]));
    }

    #[test]
    fn frecency_resolves_otherwise_ambiguous_matches() {
        let mut matches = vec![
            best_match("/home/lewis/repos/xfce4-terminal", 20.0, "xgce4-tremriianl").unwrap(),
            best_match(
                "/home/lewis/Dev/config/xfce4-terminal",
                10.0,
                "xgce4-tremriianl",
            )
            .unwrap(),
        ];
        sort_matches(&mut matches);
        assert_eq!(matches[0].path, "/home/lewis/repos/xfce4-terminal");
        assert!(!is_ambiguous(&matches[0], &matches[1]));
    }

    #[test]
    fn basename_match_beats_parent_component_match() {
        let mut matches = vec![
            best_match("/home/xfce4/project", 100.0, "x4ce4").unwrap(),
            best_match("/home/lewis/xfce4-terminal", 1.0, "x4ce4").unwrap(),
        ];
        sort_matches(&mut matches);
        assert_eq!(matches[0].path, "/home/lewis/xfce4-terminal");
        assert_eq!(matches[0].scope, MatchScope::BasenameToken);
    }

    #[test]
    fn lower_edit_distance_beats_frecency() {
        let mut matches = vec![
            best_match("/home/lewis/xfce4-terminal", 1.0, "xfce4-terinal").unwrap(),
            best_match("/home/lewis/xfce4-terminals", 1000.0, "xfce4-terinal").unwrap(),
        ];
        sort_matches(&mut matches);
        assert_eq!(matches[0].path, "/home/lewis/xfce4-terminal");
    }

    #[test]
    fn frecency_breaks_ties_after_distance_ratio_and_scope() {
        let mut matches = vec![
            best_match("/tmp/xfce4-terminal", 1.0, "x4ce4").unwrap(),
            best_match("/var/xfce4-utility", 100.0, "x4ce4").unwrap(),
        ];
        sort_matches(&mut matches);
        assert_eq!(matches[0].path, "/var/xfce4-utility");
        assert_eq!(matches[0].distance, matches[1].distance);
        assert_eq!(matches[0].scope, matches[1].scope);
        assert_eq!(matches[0].ratio, matches[1].ratio);
    }

    #[test]
    fn match_penalty_sums_token_positions() {
        let keywords = ["asks", "onfig"];
        let keywords = keywords.into_iter().map(str::to_string).collect::<Vec<_>>();
        assert_eq!(
            match_penalty("/home/lewis/tasks/config", &keywords),
            Some(2)
        );
    }

    #[test]
    fn match_path_position_prefers_basename_over_parent_components() {
        let keywords = ["ap", "laun"];
        let keywords = keywords.into_iter().map(str::to_string).collect::<Vec<_>>();
        assert_eq!(
            match_path_position("/home/lewis/Dev/applicationlauncher", &keywords),
            Some(2)
        );
        assert_eq!(
            match_path_position(
                "/home/lewis/Dev/applicationlauncher/target/release",
                &keywords
            ),
            None
        );
    }
}
