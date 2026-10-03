//! Live, machine-checkable verification helpers and deterministic log reduction.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use vexa_types::{FileRecord, IndexSnapshot};

const MAX_CAUSAL_ERRORS: usize = 64;
const MAX_TESTS: usize = 512;
const MAX_REDUCED_LINE_CHARS: usize = 4096;
const HEX_LOWER: &[u8; 16] = b"0123456789abcdef";

/// A claim VEXA can check against live repository state and, when needed, an index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Claim {
    FileExists {
        path: String,
    },
    FileHash {
        path: String,
        expected_sha256: String,
    },
    SymbolExists {
        path: String,
        symbol: String,
    },
    SignatureExists {
        path: String,
        symbol: String,
        signature: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimState {
    Verified,
    Contradicted,
    Unknown,
}

/// Structured verification output. `Unknown` is used whenever evidence is stale,
/// unavailable, or malformed rather than treating missing evidence as success.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimResult {
    pub claim: Claim,
    pub state: ClaimState,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexed_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_signature: Option<String>,
}

/// Verifies a file's existence using the live filesystem.
#[must_use]
pub fn verify_file_exists(repository_root: &Path, path: &str) -> ClaimResult {
    verify_claim(
        repository_root,
        None,
        &Claim::FileExists {
            path: path.to_owned(),
        },
    )
}

/// Verifies a SHA-256 claim by hashing the live file.
#[must_use]
pub fn verify_file_hash(repository_root: &Path, path: &str, expected_sha256: &str) -> ClaimResult {
    verify_claim(
        repository_root,
        None,
        &Claim::FileHash {
            path: path.to_owned(),
            expected_sha256: expected_sha256.to_owned(),
        },
    )
}

/// Verifies an indexed symbol only after proving that the indexed file hash still
/// matches the live file.
#[must_use]
pub fn verify_symbol(
    repository_root: &Path,
    index: &IndexSnapshot,
    path: &str,
    symbol: &str,
) -> ClaimResult {
    verify_claim(
        repository_root,
        Some(index),
        &Claim::SymbolExists {
            path: path.to_owned(),
            symbol: symbol.to_owned(),
        },
    )
}

/// Verifies an exact indexed signature against a live-hash-validated index record.
#[must_use]
pub fn verify_signature(
    repository_root: &Path,
    index: &IndexSnapshot,
    path: &str,
    symbol: &str,
    signature: &str,
) -> ClaimResult {
    verify_claim(
        repository_root,
        Some(index),
        &Claim::SignatureExists {
            path: path.to_owned(),
            symbol: symbol.to_owned(),
            signature: signature.to_owned(),
        },
    )
}

/// Verifies one structured claim. Symbol and signature claims require an index.
#[must_use]
pub fn verify_claim(
    repository_root: &Path,
    index: Option<&IndexSnapshot>,
    claim: &Claim,
) -> ClaimResult {
    match claim {
        Claim::FileExists { path } => verify_exists_claim(repository_root, path, claim.clone()),
        Claim::FileHash {
            path,
            expected_sha256,
        } => verify_hash_claim(repository_root, path, expected_sha256, claim.clone()),
        Claim::SymbolExists { path, symbol } => {
            let Some(index) = index else {
                return claim_result(claim.clone(), ClaimState::Unknown, "index_unavailable");
            };
            verify_symbol_claim(repository_root, index, path, symbol, claim.clone())
        }
        Claim::SignatureExists {
            path,
            symbol,
            signature,
        } => {
            let Some(index) = index else {
                return claim_result(claim.clone(), ClaimState::Unknown, "index_unavailable");
            };
            verify_signature_claim(
                repository_root,
                index,
                path,
                symbol,
                signature,
                claim.clone(),
            )
        }
    }
}

fn verify_exists_claim(repository_root: &Path, path: &str, claim: Claim) -> ClaimResult {
    match resolve_live_file(repository_root, path) {
        Ok(_) => claim_result(claim, ClaimState::Verified, "live_file_exists"),
        Err(LiveFileError::Missing(reason)) => {
            claim_result(claim, ClaimState::Contradicted, reason)
        }
        Err(LiveFileError::Unknown(reason)) => claim_result(claim, ClaimState::Unknown, &reason),
    }
}

fn verify_hash_claim(
    repository_root: &Path,
    path: &str,
    expected_sha256: &str,
    claim: Claim,
) -> ClaimResult {
    if !is_sha256(expected_sha256) {
        return claim_result(claim, ClaimState::Unknown, "invalid_expected_sha256");
    }
    match live_sha256(repository_root, path) {
        Ok(actual) => {
            let state = if actual.eq_ignore_ascii_case(expected_sha256) {
                ClaimState::Verified
            } else {
                ClaimState::Contradicted
            };
            let reason = if state == ClaimState::Verified {
                "live_hash_matches"
            } else {
                "live_hash_mismatch"
            };
            let mut result = claim_result(claim, state, reason);
            result.actual_sha256 = Some(actual);
            result
        }
        Err(LiveFileError::Missing(reason)) => {
            claim_result(claim, ClaimState::Contradicted, reason)
        }
        Err(LiveFileError::Unknown(reason)) => claim_result(claim, ClaimState::Unknown, &reason),
    }
}

fn verify_symbol_claim(
    repository_root: &Path,
    index: &IndexSnapshot,
    path: &str,
    symbol: &str,
    claim: Claim,
) -> ClaimResult {
    if symbol.trim().is_empty() {
        return claim_result(claim, ClaimState::Unknown, "invalid_symbol");
    }
    let Some((record, actual_hash)) = live_index_record(repository_root, index, path, &claim)
    else {
        return live_index_failure(repository_root, index, path, claim);
    };

    let found = record.symbols.iter().find(|record| record.name == symbol);
    let mut result = if found.is_some() {
        claim_result(claim, ClaimState::Verified, "symbol_present")
    } else if record.parse_errors {
        claim_result(claim, ClaimState::Unknown, "parser_incomplete")
    } else {
        claim_result(claim, ClaimState::Contradicted, "symbol_absent")
    };
    result.actual_sha256 = Some(actual_hash);
    result.indexed_sha256 = Some(record.sha256.clone());
    result.matched_signature = found.and_then(|record| record.signature.clone());
    result
}

fn verify_signature_claim(
    repository_root: &Path,
    index: &IndexSnapshot,
    path: &str,
    symbol: &str,
    signature: &str,
    claim: Claim,
) -> ClaimResult {
    if symbol.trim().is_empty() || signature.trim().is_empty() {
        return claim_result(claim, ClaimState::Unknown, "invalid_symbol_or_signature");
    }
    let Some((record, actual_hash)) = live_index_record(repository_root, index, path, &claim)
    else {
        return live_index_failure(repository_root, index, path, claim);
    };

    let named_symbols: Vec<_> = record
        .symbols
        .iter()
        .filter(|record| record.name == symbol)
        .collect();
    let (state, reason, matched_signature) = if record.parse_errors {
        (ClaimState::Unknown, "parser_incomplete", None)
    } else if named_symbols.is_empty() {
        (ClaimState::Contradicted, "symbol_absent", None)
    } else if let Some(record) = named_symbols
        .iter()
        .find(|record| record.signature.as_deref() == Some(signature))
    {
        (
            ClaimState::Verified,
            "signature_present",
            record.signature.clone(),
        )
    } else if named_symbols
        .iter()
        .all(|record| record.signature.is_none())
    {
        (ClaimState::Unknown, "signature_not_indexed", None)
    } else {
        (ClaimState::Contradicted, "signature_mismatch", None)
    };

    let mut result = claim_result(claim, state, reason);
    result.actual_sha256 = Some(actual_hash);
    result.indexed_sha256 = Some(record.sha256.clone());
    result.matched_signature = matched_signature;
    result
}

fn live_index_record<'a>(
    repository_root: &Path,
    index: &'a IndexSnapshot,
    path: &str,
    _claim: &Claim,
) -> Option<(&'a FileRecord, String)> {
    let normalized = normalize_repo_path(path).ok()?.0;
    let record = index
        .files
        .iter()
        .find(|record| normalize_index_path(&record.path) == normalized)?;
    let actual = live_sha256(repository_root, path).ok()?;
    if actual.eq_ignore_ascii_case(&record.sha256) {
        Some((record, actual))
    } else {
        None
    }
}

fn live_index_failure(
    repository_root: &Path,
    index: &IndexSnapshot,
    path: &str,
    claim: Claim,
) -> ClaimResult {
    let Ok((normalized, _)) = normalize_repo_path(path) else {
        return claim_result(claim, ClaimState::Unknown, "invalid_repository_path");
    };
    let Some(record) = index
        .files
        .iter()
        .find(|record| normalize_index_path(&record.path) == normalized)
    else {
        return claim_result(claim, ClaimState::Unknown, "path_not_indexed");
    };
    match live_sha256(repository_root, path) {
        Ok(actual) => {
            let mut result = claim_result(claim, ClaimState::Unknown, "stale_index_hash");
            result.actual_sha256 = Some(actual);
            result.indexed_sha256 = Some(record.sha256.clone());
            result
        }
        Err(LiveFileError::Missing(reason)) => {
            claim_result(claim, ClaimState::Contradicted, reason)
        }
        Err(LiveFileError::Unknown(reason)) => claim_result(claim, ClaimState::Unknown, &reason),
    }
}

fn claim_result(claim: Claim, state: ClaimState, reason: &str) -> ClaimResult {
    ClaimResult {
        claim,
        state,
        reason: reason.to_owned(),
        actual_sha256: None,
        indexed_sha256: None,
        matched_signature: None,
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn normalize_index_path(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").to_owned()
}

fn normalize_repo_path(path: &str) -> Result<(String, PathBuf), String> {
    let replaced = path.replace('\\', "/");
    if replaced.is_empty() || replaced.starts_with('/') || replaced.ends_with('/') {
        return Err("invalid_repository_path".to_owned());
    }

    let mut normalized_parts = Vec::new();
    let mut relative = PathBuf::new();
    for part in replaced.split('/') {
        match part {
            "" | ".." => return Err("invalid_repository_path".to_owned()),
            "." => {}
            component => {
                normalized_parts.push(component);
                relative.push(component);
            }
        }
    }
    if normalized_parts.is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("invalid_repository_path".to_owned());
    }
    Ok((normalized_parts.join("/"), relative))
}

#[derive(Debug)]
enum LiveFileError {
    Missing(&'static str),
    Unknown(String),
}

fn resolve_live_file(repository_root: &Path, path: &str) -> Result<PathBuf, LiveFileError> {
    let (_, relative) = normalize_repo_path(path).map_err(LiveFileError::Unknown)?;
    let root = fs::canonicalize(repository_root)
        .map_err(|error| LiveFileError::Unknown(format!("repository_root_unavailable:{error}")))?;
    let candidate = root.join(relative);
    let metadata = match fs::metadata(&candidate) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(LiveFileError::Missing("live_file_missing"));
        }
        Err(error) => {
            return Err(LiveFileError::Unknown(format!(
                "live_metadata_error:{error}"
            )));
        }
    };
    if !metadata.is_file() {
        return Err(LiveFileError::Missing("live_path_is_not_file"));
    }
    let canonical = fs::canonicalize(&candidate)
        .map_err(|error| LiveFileError::Unknown(format!("live_path_unavailable:{error}")))?;
    if !canonical.starts_with(&root) {
        return Err(LiveFileError::Unknown(
            "live_path_escapes_repository".to_owned(),
        ));
    }
    Ok(canonical)
}

fn live_sha256(repository_root: &Path, path: &str) -> Result<String, LiveFileError> {
    let live_path = resolve_live_file(repository_root, path)?;
    let mut file = File::open(&live_path)
        .map_err(|error| LiveFileError::Unknown(format!("live_file_open_error:{error}")))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| LiveFileError::Unknown(format!("live_file_read_error:{error}")))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        encoded.push(char::from(HEX_LOWER[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX_LOWER[usize::from(byte & 0x0f)]));
    }
    Ok(encoded)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Passed,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestOutcome {
    Passed,
    Failed,
    Ignored,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub line: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestObservation {
    pub name: String,
    pub outcome: TestOutcome,
    pub line: usize,
}

/// Deterministic reduction of build/test output. The reducer scans the complete input
/// but bounds retained details so giant logs cannot produce giant summaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogSummary {
    pub exit_code: Option<i32>,
    pub status: RunStatus,
    pub first_error: Option<Diagnostic>,
    pub causal_errors: Vec<Diagnostic>,
    pub tests: Vec<TestObservation>,
    pub lines_scanned: usize,
    pub truncated: bool,
}

/// Reduces raw build/test output while preserving the first error, causal error lines,
/// observed test names/outcomes, and the process exit status.
#[must_use]
pub fn reduce_log(raw: &str, exit_code: Option<i32>) -> LogSummary {
    let mut first_error = None;
    let mut causal_errors = Vec::new();
    let mut tests = BTreeMap::<String, TestObservation>::new();
    let mut lines_scanned = 0;
    let mut truncated = false;

    for (offset, line) in raw.lines().enumerate() {
        let line_number = offset + 1;
        lines_scanned = line_number;
        let is_cause = is_causal_error(line);
        if first_error.is_none() && (is_error_line(line) || is_cause) {
            let (text, was_truncated) = bounded_line(line);
            truncated |= was_truncated;
            first_error = Some(Diagnostic {
                line: line_number,
                text,
            });
        }
        if is_cause {
            if causal_errors.len() < MAX_CAUSAL_ERRORS {
                let (text, was_truncated) = bounded_line(line);
                truncated |= was_truncated;
                causal_errors.push(Diagnostic {
                    line: line_number,
                    text,
                });
            } else {
                truncated = true;
            }
        }
        if let Some(observation) = parse_test_observation(line, line_number) {
            record_test(&mut tests, observation, &mut truncated);
        }
    }

    let has_failed_test = tests
        .values()
        .any(|observation| observation.outcome == TestOutcome::Failed);
    let status = match exit_code {
        Some(0) if first_error.is_none() && !has_failed_test => RunStatus::Passed,
        Some(_) => RunStatus::Failed,
        None if first_error.is_some() || has_failed_test => RunStatus::Failed,
        None => RunStatus::Unknown,
    };

    LogSummary {
        exit_code,
        status,
        first_error,
        causal_errors,
        tests: tests.into_values().collect(),
        lines_scanned,
        truncated,
    }
}

fn bounded_line(line: &str) -> (String, bool) {
    let mut chars = line.chars();
    let mut text: String = chars.by_ref().take(MAX_REDUCED_LINE_CHARS).collect();
    let truncated = chars.next().is_some();
    if truncated {
        text.push('…');
    }
    (text, truncated)
}

fn is_causal_error(line: &str) -> bool {
    let trimmed = line.trim_start().to_ascii_lowercase();
    trimmed.starts_with("caused by:") || trimmed.starts_with("cause:")
}

fn is_error_line(line: &str) -> bool {
    let trimmed = line.trim();
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("error:")
        || lower.starts_with("error[")
        || lower.starts_with("fatal:")
        || lower.contains("panicked at")
        || lower.starts_with("failed ")
        || lower == "failed"
        || lower.starts_with("--- fail:")
        || (trimmed.starts_with("test ") && trimmed.ends_with(" ... FAILED"))
}

fn parse_test_observation(line: &str, line_number: usize) -> Option<TestObservation> {
    let trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix("test ")
        && let Some((name, outcome)) = rest.rsplit_once(" ... ")
    {
        let outcome = match outcome {
            "ok" => TestOutcome::Passed,
            "FAILED" => TestOutcome::Failed,
            "ignored" => TestOutcome::Ignored,
            _ => TestOutcome::Unknown,
        };
        return Some(TestObservation {
            name: name.to_owned(),
            outcome,
            line: line_number,
        });
    }

    for (prefix, outcome) in [
        ("FAILED ", TestOutcome::Failed),
        ("PASSED ", TestOutcome::Passed),
    ] {
        if let Some(rest) = trimmed.strip_prefix(prefix)
            && let Some(name) = rest.split_whitespace().next()
        {
            return Some(TestObservation {
                name: name.to_owned(),
                outcome,
                line: line_number,
            });
        }
    }

    for (prefix, outcome) in [
        ("--- FAIL: ", TestOutcome::Failed),
        ("--- PASS: ", TestOutcome::Passed),
    ] {
        if let Some(rest) = trimmed.strip_prefix(prefix)
            && let Some(name) = rest.split_whitespace().next()
        {
            return Some(TestObservation {
                name: name.to_owned(),
                outcome,
                line: line_number,
            });
        }
    }

    None
}

fn record_test(
    tests: &mut BTreeMap<String, TestObservation>,
    observation: TestObservation,
    truncated: &mut bool,
) {
    if let Some(existing) = tests.get_mut(&observation.name) {
        if outcome_priority(observation.outcome) > outcome_priority(existing.outcome) {
            *existing = observation;
        }
        return;
    }

    if tests.len() < MAX_TESTS {
        tests.insert(observation.name.clone(), observation);
        return;
    }

    *truncated = true;
    if observation.outcome == TestOutcome::Failed {
        let evict = tests
            .iter()
            .find(|(_, existing)| existing.outcome != TestOutcome::Failed)
            .map(|(name, _)| name.clone());
        if let Some(name) = evict {
            tests.remove(&name);
            tests.insert(observation.name.clone(), observation);
        }
    }
}

const fn outcome_priority(outcome: TestOutcome) -> u8 {
    match outcome {
        TestOutcome::Failed => 3,
        TestOutcome::Passed => 2,
        TestOutcome::Ignored => 1,
        TestOutcome::Unknown => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;
    use std::fs;
    use tempfile::tempdir;
    use vexa_types::{GitFileState, Language, SymbolKind, SymbolRecord};

    fn file_record(path: &str, hash: &str, symbols: Vec<SymbolRecord>) -> FileRecord {
        FileRecord {
            path: path.to_owned(),
            language: Language::Rust,
            size_bytes: 0,
            modified_unix_ms: 0,
            sha256: hash.to_owned(),
            git_state: GitFileState::Clean,
            generated: false,
            vendor: false,
            sensitive: false,
            token_count: 0,
            term_frequency: BTreeMap::new(),
            symbols,
            parse_errors: false,
            imports: Vec::new(),
            related_tests: Vec::new(),
        }
    }

    fn snapshot(root: &Path, record: FileRecord) -> IndexSnapshot {
        IndexSnapshot {
            schema_version: 1,
            parser_version: "test".to_owned(),
            repository_root: root.to_path_buf(),
            git_revision: Some("rev".to_owned()),
            indexed_unix_ms: 0,
            files: vec![record],
        }
    }

    fn symbol(name: &str, signature: Option<&str>) -> SymbolRecord {
        SymbolRecord {
            name: name.to_owned(),
            kind: SymbolKind::Function,
            start_line: 1,
            end_line: 1,
            signature: signature.map(str::to_owned),
        }
    }

    #[test]
    fn live_hash_verification_detects_mismatch() {
        let directory = tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let path = directory.path().join("file.txt");
        fs::write(&path, b"live").unwrap_or_else(|error| panic!("write failed: {error}"));
        let result = verify_file_hash(directory.path(), "file.txt", &"0".repeat(64));
        assert_eq!(result.state, ClaimState::Contradicted);
        assert_eq!(result.reason, "live_hash_mismatch");
        assert!(result.actual_sha256.is_some());
    }

    #[test]
    fn adversarial_stale_index_hash_is_unknown_for_symbol_claim() {
        let directory = tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let source_dir = directory.path().join("src");
        fs::create_dir(&source_dir).unwrap_or_else(|error| panic!("create src failed: {error}"));
        fs::write(source_dir.join("lib.rs"), b"fn real() {}\n")
            .unwrap_or_else(|error| panic!("write original failed: {error}"));
        let old_hash = live_sha256(directory.path(), "src/lib.rs")
            .unwrap_or_else(|error| panic!("hash failed: {error:?}"));
        let index = snapshot(
            directory.path(),
            file_record("src/lib.rs", &old_hash, vec![symbol("real", None)]),
        );
        fs::write(source_dir.join("lib.rs"), b"fn changed() {}\n")
            .unwrap_or_else(|error| panic!("rewrite failed: {error}"));

        let result = verify_symbol(directory.path(), &index, "src/lib.rs", "real");
        assert_eq!(result.state, ClaimState::Unknown);
        assert_eq!(result.reason, "stale_index_hash");
        assert_ne!(result.actual_sha256, result.indexed_sha256);
    }

    #[test]
    fn adversarial_invented_symbol_is_contradicted_when_index_is_fresh() {
        let directory = tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        fs::write(directory.path().join("lib.rs"), b"fn real() {}\n")
            .unwrap_or_else(|error| panic!("write failed: {error}"));
        let hash = live_sha256(directory.path(), "lib.rs")
            .unwrap_or_else(|error| panic!("hash failed: {error:?}"));
        let index = snapshot(
            directory.path(),
            file_record("lib.rs", &hash, vec![symbol("real", Some("fn real()"))]),
        );

        let result = verify_symbol(directory.path(), &index, "lib.rs", "invented");
        assert_eq!(result.state, ClaimState::Contradicted);
        assert_eq!(result.reason, "symbol_absent");
    }

    #[test]
    fn exact_signature_is_verified_only_on_fresh_index() {
        let directory = tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        fs::write(directory.path().join("lib.rs"), b"fn real() {}\n")
            .unwrap_or_else(|error| panic!("write failed: {error}"));
        let hash = live_sha256(directory.path(), "lib.rs")
            .unwrap_or_else(|error| panic!("hash failed: {error:?}"));
        let index = snapshot(
            directory.path(),
            file_record("lib.rs", &hash, vec![symbol("real", Some("fn real()"))]),
        );

        let result = verify_signature(directory.path(), &index, "lib.rs", "real", "fn real()");
        assert_eq!(result.state, ClaimState::Verified);
        assert_eq!(result.matched_signature.as_deref(), Some("fn real()"));
    }

    #[test]
    fn incomplete_parser_cannot_disprove_absent_symbol() {
        let directory = tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        fs::write(directory.path().join("lib.rs"), b"fn broken( {\n")
            .unwrap_or_else(|error| panic!("write failed: {error}"));
        let hash = live_sha256(directory.path(), "lib.rs")
            .unwrap_or_else(|error| panic!("hash failed: {error:?}"));
        let mut record = file_record("lib.rs", &hash, Vec::new());
        record.parse_errors = true;
        let index = snapshot(directory.path(), record);
        let result = verify_symbol(directory.path(), &index, "lib.rs", "invented");
        assert_eq!(result.state, ClaimState::Unknown);
        assert_eq!(result.reason, "parser_incomplete");
    }

    #[test]
    fn giant_log_preserves_first_error_cause_tests_and_exit_status() {
        let mut log = String::from("error[E0001]: first failure\n");
        for index in 0..25_000 {
            writeln!(log, "informational line {index}")
                .unwrap_or_else(|error| panic!("write to String failed: {error}"));
        }
        log.push_str("Caused by: late root cause\n");
        log.push_str("test suite::works ... ok\n");
        log.push_str("test suite::fails ... FAILED\n");

        let summary = reduce_log(&log, Some(101));
        assert_eq!(summary.exit_code, Some(101));
        assert_eq!(summary.status, RunStatus::Failed);
        assert_eq!(
            summary.first_error.as_ref().map(|error| error.line),
            Some(1)
        );
        assert!(
            summary
                .causal_errors
                .iter()
                .any(|error| error.text.contains("late root cause"))
        );
        assert!(
            summary
                .tests
                .iter()
                .any(|test| test.name == "suite::works" && test.outcome == TestOutcome::Passed)
        );
        assert!(
            summary
                .tests
                .iter()
                .any(|test| test.name == "suite::fails" && test.outcome == TestOutcome::Failed)
        );
        assert!(summary.lines_scanned > 25_000);
    }

    #[test]
    fn missing_exit_status_without_failure_evidence_is_unknown() {
        let summary = reduce_log("Compiling crate\nFinished dev profile\n", None);
        assert_eq!(summary.status, RunStatus::Unknown);
        assert!(summary.first_error.is_none());
    }

    #[test]
    fn zero_exit_cannot_hide_failed_test_observations() {
        let result = reduce_log("test suite::broken ... FAILED\n", Some(0));
        assert_eq!(result.status, RunStatus::Failed);
    }
}
