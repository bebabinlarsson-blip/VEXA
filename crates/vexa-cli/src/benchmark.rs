//! Deterministic local benchmark runner for the repository fixture corpus.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;
use tempfile::TempDir;
use vexa_index::IndexOptions;

pub const DEFAULT_BENCHMARK_SUITE: &str = "benchmarks/benchmark-suite.v1.json";

const SEEK_LIMIT: usize = 6;
const UNTRUSTED_PACKET_MARKER: &str = "UNTRUSTED_REPOSITORY_EVIDENCE";

#[derive(Debug)]
pub struct BenchmarkError {
    message: String,
}

impl BenchmarkError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for BenchmarkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for BenchmarkError {}

impl From<io::Error> for BenchmarkError {
    fn from(error: io::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<serde_json::Error> for BenchmarkError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(error.to_string())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkReport {
    pub context_budget_tokens: usize,
    pub requested_source_fraction: Option<f64>,
    pub suite_id: String,
    pub suite_path: String,
    pub tasks: Vec<BenchmarkTaskReport>,
    pub aggregate: BenchmarkAggregate,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkAggregate {
    pub task_count: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub errors: usize,
    pub packet_relevant_file_recall: Option<f64>,
    pub packet_relevant_symbol_recall: Option<f64>,
    pub seek_relevant_file_recall: Option<f64>,
    pub seek_relevant_symbol_recall: Option<f64>,
    pub guard_tasks: usize,
    pub guard_tasks_passed: usize,
    pub median_packet_token_reduction: Option<f64>,
    pub p50_prepare_ms: Option<f64>,
    pub p95_prepare_ms: Option<f64>,
    pub latency_percentile_method: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkTaskStatus {
    Passed,
    Failed,
    Skipped,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkTaskReport {
    pub id: String,
    pub category: String,
    pub language: String,
    pub status: BenchmarkTaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub expected_relevant_files: Vec<String>,
    pub expected_relevant_symbols: Vec<SymbolExpectation>,
    pub packet_files: Vec<String>,
    pub seek_hits: Vec<SeekHitReport>,
    pub retrieval: RetrievalMetrics,
    pub guards: GuardReport,
    pub tokens: TokenMetrics,
    pub latency: LatencyMetrics,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_reduction: Option<LogReductionReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeekHitReport {
    pub path: String,
    pub score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_symbol: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RetrievalMetrics {
    pub packet_relevant_files_found: Vec<String>,
    pub packet_relevant_files_missing: Vec<String>,
    pub packet_relevant_file_recall: Option<f64>,
    pub packet_relevant_symbols_found: Vec<SymbolExpectation>,
    pub packet_relevant_symbols_missing: Vec<SymbolExpectation>,
    pub packet_relevant_symbol_recall: Option<f64>,
    pub seek_relevant_files_found: Vec<String>,
    pub seek_relevant_files_missing: Vec<String>,
    pub seek_relevant_file_recall: Option<f64>,
    pub seek_relevant_symbols_found: Vec<SymbolExpectation>,
    pub seek_relevant_symbols_missing: Vec<SymbolExpectation>,
    pub seek_relevant_symbol_recall: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct GuardReport {
    pub configured: bool,
    pub passed: bool,
    pub forbidden_paths: Vec<String>,
    pub forbidden_path_hits: Vec<String>,
    pub forbidden_symbols: Vec<SymbolExpectation>,
    pub forbidden_symbol_hits: Vec<SymbolExpectation>,
    pub repository_content_is_untrusted_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub untrusted_evidence_marker_present: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TokenMetrics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_source_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_packet_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub packet_token_reduction: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct LatencyMetrics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prepare_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub core_reported_total_ms: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogReductionReport {
    pub status: String,
    pub lines_scanned: usize,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_error: Option<String>,
    pub causal_error_count: usize,
    pub test_observation_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolExpectation {
    pub path: String,
    pub symbol: String,
}

#[derive(Debug, Deserialize)]
struct SuiteManifest {
    schema_version: u32,
    suite_id: String,
    tasks: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct TaskManifest {
    schema_version: u32,
    id: String,
    category: String,
    language: String,
    fixture_repo: String,
    prompt: String,
    scenario: Scenario,
    expected: Expected,
    #[serde(default)]
    guards: Guards,
}

#[derive(Debug, Clone, Deserialize)]
struct Expected {
    relevant_files: Vec<String>,
    relevant_symbols: Vec<SymbolExpectation>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct Guards {
    #[serde(default)]
    forbidden_paths: Vec<String>,
    #[serde(default)]
    forbidden_symbols: Vec<SymbolExpectation>,
    #[serde(default)]
    repository_content_is_untrusted: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Scenario {
    SingleQuery,
    IndexThenMutate { mutation: Mutation },
    SymlinkEscape { symlink: SymlinkScenario },
}

#[derive(Debug, Clone, Deserialize)]
struct Mutation {
    after_step: String,
    operation: String,
    target_path: String,
    source_path: String,
}

#[derive(Debug, Clone, Deserialize)]
struct SymlinkScenario {
    link_path: String,
    target_path: String,
    #[serde(default)]
    skip_if: Vec<String>,
}

enum SetupOutcome {
    Ready,
    Skip(String),
}

/// Run the repository's default deterministic benchmark suite.
///
/// `repository_root` is the VEXA checkout containing `benchmarks/benchmark-suite.v1.json`.
/// Each task is staged in a fresh temporary directory and uses an isolated VEXA index cache.
/// Task execution errors are captured in the serializable report so one bad fixture does not hide
/// results from the rest of the suite. Only suite/manifest loading failures are returned as `Err`.
pub fn run_benchmark_suite(
    repository_root: &Path,
    context_tokens: usize,
    source_fraction: Option<f64>,
) -> Result<BenchmarkReport, BenchmarkError> {
    let repository_root = repository_root.canonicalize().map_err(|error| {
        BenchmarkError::new(format!(
            "cannot canonicalize benchmark repository root {}: {error}",
            repository_root.display()
        ))
    })?;
    let benchmarks_root = repository_root
        .join("benchmarks")
        .canonicalize()
        .map_err(|error| {
            BenchmarkError::new(format!(
                "cannot locate benchmarks directory below {}: {error}",
                repository_root.display()
            ))
        })?;
    let suite_path = repository_root.join(DEFAULT_BENCHMARK_SUITE);
    let suite_bytes = fs::read(&suite_path).map_err(|error| {
        BenchmarkError::new(format!("cannot read {}: {error}", suite_path.display()))
    })?;
    let suite: SuiteManifest = serde_json::from_slice(&suite_bytes).map_err(|error| {
        BenchmarkError::new(format!(
            "invalid benchmark suite {}: {error}",
            suite_path.display()
        ))
    })?;
    if suite.schema_version != 1 {
        return Err(BenchmarkError::new(format!(
            "unsupported benchmark suite schema {}",
            suite.schema_version
        )));
    }

    let mut reports = Vec::with_capacity(suite.tasks.len());
    for task_relative in &suite.tasks {
        let task_path = resolve_existing_under(&benchmarks_root, task_relative)?;
        let task_bytes = fs::read(&task_path).map_err(|error| {
            BenchmarkError::new(format!("cannot read {}: {error}", task_path.display()))
        })?;
        let task: TaskManifest = serde_json::from_slice(&task_bytes).map_err(|error| {
            BenchmarkError::new(format!(
                "invalid benchmark task {}: {error}",
                task_path.display()
            ))
        })?;
        if task.schema_version != 1 {
            reports.push(error_report(
                &task,
                format!("unsupported task schema {}", task.schema_version),
            ));
            continue;
        }

        reports.push(run_task(
            &benchmarks_root,
            &task,
            context_tokens,
            source_fraction,
        ));
    }

    let aggregate = aggregate(&reports);
    Ok(BenchmarkReport {
        context_budget_tokens: context_tokens,
        requested_source_fraction: source_fraction,
        suite_id: suite.suite_id,
        suite_path: DEFAULT_BENCHMARK_SUITE.to_owned(),
        tasks: reports,
        aggregate,
    })
}

fn run_task(
    benchmarks_root: &Path,
    task: &TaskManifest,
    context_tokens: usize,
    source_fraction: Option<f64>,
) -> BenchmarkTaskReport {
    match execute_task(benchmarks_root, task, context_tokens, source_fraction) {
        Ok(report) => report,
        Err(error) => error_report(task, error.to_string()),
    }
}

#[allow(clippy::too_many_lines)]
fn execute_task(
    benchmarks_root: &Path,
    task: &TaskManifest,
    context_tokens: usize,
    source_fraction: Option<f64>,
) -> Result<BenchmarkTaskReport, BenchmarkError> {
    let fixture_source = resolve_existing_under(benchmarks_root, &task.fixture_repo)?;
    if !fixture_source.is_dir() {
        return Err(BenchmarkError::new(format!(
            "fixture repo is not a directory: {}",
            fixture_source.display()
        )));
    }

    let temp = tempfile::tempdir().map_err(|error| {
        BenchmarkError::new(format!(
            "cannot create temporary benchmark directory: {error}"
        ))
    })?;
    let staged_repo = temp.path().join("repo");
    copy_directory(&fixture_source, &staged_repo)?;
    fs::create_dir_all(staged_repo.join(".git")).map_err(|error| {
        BenchmarkError::new(format!(
            "cannot create isolated repository boundary for {}: {error}",
            task.id
        ))
    })?;

    if let Scenario::SymlinkEscape { symlink } = &task.scenario {
        match stage_symlink_scenario(
            benchmarks_root,
            &fixture_source,
            &staged_repo,
            &temp,
            symlink,
        )? {
            SetupOutcome::Ready => {}
            SetupOutcome::Skip(reason) => return Ok(skipped_report(task, reason)),
        }
    }

    let config = vexa_core::CoreConfig {
        cache_root: temp.path().join("cache"),
        index_options: IndexOptions::default(),
    };
    let initial_snapshot =
        vexa_core::refresh_index(&staged_repo, &config, true).map_err(|error| {
            BenchmarkError::new(format!("fresh index failed for {}: {error}", task.id))
        })?;
    let context_tokens = source_fraction.map_or(context_tokens, |fraction| {
        let source_bytes: u64 = initial_snapshot
            .files
            .iter()
            .map(|file| file.size_bytes)
            .sum();
        let source_tokens = source_bytes.div_ceil(4);
        // This is the requested byte-estimate budget, not an achieved compression percentage.
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let tokens = (source_tokens as f64 * fraction).ceil() as usize;
        tokens.max(128)
    });

    if let Scenario::IndexThenMutate { mutation } = &task.scenario {
        apply_mutation(benchmarks_root, &staged_repo, mutation)?;
    }

    let request = serde_json::from_value(serde_json::json!({
        "vexa_api": "1",
        "op": "prepare",
        "cwd": staged_repo,
        "prompt": task.prompt,
        "model": null,
        "mode": "lite",
        "max_context_tokens": context_tokens,
        "diagnostics": []
    }))
    .map_err(|error| BenchmarkError::new(format!("cannot build prepare request: {error}")))?;

    let prepare_start = Instant::now();
    let response = vexa_core::prepare(&request, &config)
        .map_err(|error| BenchmarkError::new(format!("prepare failed for {}: {error}", task.id)))?;
    let prepare_ms = prepare_start.elapsed().as_secs_f64() * 1_000.0;

    let index_path = vexa_core::cached_index_path(&staged_repo, &config).map_err(|error| {
        BenchmarkError::new(format!(
            "cannot locate post-prepare index for {}: {error}",
            task.id
        ))
    })?;
    let current_snapshot = vexa_index::load_snapshot(&index_path).map_err(|error| {
        BenchmarkError::new(format!(
            "cannot load post-prepare index for {}: {error}",
            task.id
        ))
    })?;
    let search_result = vexa_seek::search(&current_snapshot, &task.prompt, &[], SEEK_LIMIT);

    let packet_files: Vec<String> = response
        .evidence
        .iter()
        .map(|evidence| evidence.path.clone())
        .collect();
    let packet_file_set: BTreeSet<&str> = packet_files.iter().map(String::as_str).collect();
    let seek_hits: Vec<SeekHitReport> = search_result
        .hits
        .iter()
        .map(|hit| SeekHitReport {
            path: hit.path.clone(),
            score: hit.score,
            best_symbol: hit.best_symbol.as_ref().map(|symbol| symbol.name.clone()),
        })
        .collect();
    let seek_file_set: BTreeSet<&str> = seek_hits.iter().map(|hit| hit.path.as_str()).collect();

    let mut evidence_ranges = BTreeMap::<String, Vec<(u32, u32)>>::new();
    for evidence in &response.evidence {
        evidence_ranges
            .entry(evidence.path.clone())
            .or_default()
            .push((evidence.start_line, evidence.end_line));
    }

    let packet_files_found =
        matching_strings(&task.expected.relevant_files, &packet_file_set, true);
    let packet_files_missing =
        matching_strings(&task.expected.relevant_files, &packet_file_set, false);
    let seek_files_found = matching_strings(&task.expected.relevant_files, &seek_file_set, true);
    let seek_files_missing = matching_strings(&task.expected.relevant_files, &seek_file_set, false);

    let mut packet_symbols_found = Vec::new();
    let mut packet_symbols_missing = Vec::new();
    let mut seek_symbols_found = Vec::new();
    let mut seek_symbols_missing = Vec::new();
    for expected in &task.expected.relevant_symbols {
        let verified = vexa_verify::verify_symbol(
            &staged_repo,
            &current_snapshot,
            &expected.path,
            &expected.symbol,
        );
        let index_has_live_symbol = verified.state == vexa_verify::ClaimState::Verified;
        let packet_has_symbol = evidence_ranges.get(&expected.path).is_some_and(|ranges| {
            source_ranges_contain_symbol(&staged_repo, &expected.path, ranges, &expected.symbol)
                .unwrap_or(false)
        });
        if index_has_live_symbol && packet_has_symbol {
            packet_symbols_found.push(expected.clone());
        } else {
            packet_symbols_missing.push(expected.clone());
        }

        let seek_has_symbol = search_result.hits.iter().any(|hit| {
            hit.path == expected.path
                && hit
                    .symbols
                    .iter()
                    .any(|symbol| symbol.name == expected.symbol)
        });
        if index_has_live_symbol && seek_has_symbol {
            seek_symbols_found.push(expected.clone());
        } else {
            seek_symbols_missing.push(expected.clone());
        }
    }

    let retrieval = RetrievalMetrics {
        packet_relevant_file_recall: recall(
            packet_files_found.len(),
            task.expected.relevant_files.len(),
        ),
        packet_relevant_symbol_recall: recall(
            packet_symbols_found.len(),
            task.expected.relevant_symbols.len(),
        ),
        seek_relevant_file_recall: recall(
            seek_files_found.len(),
            task.expected.relevant_files.len(),
        ),
        seek_relevant_symbol_recall: recall(
            seek_symbols_found.len(),
            task.expected.relevant_symbols.len(),
        ),
        packet_relevant_files_found: packet_files_found,
        packet_relevant_files_missing: packet_files_missing,
        packet_relevant_symbols_found: packet_symbols_found,
        packet_relevant_symbols_missing: packet_symbols_missing,
        seek_relevant_files_found: seek_files_found,
        seek_relevant_files_missing: seek_files_missing,
        seek_relevant_symbols_found: seek_symbols_found,
        seek_relevant_symbols_missing: seek_symbols_missing,
    };

    let guards = evaluate_guards(task, &staged_repo, &evidence_ranges, &response.packet);
    let tokens = TokenMetrics {
        estimated_source_tokens: Some(response.estimated_source_tokens),
        estimated_packet_tokens: Some(response.estimated_packet_tokens),
        packet_token_reduction: token_reduction(
            response.estimated_source_tokens,
            response.estimated_packet_tokens,
        ),
    };
    let latency = LatencyMetrics {
        prepare_ms: Some(prepare_ms),
        core_reported_total_ms: response.timings_ms.get("total").copied(),
    };
    let log_reduction = if task.id == "adversarial-giant-log" {
        Some(reduce_giant_log(&staged_repo)?)
    } else {
        None
    };

    let packet_recall_passed = retrieval.packet_relevant_files_missing.is_empty()
        && retrieval.packet_relevant_symbols_missing.is_empty();
    let status = if packet_recall_passed && guards.passed {
        BenchmarkTaskStatus::Passed
    } else {
        BenchmarkTaskStatus::Failed
    };

    Ok(BenchmarkTaskReport {
        id: task.id.clone(),
        category: task.category.clone(),
        language: task.language.clone(),
        status,
        skip_reason: None,
        error: None,
        expected_relevant_files: task.expected.relevant_files.clone(),
        expected_relevant_symbols: task.expected.relevant_symbols.clone(),
        packet_files,
        seek_hits,
        retrieval,
        guards,
        tokens,
        latency,
        log_reduction,
    })
}

fn evaluate_guards(
    task: &TaskManifest,
    staged_repo: &Path,
    evidence_ranges: &BTreeMap<String, Vec<(u32, u32)>>,
    packet: &str,
) -> GuardReport {
    let configured = !task.guards.forbidden_paths.is_empty()
        || !task.guards.forbidden_symbols.is_empty()
        || task.guards.repository_content_is_untrusted;
    let forbidden_path_hits: Vec<String> = task
        .guards
        .forbidden_paths
        .iter()
        .filter(|path| evidence_ranges.contains_key(path.as_str()))
        .cloned()
        .collect();
    let forbidden_symbol_hits: Vec<SymbolExpectation> = task
        .guards
        .forbidden_symbols
        .iter()
        .filter(|expectation| {
            evidence_ranges
                .get(&expectation.path)
                .is_some_and(|ranges| {
                    source_ranges_contain_symbol(
                        staged_repo,
                        &expectation.path,
                        ranges,
                        &expectation.symbol,
                    )
                    .unwrap_or(false)
                })
        })
        .cloned()
        .collect();
    let untrusted_evidence_marker_present = task
        .guards
        .repository_content_is_untrusted
        .then(|| packet.contains(UNTRUSTED_PACKET_MARKER));
    let trust_guard_passed = untrusted_evidence_marker_present.unwrap_or(true);
    let passed =
        forbidden_path_hits.is_empty() && forbidden_symbol_hits.is_empty() && trust_guard_passed;

    GuardReport {
        configured,
        passed,
        forbidden_paths: task.guards.forbidden_paths.clone(),
        forbidden_path_hits,
        forbidden_symbols: task.guards.forbidden_symbols.clone(),
        forbidden_symbol_hits,
        repository_content_is_untrusted_required: task.guards.repository_content_is_untrusted,
        untrusted_evidence_marker_present,
    }
}

fn reduce_giant_log(staged_repo: &Path) -> Result<LogReductionReport, BenchmarkError> {
    let log_path = staged_repo.join("logs/build.log");
    let raw = fs::read_to_string(&log_path).map_err(|error| {
        BenchmarkError::new(format!(
            "cannot read giant log {}: {error}",
            log_path.display()
        ))
    })?;
    let summary = vexa_verify::reduce_log(&raw, None);
    Ok(LogReductionReport {
        status: format!("{:?}", summary.status).to_ascii_lowercase(),
        lines_scanned: summary.lines_scanned,
        truncated: summary.truncated,
        first_error: summary.first_error.map(|diagnostic| diagnostic.text),
        causal_error_count: summary.causal_errors.len(),
        test_observation_count: summary.tests.len(),
    })
}

fn stage_symlink_scenario(
    benchmarks_root: &Path,
    fixture_source: &Path,
    staged_repo: &Path,
    temp: &TempDir,
    scenario: &SymlinkScenario,
) -> Result<SetupOutcome, BenchmarkError> {
    let link_path = safe_join_relative(staged_repo, &scenario.link_path)?;
    let source_target = fixture_source
        .join(&scenario.target_path)
        .canonicalize()
        .map_err(|error| {
            BenchmarkError::new(format!(
                "cannot resolve symlink fixture target {}: {error}",
                scenario.target_path
            ))
        })?;
    if !source_target.starts_with(benchmarks_root) || !source_target.is_file() {
        return Err(BenchmarkError::new(format!(
            "symlink fixture target must be an existing file below benchmarks/: {}",
            source_target.display()
        )));
    }

    let staged_target = join_with_parents_within(temp.path(), staged_repo, &scenario.target_path)?;
    let parent = staged_target.parent().ok_or_else(|| {
        BenchmarkError::new(format!(
            "symlink target has no parent: {}",
            staged_target.display()
        ))
    })?;
    fs::create_dir_all(parent)?;
    fs::copy(&source_target, &staged_target).map_err(|error| {
        BenchmarkError::new(format!(
            "cannot stage symlink target {}: {error}",
            staged_target.display()
        ))
    })?;

    if let Some(parent) = link_path.parent() {
        fs::create_dir_all(parent)?;
    }
    match create_file_symlink(Path::new(&scenario.target_path), &link_path) {
        Ok(()) => Ok(SetupOutcome::Ready),
        Err(error) if symlink_is_unsupported(&error, scenario) => Ok(SetupOutcome::Skip(
            "symlink_creation_unsupported".to_owned(),
        )),
        Err(error) => Err(BenchmarkError::new(format!(
            "cannot create benchmark symlink {} -> {}: {error}",
            link_path.display(),
            scenario.target_path
        ))),
    }
}

#[cfg(unix)]
fn create_file_symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_file_symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[cfg(not(any(unix, windows)))]
fn create_file_symlink(_target: &Path, _link: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "symbolic links are unsupported on this platform",
    ))
}

fn symlink_is_unsupported(error: &io::Error, scenario: &SymlinkScenario) -> bool {
    if error.kind() == io::ErrorKind::Unsupported {
        return true;
    }
    #[cfg(windows)]
    {
        const ERROR_PRIVILEGE_NOT_HELD: i32 = 1_314;
        scenario
            .skip_if
            .iter()
            .any(|reason| reason == "windows_symlink_creation_unsupported")
            && error.raw_os_error() == Some(ERROR_PRIVILEGE_NOT_HELD)
    }
    #[cfg(not(windows))]
    {
        // This reason is Windows-specific; consuming the contract field here keeps Unix builds checked.
        let _ = &scenario.skip_if;
        false
    }
}

fn apply_mutation(
    benchmarks_root: &Path,
    staged_repo: &Path,
    mutation: &Mutation,
) -> Result<(), BenchmarkError> {
    if mutation.after_step != "index" || mutation.operation != "replace_file" {
        return Err(BenchmarkError::new(format!(
            "unsupported mutation: after_step={}, operation={}",
            mutation.after_step, mutation.operation
        )));
    }
    let source = resolve_existing_under(benchmarks_root, &mutation.source_path)?;
    if !source.is_file() {
        return Err(BenchmarkError::new(format!(
            "mutation source is not a file: {}",
            source.display()
        )));
    }
    let target = safe_join_relative(staged_repo, &mutation.target_path)?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(&source, &target).map_err(|error| {
        BenchmarkError::new(format!(
            "cannot apply mutation {} -> {}: {error}",
            source.display(),
            target.display()
        ))
    })?;
    Ok(())
}

fn copy_directory(source: &Path, destination: &Path) -> Result<(), BenchmarkError> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_directory(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &target)?;
        } else if file_type.is_symlink() {
            return Err(BenchmarkError::new(format!(
                "fixture source contains an unexpected symlink: {}",
                entry.path().display()
            )));
        }
    }
    Ok(())
}

fn resolve_existing_under(root: &Path, relative: &str) -> Result<PathBuf, BenchmarkError> {
    let candidate = safe_join_relative(root, relative)?;
    let canonical = candidate.canonicalize().map_err(|error| {
        BenchmarkError::new(format!("cannot resolve {}: {error}", candidate.display()))
    })?;
    if !canonical.starts_with(root) {
        return Err(BenchmarkError::new(format!(
            "path escapes benchmark root: {}",
            candidate.display()
        )));
    }
    Ok(canonical)
}

fn safe_join_relative(root: &Path, relative: &str) -> Result<PathBuf, BenchmarkError> {
    let relative = Path::new(relative);
    if relative.as_os_str().is_empty() || relative.is_absolute() {
        return Err(BenchmarkError::new(format!(
            "expected a non-empty relative path: {}",
            relative.display()
        )));
    }
    if relative
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(BenchmarkError::new(format!(
            "relative path contains unsafe components: {}",
            relative.display()
        )));
    }
    Ok(root.join(relative))
}

fn join_with_parents_within(
    root: &Path,
    base: &Path,
    relative: &str,
) -> Result<PathBuf, BenchmarkError> {
    let base_relative = base.strip_prefix(root).map_err(|_| {
        BenchmarkError::new(format!(
            "base {} is outside temporary root {}",
            base.display(),
            root.display()
        ))
    })?;
    let mut parts: Vec<_> = base_relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_os_string()),
            _ => None,
        })
        .collect();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(value) => parts.push(value.to_os_string()),
            Component::ParentDir => {
                if parts.pop().is_none() {
                    return Err(BenchmarkError::new(format!(
                        "relative target escapes temporary root: {relative}"
                    )));
                }
            }
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => {
                return Err(BenchmarkError::new(format!(
                    "symlink target must be relative: {relative}"
                )));
            }
        }
    }
    Ok(parts
        .into_iter()
        .fold(root.to_path_buf(), |path, part| path.join(part)))
}

fn source_ranges_contain_symbol(
    repository_root: &Path,
    relative_path: &str,
    ranges: &[(u32, u32)],
    symbol: &str,
) -> Result<bool, BenchmarkError> {
    let path = safe_join_relative(repository_root, relative_path)?;
    let source = fs::read_to_string(&path).map_err(|error| {
        BenchmarkError::new(format!(
            "cannot read retrieved source {}: {error}",
            path.display()
        ))
    })?;
    let lines: Vec<&str> = source.lines().collect();
    for (start_line, end_line) in ranges {
        let start = usize::try_from(*start_line)
            .unwrap_or(usize::MAX)
            .saturating_sub(1);
        let end = usize::try_from(*end_line)
            .unwrap_or(usize::MAX)
            .min(lines.len());
        if start < end && lines[start..end].iter().any(|line| line.contains(symbol)) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn matching_strings(expected: &[String], actual: &BTreeSet<&str>, present: bool) -> Vec<String> {
    expected
        .iter()
        .filter(|value| actual.contains(value.as_str()) == present)
        .cloned()
        .collect()
}

fn recall(found: usize, expected: usize) -> Option<f64> {
    if expected == 0 {
        None
    } else {
        let found = u32::try_from(found).ok()?;
        let expected = u32::try_from(expected).ok()?;
        Some(f64::from(found) / f64::from(expected))
    }
}

fn token_reduction(source_tokens: usize, packet_tokens: usize) -> Option<f64> {
    if source_tokens == 0 {
        None
    } else {
        let packet_tokens = u32::try_from(packet_tokens).ok()?;
        let source_tokens = u32::try_from(source_tokens).ok()?;
        Some(1.0 - f64::from(packet_tokens) / f64::from(source_tokens))
    }
}

fn skipped_report(task: &TaskManifest, reason: String) -> BenchmarkTaskReport {
    BenchmarkTaskReport {
        id: task.id.clone(),
        category: task.category.clone(),
        language: task.language.clone(),
        status: BenchmarkTaskStatus::Skipped,
        skip_reason: Some(reason),
        error: None,
        expected_relevant_files: task.expected.relevant_files.clone(),
        expected_relevant_symbols: task.expected.relevant_symbols.clone(),
        packet_files: Vec::new(),
        seek_hits: Vec::new(),
        retrieval: RetrievalMetrics::default(),
        guards: empty_guard_report(task),
        tokens: TokenMetrics::default(),
        latency: LatencyMetrics::default(),
        log_reduction: None,
    }
}

fn error_report(task: &TaskManifest, error: String) -> BenchmarkTaskReport {
    BenchmarkTaskReport {
        id: task.id.clone(),
        category: task.category.clone(),
        language: task.language.clone(),
        status: BenchmarkTaskStatus::Error,
        skip_reason: None,
        error: Some(error),
        expected_relevant_files: task.expected.relevant_files.clone(),
        expected_relevant_symbols: task.expected.relevant_symbols.clone(),
        packet_files: Vec::new(),
        seek_hits: Vec::new(),
        retrieval: RetrievalMetrics::default(),
        guards: empty_guard_report(task),
        tokens: TokenMetrics::default(),
        latency: LatencyMetrics::default(),
        log_reduction: None,
    }
}

fn empty_guard_report(task: &TaskManifest) -> GuardReport {
    let configured = !task.guards.forbidden_paths.is_empty()
        || !task.guards.forbidden_symbols.is_empty()
        || task.guards.repository_content_is_untrusted;
    GuardReport {
        configured,
        passed: false,
        forbidden_paths: task.guards.forbidden_paths.clone(),
        forbidden_path_hits: Vec::new(),
        forbidden_symbols: task.guards.forbidden_symbols.clone(),
        forbidden_symbol_hits: Vec::new(),
        repository_content_is_untrusted_required: task.guards.repository_content_is_untrusted,
        untrusted_evidence_marker_present: None,
    }
}

fn aggregate(tasks: &[BenchmarkTaskReport]) -> BenchmarkAggregate {
    let mut passed = 0;
    let mut failed = 0;
    let mut skipped = 0;
    let mut errors = 0;
    let mut packet_files_found = 0;
    let mut packet_files_expected = 0;
    let mut packet_symbols_found = 0;
    let mut packet_symbols_expected = 0;
    let mut seek_files_found = 0;
    let mut seek_files_expected = 0;
    let mut seek_symbols_found = 0;
    let mut seek_symbols_expected = 0;
    let mut guard_tasks = 0;
    let mut guard_tasks_passed = 0;
    let mut reductions = Vec::new();
    let mut prepare_latencies = Vec::new();

    for task in tasks {
        match task.status {
            BenchmarkTaskStatus::Passed => passed += 1,
            BenchmarkTaskStatus::Failed => failed += 1,
            BenchmarkTaskStatus::Skipped => skipped += 1,
            BenchmarkTaskStatus::Error => errors += 1,
        }
        if matches!(
            task.status,
            BenchmarkTaskStatus::Passed | BenchmarkTaskStatus::Failed
        ) {
            packet_files_found += task.retrieval.packet_relevant_files_found.len();
            packet_files_expected += task.expected_relevant_files.len();
            packet_symbols_found += task.retrieval.packet_relevant_symbols_found.len();
            packet_symbols_expected += task.expected_relevant_symbols.len();
            seek_files_found += task.retrieval.seek_relevant_files_found.len();
            seek_files_expected += task.expected_relevant_files.len();
            seek_symbols_found += task.retrieval.seek_relevant_symbols_found.len();
            seek_symbols_expected += task.expected_relevant_symbols.len();
            if let Some(reduction) = task.tokens.packet_token_reduction {
                reductions.push(reduction);
            }
            if let Some(latency) = task.latency.prepare_ms {
                prepare_latencies.push(latency);
            }
        }
        if task.guards.configured {
            guard_tasks += 1;
            if task.guards.passed
                && matches!(
                    task.status,
                    BenchmarkTaskStatus::Passed | BenchmarkTaskStatus::Failed
                )
            {
                guard_tasks_passed += 1;
            }
        }
    }

    BenchmarkAggregate {
        task_count: tasks.len(),
        passed,
        failed,
        skipped,
        errors,
        packet_relevant_file_recall: recall(packet_files_found, packet_files_expected),
        packet_relevant_symbol_recall: recall(packet_symbols_found, packet_symbols_expected),
        seek_relevant_file_recall: recall(seek_files_found, seek_files_expected),
        seek_relevant_symbol_recall: recall(seek_symbols_found, seek_symbols_expected),
        guard_tasks,
        guard_tasks_passed,
        median_packet_token_reduction: percentile(&mut reductions, 50, 100),
        p50_prepare_ms: percentile(&mut prepare_latencies.clone(), 50, 100),
        p95_prepare_ms: percentile(&mut prepare_latencies, 95, 100),
        latency_percentile_method: "nearest_rank",
    }
}

fn percentile(values: &mut [f64], numerator: usize, denominator: usize) -> Option<f64> {
    if values.is_empty() || denominator == 0 || numerator > denominator {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let rank = values
        .len()
        .saturating_mul(numerator)
        .saturating_add(denominator.saturating_sub(1))
        / denominator;
    Some(values[rank.max(1).saturating_sub(1).min(values.len() - 1)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_reduction_is_observed_ratio() {
        assert_eq!(token_reduction(1_000, 250), Some(0.75));
        assert_eq!(token_reduction(0, 250), None);
    }

    #[test]
    fn nearest_rank_percentiles_are_deterministic() {
        let values = [4.0, 1.0, 3.0, 2.0];
        assert_eq!(percentile(&mut values.clone(), 50, 100), Some(2.0));
        assert_eq!(percentile(&mut values.clone(), 95, 100), Some(4.0));
    }

    #[test]
    fn fixture_paths_reject_parent_traversal() {
        let root = Path::new("fixtures");
        assert!(safe_join_relative(root, "src/lib.rs").is_ok());
        assert!(safe_join_relative(root, "../secret.txt").is_err());
    }
}
