//! Deterministic, local-first retrieval over a VEXA index snapshot.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use vexa_types::{FileRecord, GitFileState, IndexSnapshot, ScoreBreakdown, SearchHit, SymbolKind};

const K1: f64 = 1.2;
const B: f64 = 0.75;
const EXPANSION_CONFIDENCE_THRESHOLD: f64 = 0.62;
const MAX_EXPANSION_CANDIDATES: usize = 8;

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub hits: Vec<SearchHit>,
    pub confidence: f64,
    pub needs_expansion: bool,
    pub expansion_candidates: Vec<String>,
    pub alternative_candidates: Vec<String>,
}

#[must_use]
pub fn search(
    snapshot: &IndexSnapshot,
    query: &str,
    diagnostics: &[String],
    limit: usize,
) -> SearchResult {
    if limit == 0 || snapshot.files.is_empty() {
        return empty_search_result();
    }

    let query_terms = tokenize(query);
    if query_terms.is_empty() {
        return empty_search_result();
    }

    let query_term_set: BTreeSet<&str> = query_terms.iter().map(String::as_str).collect();
    let document_count = usize_as_f64(snapshot.files.len());
    let average_document_length = average_document_length(&snapshot.files);
    let document_frequencies = document_frequencies(&snapshot.files, &query_term_set);

    let mut base_scores = Vec::with_capacity(snapshot.files.len());
    for file in &snapshot.files {
        let lexical = lexical_score(
            file,
            &query_term_set,
            &document_frequencies,
            document_count,
            average_document_length,
        );
        let (exact_symbol, best_symbol) = exact_symbol_score(file, query, &query_term_set);
        let filename = filename_score(file, query, &query_term_set);
        let git = git_score(file, &query_term_set);
        let test = test_score(file, &query_term_set);
        let diagnostic = diagnostic_score(file, diagnostics);
        let recency = recency_score(file, snapshot.indexed_unix_ms);

        base_scores.push(BaseScore {
            lexical,
            exact_symbol,
            filename,
            git,
            test,
            diagnostic,
            recency,
            best_symbol,
        });
    }

    let seed_paths = seed_paths(&snapshot.files, &base_scores);
    let mut hits = Vec::new();
    for (file, base) in snapshot.files.iter().zip(base_scores.iter()) {
        let graph = graph_score(file, &query_term_set, &seed_paths);
        let breakdown = ScoreBreakdown {
            lexical: base.lexical,
            exact_symbol: base.exact_symbol,
            filename: base.filename,
            graph,
            git: base.git,
            test: base.test,
            diagnostic: base.diagnostic,
            recency: base.recency,
            semantic: 0.0,
        };
        let query_anchored =
            base.lexical + base.exact_symbol + base.filename + base.test + base.diagnostic + graph
                > 0.0
                || (asks_about_changes(&query_term_set) && base.git > 0.0);
        if !query_anchored {
            continue;
        }
        let score = total_score(&breakdown);
        if score <= 0.0 {
            continue;
        }

        hits.push(SearchHit {
            path: file.path.clone(),
            score,
            reasons: reasons(&breakdown),
            breakdown,
            best_symbol: base.best_symbol.clone(),
            symbols: file.symbols.clone(),
        });
    }

    hits.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.path.cmp(&right.path))
    });
    hits.truncate(limit);

    let confidence = confidence(&hits, &query_term_set, snapshot);
    let expansion_candidates = expansion_candidates(snapshot, &hits);
    let mut alternative_candidates = Vec::new();
    prune_duplicate_alternatives(&mut hits, query, &mut alternative_candidates);
    prune_incidental_hits(&mut hits, confidence, &mut alternative_candidates);

    SearchResult {
        hits,
        confidence,
        needs_expansion: confidence < EXPANSION_CONFIDENCE_THRESHOLD,
        expansion_candidates,
        alternative_candidates,
    }
}

fn empty_search_result() -> SearchResult {
    SearchResult {
        hits: Vec::new(),
        confidence: 0.0,
        needs_expansion: true,
        expansion_candidates: Vec::new(),
        alternative_candidates: Vec::new(),
    }
}

fn prune_duplicate_alternatives(
    hits: &mut Vec<SearchHit>,
    query: &str,
    alternatives: &mut Vec<String>,
) {
    let query = query.to_lowercase();
    let ranked = hits.clone();
    hits.retain(|hit| {
        if hit.breakdown.diagnostic > 0.0
            || hit.breakdown.graph > 0.0
            || hit.breakdown.test > 0.0
            || query.contains(&hit.path.to_lowercase())
        {
            return true;
        }
        let dominated = ranked.iter().any(|higher| {
            higher.score * 0.6 > hit.score
                && hit.symbols.iter().any(|symbol| {
                    matches!(symbol.kind, SymbolKind::Function | SymbolKind::Method)
                        && !query.contains(&symbol.name.to_lowercase())
                        && higher
                            .symbols
                            .iter()
                            .any(|other| other.kind == symbol.kind && other.name == symbol.name)
                })
        });
        if dominated {
            alternatives.push(hit.path.clone());
        }
        !dominated
    });
}

fn prune_incidental_hits(
    hits: &mut Vec<SearchHit>,
    confidence: f64,
    expansion_candidates: &mut Vec<String>,
) {
    // Strong, well-covered queries need not carry every incidental word match.
    // Keep explicit/structural anchors even when their lexical score is small.
    // Weak queries retain all candidates and the ordinary expansion path.
    if confidence >= 0.8 {
        let score_floor = hits.first().map_or(0.0, |hit| hit.score * 0.6);
        hits.retain(|hit| {
            let anchored = hit.breakdown.exact_symbol > 0.0
                || hit.breakdown.graph > 0.0
                || hit.breakdown.diagnostic > 0.0
                || hit.breakdown.test > 0.0;
            if hit.score >= score_floor || anchored {
                true
            } else {
                if !expansion_candidates.contains(&hit.path) {
                    expansion_candidates.push(hit.path.clone());
                }
                false
            }
        });
    }
}

#[derive(Debug, Clone)]
struct BaseScore {
    lexical: f64,
    exact_symbol: f64,
    filename: f64,
    git: f64,
    test: f64,
    diagnostic: f64,
    recency: f64,
    best_symbol: Option<vexa_types::SymbolRecord>,
}

fn tokenize(value: &str) -> Vec<String> {
    vexa_types::lexical_tokens(value)
        .into_iter()
        .filter(|term| {
            !matches!(
                term.as_str(),
                "the"
                    | "that"
                    | "this"
                    | "with"
                    | "from"
                    | "for"
                    | "and"
                    | "whether"
                    | "find"
                    | "code"
                    | "implementation"
                    | "using"
                    | "which"
                    | "when"
                    | "where"
                    | "by"
                    | "to"
                    | "of"
                    | "an"
                    | "as"
                    | "is"
                    | "in"
                    | "it"
                    | "at"
                    | "has"
                    | "how"
                    | "does"
                    | "do"
                    | "be"
                    | "on"
                    | "or"
                    | "a"
                    | "me"
                    | "explain"
            )
        })
        .collect()
}

fn average_document_length(files: &[FileRecord]) -> f64 {
    let total: f64 = files
        .iter()
        .map(|file| f64::from(document_length(file)))
        .sum();
    if total == 0.0 {
        1.0
    } else {
        total / usize_as_f64(files.len())
    }
}

fn document_length(file: &FileRecord) -> u32 {
    file.term_frequency
        .values()
        .copied()
        .fold(0_u32, u32::saturating_add)
}

fn document_frequencies(
    files: &[FileRecord],
    query_terms: &BTreeSet<&str>,
) -> BTreeMap<String, usize> {
    let mut frequencies = BTreeMap::new();
    for term in query_terms {
        let count = files
            .iter()
            .filter(|file| term_frequency(file, term) > 0)
            .count();
        frequencies.insert((*term).to_owned(), count);
    }
    frequencies
}

fn term_frequency(file: &FileRecord, term: &str) -> u32 {
    file.term_frequency.get(term).copied().unwrap_or_else(|| {
        file.term_frequency
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(term))
            .map_or(0, |(_, value)| *value)
    })
}

fn lexical_score(
    file: &FileRecord,
    query_terms: &BTreeSet<&str>,
    document_frequencies: &BTreeMap<String, usize>,
    document_count: f64,
    average_document_length: f64,
) -> f64 {
    let document_length = f64::from(document_length(file)).max(1.0);
    query_terms
        .iter()
        .map(|term| {
            let frequency = f64::from(term_frequency(file, term));
            if frequency == 0.0 {
                return 0.0;
            }
            let df = usize_as_f64(document_frequencies.get(*term).copied().unwrap_or(0));
            let idf = (1.0 + (document_count - df + 0.5) / (df + 0.5)).ln();
            let denominator =
                frequency + K1 * (1.0 - B + B * document_length / average_document_length.max(1.0));
            idf * (frequency * (K1 + 1.0)) / denominator
        })
        .sum()
}

fn exact_symbol_score(
    file: &FileRecord,
    query: &str,
    query_terms: &BTreeSet<&str>,
) -> (f64, Option<vexa_types::SymbolRecord>) {
    let normalized_query = query.trim().to_lowercase();
    let mut best = None;
    let mut best_score = 0.0;
    for symbol in &file.symbols {
        let name = symbol.name.to_lowercase();
        let score = if name == normalized_query {
            3.0
        } else if query_terms.contains(name.as_str()) {
            2.4
        } else if symbol
            .signature
            .as_ref()
            .is_some_and(|signature| signature.to_lowercase().contains(&normalized_query))
        {
            1.2
        } else {
            0.0
        };
        if score > best_score {
            best_score = score;
            best = Some(symbol.clone());
        }
    }
    (best_score, best)
}

fn filename_score(file: &FileRecord, query: &str, query_terms: &BTreeSet<&str>) -> f64 {
    let path = file.path.to_lowercase();
    let filename = path.rsplit('/').next().unwrap_or(path.as_str());
    let stem = filename.split('.').next().unwrap_or(filename);
    let normalized_query = query.trim().to_lowercase();

    if filename == normalized_query || stem == normalized_query {
        return 2.25;
    }
    if path == normalized_query {
        return 2.5;
    }

    let path_terms: BTreeSet<String> = tokenize(&path).into_iter().collect();
    let matched = query_terms
        .iter()
        .filter(|term| path_terms.contains(**term))
        .count();
    if matched == 0 {
        0.0
    } else {
        1.5 * usize_as_f64(matched) / usize_as_f64(query_terms.len())
    }
}

fn git_score(file: &FileRecord, query_terms: &BTreeSet<&str>) -> f64 {
    let asks_about_changes = asks_about_changes(query_terms);
    let base = match file.git_state {
        GitFileState::Conflicted => 0.55,
        GitFileState::Modified | GitFileState::Added | GitFileState::Renamed => 0.4,
        GitFileState::Untracked => 0.3,
        GitFileState::Deleted
        | GitFileState::Ignored
        | GitFileState::Clean
        | GitFileState::Unknown => 0.0,
    };
    if asks_about_changes { base * 1.8 } else { base }
}

fn asks_about_changes(query_terms: &BTreeSet<&str>) -> bool {
    ["changed", "change", "modified", "git", "diff", "current"]
        .iter()
        .any(|term| query_terms.contains(term))
}

fn test_score(file: &FileRecord, query_terms: &BTreeSet<&str>) -> f64 {
    let asks_about_tests = ["test", "tests", "testing", "spec", "specs"]
        .iter()
        .any(|term| query_terms.contains(term));
    let is_test_file = file.path.to_lowercase().contains("test")
        || file
            .symbols
            .iter()
            .any(|symbol| symbol.kind == SymbolKind::Test);
    let related_match = file.related_tests.iter().any(|related| {
        let related_terms: BTreeSet<String> = tokenize(related).into_iter().collect();
        query_terms.iter().any(|term| related_terms.contains(*term))
    });

    match (asks_about_tests, is_test_file, related_match) {
        (true, true, _) => 1.0,
        (true, false, true) => 0.75,
        (false, _, true) => 0.4,
        _ => 0.0,
    }
}

fn diagnostic_score(file: &FileRecord, diagnostics: &[String]) -> f64 {
    if diagnostics.is_empty() {
        return 0.0;
    }
    let path = file.path.to_lowercase();
    let filename = path.rsplit('/').next().unwrap_or(path.as_str());
    if diagnostics.iter().any(|diagnostic| {
        let diagnostic = diagnostic.to_lowercase().replace('\\', "/");
        diagnostic.contains(&path) || diagnostic.contains(filename)
    }) {
        1.4
    } else {
        0.0
    }
}

fn recency_score(file: &FileRecord, indexed_unix_ms: u64) -> f64 {
    let age_ms = indexed_unix_ms.saturating_sub(file.modified_unix_ms);
    let age_days = Duration::from_millis(age_ms).as_secs_f64() / 86_400.0;
    0.25 / (1.0 + age_days / 30.0)
}

fn seed_paths(files: &[FileRecord], base_scores: &[BaseScore]) -> BTreeSet<String> {
    files
        .iter()
        .zip(base_scores)
        .filter(|(_, score)| {
            score.lexical + score.exact_symbol + score.filename + score.test + score.diagnostic
                > 0.0
        })
        .map(|(file, _)| file.path.to_lowercase())
        .collect()
}

fn graph_score(
    file: &FileRecord,
    query_terms: &BTreeSet<&str>,
    seed_paths: &BTreeSet<String>,
) -> f64 {
    let mut score: f64 = 0.0;
    for import in &file.imports {
        let target = import.target.to_lowercase().replace('\\', "/");
        let target_terms: BTreeSet<String> = tokenize(&target).into_iter().collect();
        if query_terms.iter().any(|term| target_terms.contains(*term)) {
            score = score.max(0.65);
        }
        if seed_paths
            .iter()
            .any(|path| import_matches_path(&target, path))
        {
            score = score.max(0.9);
        }
    }
    score
}

fn import_matches_path(import: &str, path: &str) -> bool {
    if import == path || path.ends_with(import) || import.ends_with(path) {
        return true;
    }
    let import_stem = path_stem(import);
    let path_stem = path_stem(path);
    !import_stem.is_empty() && import_stem == path_stem
}

fn path_stem(path: &str) -> &str {
    path.rsplit('/')
        .next()
        .unwrap_or(path)
        .split('.')
        .next()
        .unwrap_or("")
}

fn total_score(breakdown: &ScoreBreakdown) -> f64 {
    breakdown.lexical
        + breakdown.exact_symbol
        + breakdown.filename
        + breakdown.graph
        + breakdown.git
        + breakdown.test
        + breakdown.diagnostic
        + breakdown.recency
}

fn reasons(breakdown: &ScoreBreakdown) -> Vec<String> {
    let signals = [
        (breakdown.lexical, "lexical"),
        (breakdown.exact_symbol, "exact_symbol"),
        (breakdown.filename, "filename_or_path"),
        (breakdown.graph, "import_graph"),
        (breakdown.git, "git_state"),
        (breakdown.test, "test_relationship"),
        (breakdown.diagnostic, "diagnostic"),
        (breakdown.recency, "recency"),
    ];
    signals
        .into_iter()
        .filter(|(score, _)| *score > 0.0)
        .map(|(_, reason)| reason.to_owned())
        .collect()
}

fn confidence(hits: &[SearchHit], query_terms: &BTreeSet<&str>, snapshot: &IndexSnapshot) -> f64 {
    let Some(top) = hits.first() else {
        return 0.0;
    };
    let Some(file) = snapshot.files.iter().find(|file| file.path == top.path) else {
        return 0.0;
    };

    let covered = query_terms
        .iter()
        .filter(|term| file_covers_term(file, term))
        .count();
    let coverage = usize_as_f64(covered) / usize_as_f64(query_terms.len().max(1));
    let strength = 1.0 - (-top.score / 4.0).exp();
    let separation = hits.get(1).map_or(1.0, |second| {
        ((top.score - second.score).max(0.0) / top.score.max(f64::EPSILON)).min(1.0)
    });
    let diversity = usize_as_f64(top.reasons.len().min(6)) / 6.0;

    (0.45 * coverage + 0.25 * strength + 0.15 * separation + 0.15 * diversity).clamp(0.0, 1.0)
}

fn file_covers_term(file: &FileRecord, term: &str) -> bool {
    term_frequency(file, term) > 0
        || tokenize(&file.path)
            .iter()
            .any(|path_term| path_term == term)
        || file
            .symbols
            .iter()
            .any(|symbol| symbol.name.eq_ignore_ascii_case(term))
}

fn expansion_candidates(snapshot: &IndexSnapshot, hits: &[SearchHit]) -> Vec<String> {
    let hit_paths: BTreeSet<&str> = hits.iter().map(|hit| hit.path.as_str()).collect();
    let indexed_paths: BTreeMap<String, &str> = snapshot
        .files
        .iter()
        .map(|file| (file.path.to_lowercase(), file.path.as_str()))
        .collect();
    let mut candidates: BTreeMap<String, f64> = BTreeMap::new();

    for (rank, hit) in hits.iter().enumerate() {
        let Some(file) = snapshot.files.iter().find(|file| file.path == hit.path) else {
            continue;
        };
        let rank_weight = 1.0 / usize_as_f64(rank + 1);

        for related in &file.related_tests {
            if let Some(path) = resolve_path(related, &indexed_paths)
                && !hit_paths.contains(path)
            {
                *candidates.entry(path.to_owned()).or_default() += 1.25 * rank_weight;
            }
        }
        for import in &file.imports {
            if let Some(path) = resolve_path(&import.target, &indexed_paths)
                && !hit_paths.contains(path)
            {
                *candidates.entry(path.to_owned()).or_default() += rank_weight;
            }
        }
        for candidate in &snapshot.files {
            if hit_paths.contains(candidate.path.as_str()) {
                continue;
            }
            let imports_hit = candidate.imports.iter().any(|import| {
                let target = import.target.to_lowercase().replace('\\', "/");
                import_matches_path(&target, &hit.path.to_lowercase())
            });
            if imports_hit {
                *candidates.entry(candidate.path.clone()).or_default() += 0.9 * rank_weight;
            }
        }
    }

    let mut ranked: Vec<(String, f64)> = candidates.into_iter().collect();
    ranked.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    ranked
        .into_iter()
        .take(MAX_EXPANSION_CANDIDATES)
        .map(|(path, _)| path)
        .collect()
}

fn resolve_path<'a>(target: &str, indexed_paths: &'a BTreeMap<String, &'a str>) -> Option<&'a str> {
    let normalized = target.to_lowercase().replace('\\', "/");
    if let Some(path) = indexed_paths.get(&normalized) {
        return Some(*path);
    }
    indexed_paths.iter().find_map(|(candidate, original)| {
        import_matches_path(&normalized, candidate).then_some(*original)
    })
}

fn usize_as_f64(value: usize) -> f64 {
    f64::from(u32::try_from(value).unwrap_or(u32::MAX))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use vexa_types::{GitFileState, INDEX_SCHEMA_VERSION, ImportRecord, Language, SymbolRecord};

    use super::*;

    fn file(path: &str, terms: &[(&str, u32)]) -> FileRecord {
        FileRecord {
            path: path.to_owned(),
            language: Language::Rust,
            size_bytes: 100,
            modified_unix_ms: 1_000_000,
            sha256: format!("sha-{path}"),
            git_state: GitFileState::Clean,
            generated: false,
            vendor: false,
            sensitive: false,
            token_count: 20,
            term_frequency: terms
                .iter()
                .map(|(term, count)| ((*term).to_owned(), *count))
                .collect(),
            symbols: Vec::new(),
            parse_errors: false,
            imports: Vec::new(),
            related_tests: Vec::new(),
        }
    }

    fn snapshot(files: Vec<FileRecord>) -> IndexSnapshot {
        IndexSnapshot {
            schema_version: INDEX_SCHEMA_VERSION,
            parser_version: "test".to_owned(),
            repository_root: PathBuf::from("."),
            git_revision: Some("abc123".to_owned()),
            indexed_unix_ms: 1_000_000,
            files,
        }
    }

    #[test]
    fn exact_symbol_and_filename_beat_lexical_noise_deterministically() {
        let mut target = file("src/auth.rs", &[("auth", 2), ("token", 1)]);
        target.symbols.push(SymbolRecord {
            name: "validate_token".to_owned(),
            kind: SymbolKind::Function,
            start_line: 10,
            end_line: 20,
            signature: Some("fn validate_token(token: &str) -> bool".to_owned()),
        });
        let noisy = file("src/notes.rs", &[("validate_token", 8), ("token", 8)]);
        let index = snapshot(vec![noisy, target]);

        let result = search(&index, "validate_token", &[], 2);

        assert_eq!(
            result.hits.first().map(|hit| hit.path.as_str()),
            Some("src/auth.rs")
        );
        assert!(result.hits[0].breakdown.exact_symbol > 0.0);
        assert!(result.hits[0].breakdown.semantic.abs() < f64::EPSILON);
    }

    #[test]
    fn diagnostics_git_and_test_relationships_are_scored() {
        let mut source = file("src/parser.rs", &[("parser", 1)]);
        source.git_state = GitFileState::Modified;
        source.related_tests.push("tests/parser_test.rs".to_owned());
        let mut tests = file("tests/parser_test.rs", &[("parser", 1), ("test", 2)]);
        tests.symbols.push(SymbolRecord {
            name: "parses_input".to_owned(),
            kind: SymbolKind::Test,
            start_line: 1,
            end_line: 5,
            signature: None,
        });
        let index = snapshot(vec![source, tests]);

        let result = search(
            &index,
            "changed parser tests",
            &["error in src/parser.rs:17".to_owned()],
            2,
        );

        let parser = result.hits.iter().find(|hit| hit.path == "src/parser.rs");
        assert!(
            parser.is_some_and(|hit| hit.breakdown.git > 0.0 && hit.breakdown.diagnostic > 0.0)
        );
        let test = result
            .hits
            .iter()
            .find(|hit| hit.path == "tests/parser_test.rs");
        assert!(test.is_some_and(|hit| hit.breakdown.test > 0.0));
    }

    #[test]
    fn import_graph_produces_expansion_candidates() {
        let mut entry = file("src/main.rs", &[("server", 3)]);
        entry.imports.push(ImportRecord {
            target: "src/net.rs".to_owned(),
            start_line: 1,
            end_line: 1,
        });
        entry.related_tests.push("tests/net.rs".to_owned());
        let network = file("src/net.rs", &[("socket", 2)]);
        let tests = file("tests/net.rs", &[("network", 1)]);
        let index = snapshot(vec![entry, network, tests]);

        let result = search(&index, "server", &[], 1);

        assert_eq!(result.hits.len(), 1);
        assert_eq!(result.hits[0].path, "src/main.rs");
        assert_eq!(
            result.expansion_candidates,
            vec!["tests/net.rs", "src/net.rs"]
        );
    }

    #[test]
    fn weak_unknown_query_requests_expansion() {
        let index = snapshot(vec![file("src/lib.rs", &[("library", 1)])]);
        let result = search(&index, "totally_unknown_symbol", &[], 5);
        assert!(result.hits.is_empty());
        assert!(result.needs_expansion);
        assert!(result.confidence < EXPANSION_CONFIDENCE_THRESHOLD);
    }

    #[test]
    fn explicit_duplicate_symbol_keeps_both_definitions() {
        let mut high = file("src/high.rs", &[("pick", 9), ("candidate", 9)]);
        let mut low = file("src/low.rs", &[("pick", 1)]);
        let symbol = SymbolRecord {
            name: "pick".to_owned(),
            kind: SymbolKind::Function,
            start_line: 1,
            end_line: 3,
            signature: None,
        };
        high.symbols.push(symbol.clone());
        low.symbols.push(symbol);
        let index = snapshot(vec![high, low]);
        let result = search(&index, "pick", &[], 6);
        assert_eq!(result.hits.len(), 2);
        assert!(result.alternative_candidates.is_empty());
    }

    #[test]
    fn strong_query_prunes_incidental_words_but_retains_weak_structural_evidence() {
        let mut target = file(
            "src/rank.rs",
            &[("rank", 5), ("stable", 5), ("path", 5), ("tie", 5)],
        );
        target.symbols.push(SymbolRecord {
            name: "rank".to_owned(),
            kind: SymbolKind::Function,
            start_line: 1,
            end_line: 8,
            signature: None,
        });
        let mut caller = file("src/caller.rs", &[("invoke", 1)]);
        caller.imports.push(ImportRecord {
            target: "src/rank.rs".to_owned(),
            start_line: 1,
            end_line: 1,
        });
        let diagnostic = file("src/required.rs", &[("unrelated", 1)]);
        let noise = file("docs/incidental.md", &[("rank", 1)]);
        let index = snapshot(vec![target, caller, diagnostic, noise]);
        let result = search(
            &index,
            "rank stable path tie",
            &["src/required.rs:12 error".to_owned()],
            8,
        );
        assert!(result.confidence >= 0.8);
        assert!(result.hits.iter().any(|hit| hit.path == "src/caller.rs"));
        assert!(result.hits.iter().any(|hit| hit.path == "src/required.rs"));
        assert!(
            !result
                .hits
                .iter()
                .any(|hit| hit.path == "docs/incidental.md")
        );
        assert!(
            result
                .alternative_candidates
                .contains(&"docs/incidental.md".to_owned())
        );
    }
}
