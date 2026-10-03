//! Shared, serializable types used by VEXA's deterministic pipeline.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const VEXA_API_VERSION: &str = "1";
pub const INDEX_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Rust,
    Python,
    TypeScript,
    JavaScript,
    Java,
    C,
    Cpp,
    CSharp,
    Go,
    Json,
    Toml,
    Yaml,
    Markdown,
    Shell,
    PowerShell,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GitFileState {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Ignored,
    Conflicted,
    #[default]
    Clean,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Interface,
    Trait,
    Module,
    Type,
    Constant,
    Variable,
    Field,
    Test,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolRecord {
    pub name: String,
    pub kind: SymbolKind,
    pub start_line: u32,
    pub end_line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportRecord {
    pub target: String,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
// Independent file classifications and parser status are orthogonal flags,
// rather than mutually exclusive states.
#[allow(clippy::struct_excessive_bools)]
pub struct FileRecord {
    /// Repository-relative path using `/` separators on every platform.
    pub path: String,
    pub language: Language,
    pub size_bytes: u64,
    pub modified_unix_ms: u64,
    pub sha256: String,
    pub git_state: GitFileState,
    pub generated: bool,
    pub vendor: bool,
    pub sensitive: bool,
    pub token_count: u32,
    #[serde(default)]
    pub term_frequency: BTreeMap<String, u32>,
    #[serde(default)]
    pub symbols: Vec<SymbolRecord>,
    #[serde(default)]
    pub parse_errors: bool,
    #[serde(default)]
    pub imports: Vec<ImportRecord>,
    #[serde(default)]
    pub related_tests: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexSnapshot {
    pub schema_version: u32,
    pub parser_version: String,
    pub repository_root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_revision: Option<String>,
    pub indexed_unix_ms: u64,
    pub files: Vec<FileRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OperatingProfile {
    Lite,
    Standard,
    Enhanced,
    #[default]
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustLevel {
    TrustedMetadata,
    VerifiedRepositoryEvidence,
    UntrustedRepositoryContent,
    UserProvided,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationState {
    Verified,
    Stale,
    Unverified,
    Contradicted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_revision: Option<String>,
    pub observed_unix_ms: u64,
    pub trust: TrustLevel,
    pub verification: VerificationState,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScoreBreakdown {
    pub lexical: f64,
    pub exact_symbol: f64,
    pub filename: f64,
    pub graph: f64,
    pub git: f64,
    pub test: f64,
    pub diagnostic: f64,
    pub recency: f64,
    pub semantic: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub path: String,
    pub score: f64,
    pub breakdown: ScoreBreakdown,
    #[serde(default)]
    pub reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best_symbol: Option<SymbolRecord>,
    /// Exact AST symbols available in this candidate file, not inferred definitions.
    #[serde(default)]
    pub symbols: Vec<SymbolRecord>,
}

/// Lexical identifiers plus camel/snake components, shared by index and query processing.
#[must_use]
pub fn lexical_tokens(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    for raw in text
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|s| !s.is_empty())
    {
        let raw: String = raw.chars().take(96).collect();
        let mut terms = vec![raw.to_lowercase()];
        let mut component = String::new();
        for ch in raw.chars() {
            if (ch == '_'
                || (ch.is_uppercase() && component.chars().last().is_some_and(char::is_lowercase)))
                && !component.is_empty()
            {
                terms.push(std::mem::take(&mut component));
            }
            if ch != '_' {
                component.extend(ch.to_lowercase());
            }
        }
        if !component.is_empty() {
            terms.push(component);
        }
        let stems: Vec<_> = terms
            .iter()
            .filter_map(|term| {
                ["ation", "ing", "ed", "es", "s"].iter().find_map(|suffix| {
                    term.strip_suffix(suffix)
                        .filter(|stem| stem.len() >= 4)
                        .map(str::to_owned)
                })
            })
            .collect();
        terms.extend(stems);
        terms.sort();
        terms.dedup();
        result.extend(terms.into_iter().filter(|s| s.len() >= 2));
    }
    result
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub score: f64,
    pub sha256: String,
    pub excerpt: String,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareRequest {
    #[serde(default = "default_api_version")]
    pub vexa_api: String,
    #[serde(default = "default_prepare_op")]
    pub op: String,
    pub cwd: PathBuf,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub mode: OperatingProfile,
    #[serde(default = "default_context_tokens")]
    pub max_context_tokens: usize,
    #[serde(default)]
    pub diagnostics: Vec<String>,
    #[serde(default)]
    pub include_paths: Vec<String>,
}

fn default_api_version() -> String {
    VEXA_API_VERSION.to_owned()
}

fn default_prepare_op() -> String {
    "prepare".to_owned()
}

const fn default_context_tokens() -> usize {
    1800
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareResponse {
    pub vexa_api: String,
    pub status: String,
    pub profile: OperatingProfile,
    pub confidence: f64,
    pub estimated_source_tokens: usize,
    pub estimated_packet_tokens: usize,
    pub packet: String,
    pub evidence: Vec<EvidenceSummary>,
    pub warnings: Vec<String>,
    pub needs_expansion: bool,
    #[serde(default)]
    pub expansion_candidates: Vec<String>,
    pub timings_ms: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceSummary {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub sha256: String,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryFact {
    pub key: String,
    pub value: serde_json::Value,
    pub source: Provenance,
    pub revision: Option<String>,
    pub created_unix_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectMemory {
    pub project_name: Option<String>,
    pub repository_root: Option<PathBuf>,
    pub revision: Option<String>,
    #[serde(default)]
    pub facts: Vec<MemoryFact>,
    #[serde(default)]
    pub unresolved: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkTask {
    pub id: String,
    pub prompt: String,
    #[serde(default)]
    pub relevant_files: Vec<String>,
    #[serde(default)]
    pub relevant_symbols: Vec<String>,
    #[serde(default = "default_context_tokens")]
    pub max_context_tokens: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkResult {
    pub tasks: usize,
    pub relevant_file_recall: f64,
    pub relevant_symbol_recall: f64,
    pub median_context_reduction: f64,
    pub p50_prepare_ms: f64,
    pub p95_prepare_ms: f64,
    pub failures: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_request_defaults_are_safe() {
        let request: PrepareRequest = serde_json::from_str(
            r#"{"cwd":".","prompt":"Fix auth without changing the public API"}"#,
        )
        .unwrap_or_else(|error| panic!("failed to parse request: {error}"));
        assert_eq!(request.vexa_api, VEXA_API_VERSION);
        assert_eq!(request.op, "prepare");
        assert_eq!(request.max_context_tokens, 1800);
        assert_eq!(request.mode, OperatingProfile::Auto);
    }
}
