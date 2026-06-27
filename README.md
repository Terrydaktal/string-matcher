# Fuzzy Rank

`fuzzy-rank` is a Rust library for typo-tolerant matching and ranking across several kinds of searchable data.

It is structured as:

- one shared edit-distance engine
- one shared ranking/comparison layer
- one shared text/query-preparation layer
- one shared token-matching layer
- a small set of domain adapters on top

The goal is to keep the hard parts implemented once, while still allowing different match semantics for:

- filesystem paths
- shell commands
- application and window metadata
- chat and message text

## Project Structure

```text
fuzzy-rank/
├── Cargo.toml
├── README.md
└── src
    ├── lib.rs
    ├── core.rs
    ├── ranking.rs
    ├── text.rs
    ├── token_match.rs
    ├── path.rs
    ├── path/
    │   ├── exact.rs
    │   └── typo.rs
    ├── command.rs
    ├── message.rs
    └── metadata.rs
```

## Ownership Boundaries

The crate is intentionally split into shared layers and adapter layers.

### Shared layers

These modules are intended to be reused by all adapters:

- `core`
- `ranking`
- `text`
- `token_match`

They own the generic machinery. They do not own path-specific, command-specific, or metadata-specific policy.

### Adapter layers

These modules are domain-specific adapters:

- `path`
- `command`
- `message`
- `metadata`

They own the rules that differ by data type:

- which parts of a candidate are compared
- how position is interpreted
- which structural penalties matter
- how candidate fields are constructed
- what “stronger” means within that domain

They should stay thin. They are allowed to define policy, but they should consume shared utilities rather than reimplementing edit distance, token scanning, or generic ranking comparisons.

## What Is Shared

The following behavior is shared across adapters.

### `core.rs`

Owns the low-level edit-distance engine:

- bounded Damerau-Levenshtein
- `OperationProfile`
- typo-limit policy via `max_typos`

This module is responsible for:

- counting substitutions, insert/delete operations, and transpositions
- supporting bounded early-exit behavior
- providing a consistent raw typo distance to all adapters

This module does not know anything about:

- paths
- basename vs dirname
- command argv position
- app/window field priority
- frecency calculation

### `ranking.rs`

Owns shared ranking and comparison primitives:

- `StructuralRank`
- `DistanceRank`
- `AbbreviationRank`
- `SearchRank`
- `TypoSortKey`
- `PathTypoSortKey`
- comparison helpers such as:
  - `compare_search_results`
  - `compare_typo_sort_keys`
  - `compare_path_typo_sort_keys`
  - ambiguity helpers
  - `ratio_milli`

This module is responsible for:

- centralizing tuple ordering
- keeping rank precedence consistent
- avoiding duplicated sort-key logic across adapters

This module does not decide:

- what a field is
- what a token is
- what counts as basename or parent directory
- which metadata field should be priority 0

### `text.rs`

Owns shared text preparation helpers:

- lowercasing
- missing-space separator variant generation
- token splitting
- compact alphanumeric normalization
- cheap signal/prefilter checks

Current helpers include:

- `to_lowercase`
- `separator_variants`
- `token_has_signal`
- `has_query_signal`
- `split_whitespace_tokens`
- `split_search_tokens`
- `split_path_tokens`
- `normalize_compact_alnum`

This module is responsible for cheap, reusable preprocessing.

It does not decide final ranking policy.

### `token_match.rs`

Owns shared token-level typo matching helpers:

- `PositionedToken`
- `best_token_match`
- `aligned_token_distance`
- `partitioned_token_distance`

This module is responsible for:

- comparing query tokens against candidate tokens
- aligning token sequences
- matching multiple query tokens inside a single candidate token
- returning shared token-level distance, position, and structure outputs

It does not decide domain meaning for the resulting position numbers.

## What Is Not Shared

The following behavior remains adapter-specific on purpose.

### Frecency calculation

This crate accepts numeric `score` inputs, but does not compute them.

Examples:

- zoxide computes directory frecency itself, then passes `score` into `path`
- a shell-history consumer should compute command history frecency itself, then pass `score` into `command`
- an application launcher should compute app/window recency or launch counts itself, then pass `score` into `metadata`

The crate uses scores in ranking, but it does not define one universal frecency model.

### Candidate extraction

This crate does not fetch or index application records, shell history, or path databases.

Each caller is responsible for:

- loading its own data
- building candidates
- deciding which fields to expose
- deciding the score value for each candidate

### Domain semantics

The adapters intentionally do not share exact semantics, because the data is different.

Examples:

- path search cares about basename vs parent components
- command search cares about executable vs later argv tokens
- message search cares about phrase continuity and token coverage
- metadata search cares about field priority such as title vs class vs description

These are not accidental differences. They are the reason the adapters exist.

## Adapters

## `path.rs`

Purpose:

- split exact-path and typo-path matching
- basename/dirname-aware ranking
- shared top-level namespace for path semantics

This adapter is intended for zoxide-style search.

It owns:

- `path::exact`
- `path::typo`

It uses shared core from:

- `path::exact` uses `text` for path normalization and token-boundary logic
- `path::typo` uses:
  - `core` for edit distance
  - `ranking` for typo sort-key comparison
  - `text` for path-token splitting and query variants
  - `token_match` for token alignment and compound-token matching

What is path-specific here:

- basename outranking parent components
- parent-depth-sensitive `path_position`
- path token separators such as `/`, `\\`, `-`, `_`, `.`, and whitespace

This is the adapter to use for filesystem path search.

### `path::exact`

`path::exact` owns the normal non-typo path helpers:

- `match_qualities`
- `match_path_position`
- `match_penalty`
- `exact_match`
- `compare_exact_path_matches`

The exact-path comparator now lives in the crate.
Its sort-key order is:

1. descending `score`
2. `position`
3. `structure`
4. `path`

The exact-path helpers effectively expose these keys:

1. keyword matchability through `match_qualities`
2. summed path position through `match_path_position`
3. summed structural penalty through `match_penalty`
4. caller-provided `score`

### `path::typo`

`path::typo` owns:

- `TypoQuery`
- `PathMatch`
- `MatchScope`
- `query_from_keywords`

Typo path matching uses the shared `PathTypoSortKey` comparator from `ranking.rs`.
The exact typo sort key order is:

1. `distance`
2. `operations`
3. `ratio_milli`
4. `position`
5. descending `score`
6. `structure`
7. `path_depth`
8. `key`

Shared versus adapter-local:

- shared:
  - `distance`
  - `operations`
  - `ratio_milli`
  - the tuple ordering in `compare_path_typo_sort_keys`
- adapter-local:
  - `position`
  - `structure`
  - `path_depth`
  - normal-path helper logic

## `command.rs`

Purpose:

- typo-tolerant command-string matching
- executable-first command ranking
- command-history style matching

This adapter is intended for fish-history-like or command-palette-like search.

It owns:

- `CommandQuery`
- `CommandCandidate`
- `CommandMatch`
- `parse_command_candidate`

It uses shared core from:

- `core` for edit distance
- `ranking` for typo sort-key comparison
- `text` for whitespace tokenization and missing-space variants
- `token_match` for aligned and partitioned token matching

What is command-specific here:

- executable token is stronger than later arguments
- earlier argv positions beat later argv positions
- command position is interpreted as token order rather than path depth

This is the adapter to use for shell command history or command-string search.

### Exact command sort order

`command` uses the shared `TypoSortKey` comparator from `ranking.rs`.
The exact command sort key order is:

1. `distance`
2. `operations`
3. `ratio_milli`
4. `position`
5. `structure`
6. descending `score`
7. `secondary`
8. `key`

Where:

- `position` is command-specific token position, with earlier and stronger command-token matches ranking better
- `structure` is the command adapter’s token-span looseness metric
- `secondary` is currently the command token count

Shared versus adapter-local:

- shared:
  - `distance`
  - `operations`
  - `ratio_milli`
  - the tuple ordering in `compare_typo_sort_keys`
- adapter-local:
  - `position`
  - `structure`
  - `secondary`

## `message.rs`

Purpose:

- tokenized text matching for chat logs, notes, or message history
- phrase-aware ordering for exact multi-token sequences
- lightweight precomputation for repeated query evaluation

This adapter is intended for message search where coverage and phrase continuity
matter more than typo-distance ranking.

It owns:

- `MessageQuery`
- `MessageCandidate`
- `PreparedMessageCandidate`
- `MessageMatch`
- `contains_query_signal`

It uses shared core from:

- `text` for lowercasing and token splitting

What is message-specific here:

- phrase matches outrank split-token matches
- broader token coverage outranks repeated occurrences of only one query term
- repeated query-term occurrences refine ties after phrase coverage

Final message result ordering is:

1. phrase presence
2. matched query-term count
3. phrase occurrence count
4. total matched-term occurrences
5. descending `score`
6. `key`

This is the adapter to use for:

- chat messages
- notes or snippets
- searchable conversation history
- other single-text records where phrase continuity matters

## `metadata.rs`

Purpose:

- multi-field matching for records like applications and windows
- field-priority-aware structural, abbreviation, fuzzy, and typo ranking

This adapter is intended for application launchers and window search.

It owns:

- `MetadataQuery`
- `MetadataCandidate`
- `SearchField`
- `MetadataMatch`
- `dedup_push_search_field`

It uses shared core from:

- `ranking` for structural, fuzzy, abbreviation, and typo rank comparison
- `text` for general search tokenization and normalized key cleanup

It currently keeps more logic locally than `path` and `command`, because metadata search has richer field-aware policy.

What is metadata-specific here:

- field priorities such as title/name/class/description
- choosing between structural, abbreviation, fuzzy, and typo modes
- ranking across multiple independent fields rather than one path or argv sequence

This is the adapter to use for:

- application names
- desktop-file metadata
- window titles
- WM class / instance values
- other structured multi-field records

### Exact metadata sort order

`metadata` uses the shared `SearchRank` ordering plus the shared
`compare_search_results` comparator from `ranking.rs`.

Final metadata result ordering is:

1. `SearchRank`
2. descending `score`
3. `key`

`SearchRank` itself has a fixed shared kind precedence:

1. `Structural`
2. `Abbreviation`
3. `Fuzzy`
4. `Typo`

Within each kind, the exact sort keys are:

`StructuralRank`:

1. `field_priority`
2. `position_class`
3. `token_index`
4. `token_span_delta`
5. `start_idx`
6. `field_len`

`AbbreviationRank`:

1. `field_priority`
2. `variant_scope`
3. `position_class`
4. `token_index`
5. `gap_span`
6. `gap_count`
7. `start_idx`
8. `field_len`

`DistanceRank` for both `Fuzzy` and `Typo`:

1. `distance`
2. `ratio_milli`
3. `field_priority`
4. `variant_scope`
5. `position_class`
6. `token_index`
7. `token_span_delta`
8. `start_idx`
9. `field_len`

Shared versus adapter-local:

- shared:
  - `SearchRank`
  - `StructuralRank`
  - `AbbreviationRank`
  - `DistanceRank`
  - the kind precedence and final comparator in `compare_search_results`
- adapter-local:
  - how fields are built
  - what `field_priority` means
  - what `variant_scope` means
  - how structural, abbreviation, fuzzy, and typo candidates are generated
  - some of the metadata matching logic is still local and richer than the other adapters

## Which Adapter To Use

- Use `path` for filesystem paths.
- Use `command` for shell commands or command history lines.
- Use `message` for tokenized free-text records where phrase continuity matters.
- Use `metadata` for apps, windows, or generic multi-field searchable records.

If the data is not naturally a path, do not force it through `path`.
If the data is not naturally a command line, do not force it through `command`.

## Current Shared/Core Story

The current design is:

1. `core` provides one edit-distance engine.
2. `ranking` provides one ranking/comparison framework.
3. `text` provides one shared text/query-prep layer.
4. `token_match` provides one shared token-matching layer.
5. `path`, `command`, `message`, and `metadata` adapt those shared layers to their own domains.

So the crate does not have separate ad hoc matcher implementations anymore.
It has:

- one shared engine
- shared ranking helpers
- shared text helpers
- shared token-level matching helpers
- several domain adapters that intentionally keep their own semantics

## Shared Keys vs Adapter Keys

The crate does not currently share every ranking key end-to-end.

What is fully shared:

- raw edit distance
- operation-profile tie-breaks
- ratio normalization
- the tuple ordering for:
  - `TypoSortKey`
  - `PathTypoSortKey`
  - `StructuralRank`
  - `AbbreviationRank`
  - `DistanceRank`
  - `SearchRank`

What is not fully shared:

- the meaning of `position`
- the meaning of `structure`
- field construction
- some normal non-typo path ordering helpers
- some metadata-specific candidate generation and matching logic

So the shared ranking layer standardizes tuple comparison, but the adapters still supply several of the values inside those tuples.

## Inputs And Outputs

### Inputs

Each adapter expects:

- a user query
- a domain-shaped candidate
- an externally computed score if ranking should consider recency/frequency

### Outputs

Each adapter returns an adapter-specific match type or rank, for example:

- `PathMatch`
- `CommandMatch`
- `SearchRank`

Those outputs are intentionally not fully unified because the caller usually needs domain-specific details.

## Caller-Owned Priority

This library separates textual relevance from product-specific priority.

The matcher owns:

- textual ranking
- typo distance
- structural ranking
- field priority within a candidate
- deterministic comparison of match quality

The caller owns:

- pins
- favorites
- app type or category priority
- history/frecency policy
- browse order when no query is active

For example, an application launcher may want:

- pinned apps above normal apps
- normal apps above settings/system modules
- earlier pin positions above later pin positions

Those are not string-matching concepts, so they should not be hardcoded into `fuzzy-rank`.

Instead, callers should express those rules through the candidate `score` they pass in, or through separate non-search ordering when search is empty.

That means the intended split is:

- `fuzzy-rank` decides how good the text match is
- the caller decides how important the candidate is after textual rank ties or near-ties

This is why concepts like `pinned` or `system app` are not explicit library parameters today.
They are application policy, not shared matching policy.

## Design Rule

When adding new functionality:

- put generic edit logic in `core`
- put generic comparison logic in `ranking`
- put generic text preparation in `text`
- put generic token-level alignment/matching in `token_match`
- keep only domain semantics in adapters

If a helper would make sense for both `path` and `command`, it probably does not belong in either of them.
