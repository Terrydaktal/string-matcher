use std::cmp::Ordering;

use crate::core::OperationProfile;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructuralRank {
    pub field_priority: u8,
    pub position_class: u8,
    pub token_index: usize,
    pub token_span_delta: usize,
    pub start_idx: usize,
    pub field_len: usize,
}

impl Ord for StructuralRank {
    fn cmp(&self, other: &Self) -> Ordering {
        (
            self.field_priority,
            self.position_class,
            self.token_index,
            self.token_span_delta,
            self.start_idx,
            self.field_len,
        )
            .cmp(&(
                other.field_priority,
                other.position_class,
                other.token_index,
                other.token_span_delta,
                other.start_idx,
                other.field_len,
            ))
    }
}

impl PartialOrd for StructuralRank {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DistanceRank {
    pub distance: usize,
    pub ratio_milli: usize,
    pub field_priority: u8,
    pub variant_scope: u8,
    pub position_class: u8,
    pub token_index: usize,
    pub token_span_delta: usize,
    pub start_idx: usize,
    pub field_len: usize,
}

impl Ord for DistanceRank {
    fn cmp(&self, other: &Self) -> Ordering {
        (
            self.distance,
            self.ratio_milli,
            self.field_priority,
            self.variant_scope,
            self.position_class,
            self.token_index,
            self.token_span_delta,
            self.start_idx,
            self.field_len,
        )
            .cmp(&(
                other.distance,
                other.ratio_milli,
                other.field_priority,
                other.variant_scope,
                other.position_class,
                other.token_index,
                other.token_span_delta,
                other.start_idx,
                other.field_len,
            ))
    }
}

impl PartialOrd for DistanceRank {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbbreviationRank {
    pub field_priority: u8,
    pub variant_scope: u8,
    pub position_class: u8,
    pub token_index: usize,
    pub gap_span: usize,
    pub gap_count: usize,
    pub start_idx: usize,
    pub field_len: usize,
}

impl Ord for AbbreviationRank {
    fn cmp(&self, other: &Self) -> Ordering {
        (
            self.field_priority,
            self.variant_scope,
            self.position_class,
            self.token_index,
            self.gap_span,
            self.gap_count,
            self.start_idx,
            self.field_len,
        )
            .cmp(&(
                other.field_priority,
                other.variant_scope,
                other.position_class,
                other.token_index,
                other.gap_span,
                other.gap_count,
                other.start_idx,
                other.field_len,
            ))
    }
}

impl PartialOrd for AbbreviationRank {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchRank {
    Structural(StructuralRank),
    Abbreviation(AbbreviationRank),
    Fuzzy(DistanceRank),
    Typo(DistanceRank),
}

impl Ord for SearchRank {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (SearchRank::Structural(a), SearchRank::Structural(b)) => a.cmp(b),
            (SearchRank::Structural(_), _) => Ordering::Less,
            (_, SearchRank::Structural(_)) => Ordering::Greater,
            (SearchRank::Abbreviation(a), SearchRank::Abbreviation(b)) => a.cmp(b),
            (SearchRank::Abbreviation(_), _) => Ordering::Less,
            (_, SearchRank::Abbreviation(_)) => Ordering::Greater,
            (SearchRank::Fuzzy(a), SearchRank::Fuzzy(b)) => a.cmp(b),
            (SearchRank::Fuzzy(_), SearchRank::Typo(_)) => Ordering::Less,
            (SearchRank::Typo(_), SearchRank::Fuzzy(_)) => Ordering::Greater,
            (SearchRank::Typo(a), SearchRank::Typo(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for SearchRank {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub fn compare_search_results(
    left_rank: &SearchRank,
    left_score: f64,
    left_key: &str,
    right_rank: &SearchRank,
    right_score: f64,
    right_key: &str,
) -> Ordering {
    left_rank
        .cmp(right_rank)
        .then_with(|| right_score.total_cmp(&left_score))
        .then_with(|| left_key.cmp(right_key))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypoSortKey<'a> {
    pub distance: usize,
    pub operations: OperationProfile,
    pub ratio_milli: usize,
    pub position: usize,
    pub structure: usize,
    pub score: f64,
    pub secondary: usize,
    pub key: &'a str,
}

pub fn compare_typo_sort_keys(left: &TypoSortKey<'_>, right: &TypoSortKey<'_>) -> Ordering {
    left.distance
        .cmp(&right.distance)
        .then_with(|| left.operations.cmp(&right.operations))
        .then_with(|| left.ratio_milli.cmp(&right.ratio_milli))
        .then_with(|| left.position.cmp(&right.position))
        .then_with(|| left.structure.cmp(&right.structure))
        .then_with(|| right.score.total_cmp(&left.score))
        .then_with(|| left.secondary.cmp(&right.secondary))
        .then_with(|| left.key.cmp(right.key))
}

pub fn typo_keys_are_ambiguous(left: &TypoSortKey<'_>, right: &TypoSortKey<'_>) -> bool {
    left.distance == right.distance
        && left.position == right.position
        && left.structure == right.structure
        && left.ratio_milli.abs_diff(right.ratio_milli) <= 20
        && left.score.total_cmp(&right.score).is_eq()
}

pub fn ratio_milli(distance: usize, compared_len: usize) -> usize {
    distance * 1000 / compared_len.max(1)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathTypoSortKey<'a, Scope> {
    pub distance: usize,
    pub operations: OperationProfile,
    pub ratio_milli: usize,
    pub scope: Scope,
    pub position: usize,
    pub structure: usize,
    pub score: f64,
    pub path_depth: usize,
    pub key: &'a str,
}

pub fn compare_path_typo_sort_keys<Scope: Ord>(
    left: &PathTypoSortKey<'_, Scope>,
    right: &PathTypoSortKey<'_, Scope>,
) -> Ordering {
    left.distance
        .cmp(&right.distance)
        .then_with(|| left.operations.cmp(&right.operations))
        .then_with(|| left.ratio_milli.cmp(&right.ratio_milli))
        .then_with(|| left.scope.cmp(&right.scope))
        .then_with(|| left.position.cmp(&right.position))
        .then_with(|| left.structure.cmp(&right.structure))
        .then_with(|| right.score.total_cmp(&left.score))
        .then_with(|| left.path_depth.cmp(&right.path_depth))
        .then_with(|| left.key.cmp(right.key))
}

pub fn path_typo_keys_are_ambiguous<Scope: PartialEq>(
    left: &PathTypoSortKey<'_, Scope>,
    right: &PathTypoSortKey<'_, Scope>,
) -> bool {
    left.distance == right.distance
        && left.scope == right.scope
        && left.position == right.position
        && left.structure == right.structure
        && left.ratio_milli.abs_diff(right.ratio_milli) <= 20
        && left.score.total_cmp(&right.score).is_eq()
}
