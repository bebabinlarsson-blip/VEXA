//! VEXA deterministic orchestration core.

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use vexa_index::{IndexOptions, build_index, hash_file, load_snapshot, save_snapshot};
use vexa_pack::{PackOptions, pack_with_options};
use vexa_seek::{SearchResult, search};
use vexa_types::{
    Evidence, EvidenceSummary, IndexSnapshot, OperatingProfile, PrepareRequest, PrepareResponse,
    Provenance, ScoreBreakdown, SearchHit, TrustLevel, VEXA_API_VERSION, VerificationState,
};

const MAX_EXCERPT_LINES: usize = 96;
const MIN_CONTEXT_TOKENS: usize = 128;

#[derive(Debug, Clone)]
pub struct CoreConfig {
    pub cache_root: PathBuf,
    pub index_options: IndexOptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrepareOptions {
    pub include_user_prompt_in_packet: bool,
    pub include_protected_constraints_in_packet: bool,
}

impl Default for PrepareOptions {
    fn default() -> Self {
        Self {
            include_user_prompt_in_packet: true,
            include_protected_constraints_in_packet: true,
        }
    }
}

impl CoreConfig {
    /// Detect the default per-user VEXA cache location.
    ///
    /// # Errors
    ///
    /// Returns an error when the platform does not expose a writable per-user cache/data location.
    pub fn detect() -> Result<Self> {
        if let Some(path) = std::env::var_os("VEXA_CACHE_ROOT") {
            if path.is_empty() {
                bail!("VEXA_CACHE_ROOT cannot be empty");
            }
            return Ok(Self {
                cache_root: PathBuf::from(path),
                index_options: IndexOptions::default(),
            });
        }
        let cache_root = dirs::cache_dir()
            .or_else(dirs::data_local_dir)
            .context("cannot determine a per-user cache directory")?
            .join("vexa");
        Ok(Self {
            cache_root,
            index_options: IndexOptions::default(),
        })
    }
}

/// Prepare a verified context packet for a frontier-model request.
///
/// # Errors
///
/// Returns an error for an unsupported protocol operation/version, invalid repository root, index
/// failures, or persistence failures.
pub fn prepare(request: &PrepareRequest, config: &CoreConfig) -> Result<PrepareResponse> {
    prepare_with_options(request, config, PrepareOptions::default())
}

/// Prepare context for an additive host hook that already supplies the original user prompt.
///
/// # Errors
///
/// Returns the same errors as [`prepare`].
pub fn prepare_additive_context(
    request: &PrepareRequest,
    config: &CoreConfig,
) -> Result<PrepareResponse> {
    prepare_with_options(
        request,
        config,
        PrepareOptions {
            include_user_prompt_in_packet: false,
            include_protected_constraints_in_packet: false,
        },
    )
}

/// Prepare a verified packet using explicit packet-inclusion options.
///
/// # Errors
///
/// Returns an error for an unsupported protocol operation/version, invalid repository root, index
/// failures, or persistence failures.
#[allow(clippy::too_many_lines)]
pub fn prepare_with_options(
    request: &PrepareRequest,
    config: &CoreConfig,
    options: PrepareOptions,
) -> Result<PrepareResponse> {
    if request.vexa_api != VEXA_API_VERSION {
        bail!(
            "unsupported VEXA API version {} (expected {})",
            request.vexa_api,
            VEXA_API_VERSION
        );
    }
    if request.op != "prepare" {
        bail!("unsupported VEXA operation: {}", request.op);
    }
    if request.max_context_tokens < MIN_CONTEXT_TOKENS {
        bail!(
            "max_context_tokens must be at least {MIN_CONTEXT_TOKENS}, got {}",
            request.max_context_tokens
        );
    }

    let total_start = Instant::now();
    let root = discover_repository_root(&request.cwd)?;
    let profile = match request.mode {
        OperatingProfile::Auto => detect_profile(),
        explicit => explicit,
    };
    let index_path = index_path_for(&config.cache_root, &root);

    let index_start = Instant::now();
    let previous = load_snapshot(&index_path)
        .ok()
        .filter(|snapshot| snapshot.repository_root == root);
    let mut snapshot = build_index(&root, previous.as_ref(), &config.index_options)?;
    save_snapshot(&index_path, &snapshot)?;
    let index_ms = elapsed_ms(index_start);

    let search_start = Instant::now();
    let mut search_result = search(
        &snapshot,
        &request.prompt,
        &request.diagnostics,
        initial_limit(profile),
    );
    let search_ms = elapsed_ms(search_start);

    let evidence_start = Instant::now();
    let mut warnings = Vec::new();
    let stale = search_result
        .hits
        .iter()
        .any(|hit| hit_is_stale(&root, &snapshot, hit));
    if stale {
        warnings
            .push("index changed during preparation; refreshed before emitting source".to_owned());
        snapshot = build_index(&root, None, &config.index_options)?;
        save_snapshot(&index_path, &snapshot)?;
        search_result = search(
            &snapshot,
            &request.prompt,
            &request.diagnostics,
            initial_limit(profile),
        );
    }

    let mut selected_hits = search_result.hits.clone();
    for path in &request.include_paths {
        let record = snapshot
            .files
            .iter()
            .find(|file| file.path == *path)
            .with_context(|| format!("requested path is not indexed or is excluded: {path:?}"))?;
        if !selected_hits.iter().any(|hit| hit.path == *path) {
            selected_hits.push(SearchHit {
                path: path.clone(),
                score: 0.0,
                breakdown: ScoreBreakdown::default(),
                reasons: vec!["explicit_path".to_owned()],
                best_symbol: None,
                symbols: record.symbols.clone(),
            });
        }
    }
    if search_result.needs_expansion {
        append_expansion_hits(&mut selected_hits, &search_result, &snapshot);
        warnings.push(
            "retrieval confidence is low; VEXA expanded context instead of compressing aggressively"
                .to_owned(),
        );
    }
    let evidence = build_evidence(
        &root,
        &snapshot,
        &selected_hits,
        &request.prompt,
        &mut warnings,
    );
    let mut expansion_candidates = search_result.expansion_candidates.clone();
    expansion_candidates.extend(search_result.alternative_candidates.clone());
    if !expansion_candidates.is_empty() {
        warnings.push(format!(
            "Additional candidates can be requested with --include-path: {expansion_candidates:?}"
        ));
    }
    let evidence_ms = elapsed_ms(evidence_start);

    for diagnostic in &request.diagnostics {
        warnings.push(format!(
            "user-supplied diagnostic (not execution proof): {diagnostic}"
        ));
    }

    let pack_start = Instant::now();
    let packed = pack_with_options(
        &request.prompt,
        profile,
        search_result.confidence,
        evidence.clone(),
        request.max_context_tokens,
        warnings.clone(),
        PackOptions {
            include_user_prompt: options.include_user_prompt_in_packet,
            include_protected_constraints: options.include_protected_constraints_in_packet,
        },
    );
    let pack_ms = elapsed_ms(pack_start);

    let estimated_source_tokens = snapshot
        .files
        .iter()
        .map(|file| {
            usize::try_from(file.size_bytes)
                .unwrap_or(usize::MAX)
                .div_ceil(4)
        })
        .fold(0_usize, usize::saturating_add);
    let summaries = evidence
        .into_iter()
        .enumerate()
        .filter(|(index, _)| packed.included_indices.contains(index))
        .map(|(_, item)| EvidenceSummary {
            path: item.path,
            start_line: item.start_line,
            end_line: item.end_line,
            sha256: item.sha256,
            score: item.score,
        })
        .collect();

    let mut timings_ms = BTreeMap::new();
    timings_ms.insert("index".to_owned(), index_ms);
    timings_ms.insert("search".to_owned(), search_ms);
    timings_ms.insert("evidence".to_owned(), evidence_ms);
    timings_ms.insert("pack".to_owned(), pack_ms);
    timings_ms.insert("total".to_owned(), elapsed_ms(total_start));

    Ok(PrepareResponse {
        vexa_api: VEXA_API_VERSION.to_owned(),
        status: "ok".to_owned(),
        profile,
        confidence: search_result.confidence,
        estimated_source_tokens,
        estimated_packet_tokens: packed.estimated_tokens,
        packet: packed.packet,
        evidence: summaries,
        warnings,
        needs_expansion: search_result.needs_expansion || packed.needs_expansion,
        expansion_candidates,
        timings_ms,
    })
}

/// Refresh or rebuild the cached repository index.
///
/// # Errors
///
/// Returns an error when repository discovery, indexing, or snapshot persistence fails.
pub fn refresh_index(root: &Path, config: &CoreConfig, rebuild: bool) -> Result<IndexSnapshot> {
    let root = discover_repository_root(root)?;
    let path = index_path_for(&config.cache_root, &root);
    let previous = if rebuild {
        None
    } else {
        load_snapshot(&path).ok()
    };
    let snapshot = build_index(&root, previous.as_ref(), &config.index_options)?;
    save_snapshot(&path, &snapshot)?;
    Ok(snapshot)
}

/// Return the deterministic per-user cache path for a repository index.
///
/// # Errors
///
/// Returns an error when the repository root cannot be discovered or canonicalized.
pub fn cached_index_path(root: &Path, config: &CoreConfig) -> Result<PathBuf> {
    let root = discover_repository_root(root)?;
    Ok(index_path_for(&config.cache_root, &root))
}

#[must_use]
pub fn detect_profile() -> OperatingProfile {
    let cpu_count = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    let memory_bytes = system.total_memory();
    let gib = memory_bytes / (1024 * 1024 * 1024);
    if cpu_count >= 8 && gib >= 16 {
        OperatingProfile::Enhanced
    } else if cpu_count >= 4 && gib >= 8 {
        OperatingProfile::Standard
    } else {
        OperatingProfile::Lite
    }
}

fn discover_repository_root(cwd: &Path) -> Result<PathBuf> {
    let cwd = cwd
        .canonicalize()
        .with_context(|| format!("cannot canonicalize cwd {}", cwd.display()))?;
    if !cwd.is_dir() {
        bail!("cwd is not a directory: {}", cwd.display());
    }
    for candidate in cwd.ancestors() {
        if candidate.join(".git").exists() {
            return candidate
                .canonicalize()
                .with_context(|| format!("cannot canonicalize {}", candidate.display()));
        }
    }
    Ok(cwd)
}

fn index_path_for(cache_root: &Path, root: &Path) -> PathBuf {
    let key = hex::encode(Sha256::digest(root.to_string_lossy().as_bytes()));
    cache_root
        .join("indexes")
        .join(&key[..24])
        .join("index.json")
}

fn initial_limit(profile: OperatingProfile) -> usize {
    match profile {
        OperatingProfile::Lite => 6,
        OperatingProfile::Standard | OperatingProfile::Auto => 8,
        OperatingProfile::Enhanced => 12,
    }
}

fn hit_is_stale(root: &Path, snapshot: &IndexSnapshot, hit: &SearchHit) -> bool {
    let Some(record) = snapshot.files.iter().find(|file| file.path == hit.path) else {
        return true;
    };
    let Some(path) = safe_repository_path(root, &record.path) else {
        return true;
    };
    hash_file(&path).map_or(true, |hash| hash != record.sha256)
}

fn append_expansion_hits(
    hits: &mut Vec<SearchHit>,
    search: &SearchResult,
    snapshot: &IndexSnapshot,
) {
    for path in &search.expansion_candidates {
        if hits.iter().any(|hit| hit.path == *path) {
            continue;
        }
        if snapshot.files.iter().any(|file| file.path == *path) {
            hits.push(SearchHit {
                path: path.clone(),
                score: 0.001,
                breakdown: ScoreBreakdown::default(),
                reasons: vec!["confidence_expansion".to_owned()],
                best_symbol: None,
                symbols: snapshot
                    .files
                    .iter()
                    .find(|file| file.path == *path)
                    .map(|file| file.symbols.clone())
                    .unwrap_or_default(),
            });
        }
    }
}

fn build_evidence(
    root: &Path,
    snapshot: &IndexSnapshot,
    hits: &[SearchHit],
    prompt: &str,
    warnings: &mut Vec<String>,
) -> Vec<Evidence> {
    let query_terms = tokenize(prompt);
    let mut evidence = Vec::new();
    for hit in hits {
        let Some(record) = snapshot.files.iter().find(|file| file.path == hit.path) else {
            continue;
        };
        if record.sensitive {
            warnings.push(format!(
                "sensitive file excluded from packet: {}",
                record.path
            ));
            continue;
        }
        let Some(path) = safe_repository_path(root, &record.path) else {
            warnings.push(format!(
                "path escaped repository root and was excluded: {}",
                record.path
            ));
            continue;
        };
        let Ok(bytes) = fs::read(&path) else {
            warnings.push(format!(
                "could not verify current file hash: {}",
                record.path
            ));
            continue;
        };
        let current_hash = hex::encode(Sha256::digest(&bytes));
        if current_hash != record.sha256 {
            warnings.push(format!(
                "file changed after retrieval and was excluded: {}",
                record.path
            ));
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else {
            warnings.push(format!(
                "file is not valid UTF-8 and was excluded: {}",
                record.path
            ));
            continue;
        };
        if vexa_index::contains_secret(&text) {
            warnings.push(format!(
                "suspected secret content excluded: {}",
                record.path
            ));
            continue;
        }
        let lines: Vec<&str> = text.lines().collect();
        if lines.is_empty() {
            continue;
        }
        let (start, end) = evidence_range(hit, &lines, &query_terms);
        // Slice the same verified bytes, preserving original line endings.
        let excerpt: String = text
            .split_inclusive('\n')
            .skip(start - 1)
            .take(end - start + 1)
            .collect();
        let start_line = u32::try_from(start).unwrap_or(u32::MAX);
        let end_line = u32::try_from(end).unwrap_or(u32::MAX);
        let provenance = Provenance {
            path: record.path.clone(),
            start_line,
            end_line,
            sha256: current_hash.clone(),
            git_revision: snapshot.git_revision.clone(),
            observed_unix_ms: now_ms(),
            trust: TrustLevel::UntrustedRepositoryContent,
            verification: VerificationState::Verified,
        };
        evidence.push(Evidence {
            path: record.path.clone(),
            start_line,
            end_line,
            score: hit.score,
            sha256: current_hash,
            excerpt,
            provenance,
        });
    }
    evidence
}

fn evidence_range(hit: &SearchHit, lines: &[&str], query_terms: &[String]) -> (usize, usize) {
    if let Some(symbol) = &hit.best_symbol {
        let symbol_start = usize::try_from(symbol.start_line).unwrap_or(1).max(1);
        let symbol_end = usize::try_from(symbol.end_line)
            .unwrap_or(symbol_start)
            .max(symbol_start);
        let start = symbol_start.saturating_sub(8).max(1);
        let end = symbol_end.saturating_add(12).min(lines.len());
        return cap_range(start, end, lines.len());
    }

    let match_line = lines.iter().position(|line| {
        let lower = line.to_lowercase();
        query_terms.iter().any(|term| lower.contains(term))
    });
    let center = match_line.map_or(1, |index| index + 1);
    let start = center.saturating_sub(20).max(1);
    let end = center.saturating_add(40).min(lines.len());
    cap_range(start, end, lines.len())
}

fn cap_range(start: usize, end: usize, total: usize) -> (usize, usize) {
    if end.saturating_sub(start).saturating_add(1) <= MAX_EXCERPT_LINES {
        return (start, end);
    }
    (
        start,
        start.saturating_add(MAX_EXCERPT_LINES - 1).min(total),
    )
}

fn safe_repository_path(root: &Path, relative: &str) -> Option<PathBuf> {
    if relative.is_empty() || Path::new(relative).is_absolute() {
        return None;
    }
    if relative
        .split('/')
        .any(|component| component == ".." || component.is_empty())
    {
        return None;
    }
    let canonical_root = root.canonicalize().ok()?;
    let candidate = canonical_root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
    let canonical = candidate.canonicalize().ok()?;
    canonical.starts_with(&canonical_root).then_some(canonical)
}

fn tokenize(value: &str) -> Vec<String> {
    value
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|term| term.len() >= 2)
        .map(str::to_lowercase)
        .collect()
}

fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1_000.0
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_expansion_includes_source_without_bypassing_sensitive_exclusions() -> Result<()> {
        let repo = tempfile::tempdir()?;
        fs::create_dir(repo.path().join(".git"))?;
        fs::write(repo.path().join("entry.rs"), "fn entry() {}\n")?;
        fs::write(repo.path().join("extra.rs"), "fn hidden() {}\n")?;
        fs::write(
            repo.path().join(".env"),
            "PRIVATE_TOKEN=synthetic-fixture-secret\n",
        )?;
        let cache = tempfile::tempdir()?;
        let config = CoreConfig {
            cache_root: cache.path().to_path_buf(),
            index_options: IndexOptions::default(),
        };
        let mut request: PrepareRequest = serde_json::from_value(serde_json::json!({
            "cwd": repo.path(), "prompt": "entry", "mode": "lite", "include_paths": ["extra.rs"]
        }))?;
        let response = prepare(&request, &config)?;
        assert!(response.evidence.iter().any(|item| item.path == "extra.rs"));
        request.include_paths = vec![".env".to_owned()];
        assert!(prepare(&request, &config).is_err());
        request.include_paths = vec!["../outside.rs".to_owned()];
        assert!(prepare(&request, &config).is_err());
        Ok(())
    }

    #[test]
    fn metadata_reports_only_emitted_evidence() -> Result<()> {
        let dir = tempfile::tempdir()?;
        fs::create_dir(dir.path().join(".git"))?;
        fs::write(dir.path().join("lib.rs"), "fn lookup() {}\n".repeat(80))?;
        let config = CoreConfig {
            cache_root: dir.path().join(".cache"),
            index_options: IndexOptions::default(),
        };
        let request: PrepareRequest = serde_json::from_value(serde_json::json!({
            "cwd": dir.path(), "prompt": "lookup", "max_context_tokens": 128, "mode": "lite"
        }))?;
        let response = prepare(&request, &config)?;
        assert!(response.evidence.is_empty());
        assert!(response.needs_expansion);
        assert!(!response.packet.contains("fn lookup()"));
        Ok(())
    }

    #[test]
    fn excerpts_preserve_crlf_from_verified_bytes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        fs::create_dir(dir.path().join(".git"))?;
        fs::write(
            dir.path().join("lib.rs"),
            "fn lookup() {\r\n    // original\r\n}\r\n",
        )?;
        let config = CoreConfig {
            cache_root: dir.path().join(".cache"),
            index_options: IndexOptions::default(),
        };
        let request: PrepareRequest = serde_json::from_value(serde_json::json!({
            "cwd": dir.path(), "prompt": "lookup", "max_context_tokens": 1800, "mode": "lite"
        }))?;
        let response = prepare(&request, &config)?;
        assert!(
            response
                .packet
                .contains("fn lookup() {\r\n    // original\r\n}\r\n")
        );
        Ok(())
    }

    #[test]
    fn profile_detection_always_resolves_auto() {
        assert_ne!(detect_profile(), OperatingProfile::Auto);
    }

    #[test]
    fn prepare_emits_verified_exact_source() -> Result<()> {
        let repo = tempfile::tempdir()?;
        fs::create_dir(repo.path().join(".git"))?;
        fs::create_dir(repo.path().join("src"))?;
        fs::write(
            repo.path().join("src/auth.rs"),
            "pub fn validate_token(token: &str) -> bool { !token.is_empty() }\n",
        )?;
        let cache = tempfile::tempdir()?;
        let config = CoreConfig {
            cache_root: cache.path().to_path_buf(),
            index_options: IndexOptions::default(),
        };
        let request = PrepareRequest {
            vexa_api: VEXA_API_VERSION.to_owned(),
            op: "prepare".to_owned(),
            cwd: repo.path().to_path_buf(),
            prompt: "Fix validate_token. MUST keep the public signature.".to_owned(),
            model: Some("future-model".to_owned()),
            mode: OperatingProfile::Lite,
            max_context_tokens: 1800,
            diagnostics: Vec::new(),
            include_paths: Vec::new(),
        };

        let response = prepare(&request, &config)?;
        assert_eq!(response.status, "ok");
        assert!(response.packet.contains("validate_token"));
        assert!(response.packet.contains("MUST keep the public signature"));
        assert!(response.packet.contains("UNTRUSTED_REPOSITORY_EVIDENCE"));
        assert!(!response.evidence.is_empty());
        Ok(())
    }

    #[test]
    fn additive_prepare_avoids_prompt_duplication() -> Result<()> {
        let repo = tempfile::tempdir()?;
        fs::create_dir(repo.path().join(".git"))?;
        fs::write(repo.path().join("auth.rs"), "fn refresh_token() {}\n")?;
        let cache = tempfile::tempdir()?;
        let config = CoreConfig {
            cache_root: cache.path().to_path_buf(),
            index_options: IndexOptions::default(),
        };
        let prompt = "Fix refresh_token. MUST preserve API v2.";
        let request = PrepareRequest {
            vexa_api: VEXA_API_VERSION.to_owned(),
            op: "prepare".to_owned(),
            cwd: repo.path().to_path_buf(),
            prompt: prompt.to_owned(),
            model: None,
            mode: OperatingProfile::Lite,
            max_context_tokens: 1200,
            diagnostics: Vec::new(),
            include_paths: Vec::new(),
        };
        let response = prepare_additive_context(&request, &config)?;
        assert!(!response.packet.contains(prompt));
        assert!(response.packet.contains("refresh_token"));
        Ok(())
    }

    #[test]
    fn path_escape_is_rejected() -> Result<()> {
        let repo = tempfile::tempdir()?;
        fs::write(repo.path().join("inside.rs"), "fn inside() {}")?;
        assert!(safe_repository_path(repo.path(), "../outside.rs").is_none());
        assert!(safe_repository_path(repo.path(), "/absolute.rs").is_none());
        assert!(safe_repository_path(repo.path(), "inside.rs").is_some());
        Ok(())
    }
}
