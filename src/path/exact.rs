use std::path;

use crate::text::to_lowercase;

pub type Score = f64;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum MatchQuality {
    Contains = 0,
    Suffix = 1,
    Prefix = 2,
    Exact = 3,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExactPathMatch<'a> {
    pub path: &'a str,
    pub score: Score,
    pub position: usize,
    pub structure: usize,
}

pub fn exact_match<'a>(
    path: &'a str,
    keywords: &[String],
    score: Score,
) -> Option<ExactPathMatch<'a>> {
    Some(ExactPathMatch {
        path,
        score,
        position: match_path_position(path, keywords)?,
        structure: match_penalty(path, keywords)?,
    })
}

pub fn compare_exact_path_matches(
    left: ExactPathMatch<'_>,
    right: ExactPathMatch<'_>,
) -> std::cmp::Ordering {
    right
        .score
        .total_cmp(&left.score)
        .then_with(|| left.position.cmp(&right.position))
        .then_with(|| left.structure.cmp(&right.structure))
        .then_with(|| left.path.cmp(right.path))
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn compare_exact_path_matches_prefers_score_then_position_then_structure() {
        let keywords = ["tasks", "onfig"];
        let keywords = keywords.into_iter().map(str::to_string).collect::<Vec<_>>();
        let lower_structure = exact_match("/home/lewis/tasks/config", &keywords, 10.0).unwrap();
        let higher_structure =
            exact_match("/home/lewis/tasks/redragonmouseconfig", &keywords, 10.0).unwrap();
        let higher_score = exact_match("/home/lewis/tasks/config", &keywords, 20.0).unwrap();

        assert_eq!(
            compare_exact_path_matches(higher_score, lower_structure),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_exact_path_matches(lower_structure, higher_structure),
            std::cmp::Ordering::Less
        );
    }
}
