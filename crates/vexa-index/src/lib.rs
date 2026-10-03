//! Deterministic, execution-free repository indexing.

use anyhow::{Context, Result, bail};
use ignore::WalkBuilder;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write as _;
use std::path::{Component, Path};
use std::time::{SystemTime, UNIX_EPOCH};
use tree_sitter::{Language as TsLanguage, Node, Parser};
use vexa_types::{
    FileRecord, GitFileState, INDEX_SCHEMA_VERSION, ImportRecord, IndexSnapshot, Language,
    SymbolKind, SymbolRecord,
};

const PARSER_VERSION: &str = "tree-sitter-0.27/vexa-index-v4";

#[derive(Debug, Clone)]
pub struct IndexOptions {
    pub max_file_bytes: u64,
    pub include_sensitive: bool,
    pub include_generated: bool,
    pub include_vendor: bool,
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            max_file_bytes: 2 * 1024 * 1024,
            include_sensitive: false,
            include_generated: false,
            include_vendor: false,
        }
    }
}

/// Build or incrementally refresh a deterministic repository snapshot without executing repository code.
///
/// # Errors
///
/// Returns an error when the repository root cannot be canonicalized/read or when required index
/// metadata cannot be produced safely.
#[allow(clippy::too_many_lines)]
pub fn build_index(
    root: &Path,
    previous: Option<&IndexSnapshot>,
    options: &IndexOptions,
) -> Result<IndexSnapshot> {
    let root = root
        .canonicalize()
        .with_context(|| format!("cannot canonicalize repository root {}", root.display()))?;
    if !root.is_dir() {
        bail!("repository root is not a directory: {}", root.display());
    }

    let (git_revision, git_states) = collect_git_state(&root);
    let previous_by_path: HashMap<&str, &FileRecord> = previous
        .filter(|snapshot| {
            snapshot.schema_version == INDEX_SCHEMA_VERSION
                && snapshot.parser_version == PARSER_VERSION
                && snapshot.repository_root == root
        })
        .map(|snapshot| {
            snapshot
                .files
                .iter()
                .map(|file| (file.path.as_str(), file))
                .collect()
        })
        .unwrap_or_default();

    let mut builder = WalkBuilder::new(&root);
    builder
        .hidden(false)
        .follow_links(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .add_custom_ignore_filename(".vexaignore");

    let root_for_filter = root.clone();
    let include_vendor = options.include_vendor;
    let include_generated = options.include_generated;
    builder.filter_entry(move |entry| {
        if entry.path() == root_for_filter {
            return true;
        }
        let Some(name) = entry.file_name().to_str() else {
            return false;
        };
        !matches!(
            name.to_ascii_lowercase().as_str(),
            ".git" | ".hg" | ".svn" | ".venv" | "venv" | "__pycache__"
        ) && (include_vendor
            || !matches!(
                name.to_ascii_lowercase().as_str(),
                "node_modules" | "vendor" | "vendors"
            ))
            && (include_generated
                || !matches!(
                    name.to_ascii_lowercase().as_str(),
                    "target" | "dist" | "build"
                ))
    });

    let mut files = Vec::new();
    for entry in builder.build() {
        let Ok(entry) = entry else { continue };
        let Ok(file_type) = entry
            .file_type()
            .ok_or_else(|| anyhow::anyhow!("missing file type"))
        else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }

        let path = entry.path();
        let Ok(canonical) = path.canonicalize() else {
            continue;
        };
        if !canonical.starts_with(&root) {
            continue;
        }
        let Ok(relative) = canonical.strip_prefix(&root) else {
            continue;
        };
        let relative = normalize_relative(relative);
        if relative.is_empty() {
            continue;
        }

        let sensitive = is_sensitive_path(&relative);
        if sensitive && !options.include_sensitive {
            continue;
        }
        let generated = is_generated_path(&relative);
        if generated && !options.include_generated {
            continue;
        }
        let vendor = is_vendor_path(&relative);
        if vendor && !options.include_vendor {
            continue;
        }

        let Ok(metadata) = fs::metadata(&canonical) else {
            continue;
        };
        if metadata.len() > options.max_file_bytes {
            continue;
        }
        let modified_unix_ms = modified_ms(&metadata);
        let git_state = git_states
            .get(&relative)
            .copied()
            .unwrap_or(GitFileState::Clean);

        let Ok(bytes) = fs::read(&canonical) else {
            continue;
        };
        if bytes.len() as u64 > options.max_file_bytes || looks_binary(&bytes) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let sensitive = sensitive || contains_secret(text);
        if sensitive && !options.include_sensitive {
            continue;
        }
        let sha256 = sha256_bytes(&bytes);

        if let Some(old) = previous_by_path.get(relative.as_str())
            && old.sha256 == sha256
        {
            let mut reused = (*old).clone();
            reused.git_state = git_state;
            reused.modified_unix_ms = modified_unix_ms;
            files.push(reused);
            continue;
        }

        let language = detect_language(&canonical);
        let (symbols, imports, parse_errors) = parse_structure(language, text.as_bytes());
        let (term_frequency, token_count) = lexical_terms(text);

        files.push(FileRecord {
            path: relative,
            language,
            size_bytes: metadata.len(),
            modified_unix_ms,
            sha256,
            git_state,
            generated,
            vendor,
            sensitive,
            token_count,
            term_frequency,
            symbols,
            parse_errors,
            imports,
            related_tests: Vec::new(),
        });
    }

    files.sort_by(|left, right| left.path.cmp(&right.path));
    link_related_tests(&mut files);

    Ok(IndexSnapshot {
        schema_version: INDEX_SCHEMA_VERSION,
        parser_version: PARSER_VERSION.to_owned(),
        repository_root: root,
        git_revision,
        indexed_unix_ms: now_ms(),
        files,
    })
}

/// Load a previously persisted index snapshot.
///
/// # Errors
///
/// Returns an error if the file cannot be read, parsed, or uses an unsupported schema version.
pub fn load_snapshot(path: &Path) -> Result<IndexSnapshot> {
    let bytes = fs::read(path).with_context(|| format!("cannot read index {}", path.display()))?;
    let snapshot: IndexSnapshot = serde_json::from_slice(&bytes)
        .with_context(|| format!("invalid index JSON {}", path.display()))?;
    if snapshot.schema_version != INDEX_SCHEMA_VERSION {
        bail!(
            "unsupported index schema {} (expected {})",
            snapshot.schema_version,
            INDEX_SCHEMA_VERSION
        );
    }
    Ok(snapshot)
}

/// Persist an index snapshot using a temporary file and recoverable rotation.
///
/// # Errors
///
/// Returns an error if serialization, directory creation, writing, or activation fails.
pub fn save_snapshot(path: &Path, snapshot: &IndexSnapshot) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create index directory {}", parent.display()))?;
    }
    let data = serde_json::to_vec(snapshot).context("cannot serialize index")?;
    let parent = path.parent().context("index path has no parent")?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("lock"))?;
    fs2::FileExt::lock_exclusive(&lock)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(&data)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

/// Compute the current SHA-256 hash of a file.
///
/// # Errors
///
/// Returns an error when the file cannot be read.
pub fn hash_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    Ok(sha256_bytes(&bytes))
}

fn collect_git_state(root: &Path) -> (Option<String>, HashMap<String, GitFileState>) {
    let Ok(repo) = gix::discover(root) else {
        return (None, HashMap::new());
    };
    // An explicit index root must never enumerate a different ancestor repository.
    if repo
        .workdir()
        .and_then(|path| path.canonicalize().ok())
        .as_deref()
        != Some(root)
    {
        return (None, HashMap::new());
    }
    let revision = repo.head_id().ok().map(|id| id.to_string());
    let mut states = HashMap::new();
    let Ok(platform) = repo.status(gix::progress::Discard) else {
        return (revision, states);
    };
    let Ok(iter) = platform.into_iter(Vec::<gix::bstr::BString>::new()) else {
        return (revision, states);
    };
    for item in iter.flatten() {
        let path = item.location().to_string();
        let debug = format!("{item:?}");
        let state = if debug.contains("Conflict") {
            GitFileState::Conflicted
        } else if debug.contains("Rewrite") || debug.contains("Renamed") {
            GitFileState::Renamed
        } else if debug.contains("DirectoryContents") || debug.contains("Untracked") {
            GitFileState::Untracked
        } else if debug.contains("Removed") {
            GitFileState::Deleted
        } else if debug.contains("Addition")
            || debug.contains("Added")
            || debug.contains("IntentToAdd")
        {
            GitFileState::Added
        } else {
            GitFileState::Modified
        };
        states.insert(path.replace('\\', "/"), state);
    }
    (revision, states)
}

fn normalize_relative(path: &Path) -> String {
    let mut parts = Vec::new();
    for component in path.components() {
        if let Component::Normal(part) = component {
            parts.push(part.to_string_lossy().into_owned());
        }
    }
    parts.join("/")
}

fn detect_language(path: &Path) -> Language {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "rs" => Language::Rust,
        "py" | "pyi" => Language::Python,
        "ts" | "tsx" => Language::TypeScript,
        "js" | "jsx" | "mjs" | "cjs" => Language::JavaScript,
        "java" => Language::Java,
        "c" | "h" => Language::C,
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => Language::Cpp,
        "cs" => Language::CSharp,
        "go" => Language::Go,
        "json" | "jsonc" => Language::Json,
        "toml" => Language::Toml,
        "yml" | "yaml" => Language::Yaml,
        "md" | "mdx" => Language::Markdown,
        "sh" | "bash" | "zsh" | "fish" => Language::Shell,
        "ps1" | "psm1" | "psd1" => Language::PowerShell,
        _ if matches!(file_name.as_str(), "makefile" | "dockerfile") => Language::Unknown,
        _ => Language::Unknown,
    }
}

fn ts_language(language: Language) -> Option<TsLanguage> {
    Some(match language {
        Language::Rust => tree_sitter_rust::LANGUAGE.into(),
        Language::Python => tree_sitter_python::LANGUAGE.into(),
        Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Language::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
        Language::Java => tree_sitter_java::LANGUAGE.into(),
        Language::C => tree_sitter_c::LANGUAGE.into(),
        Language::Cpp => tree_sitter_cpp::LANGUAGE.into(),
        Language::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
        Language::Go => tree_sitter_go::LANGUAGE.into(),
        _ => return None,
    })
}

fn parse_structure(
    language: Language,
    source: &[u8],
) -> (Vec<SymbolRecord>, Vec<ImportRecord>, bool) {
    let Some(ts_language) = ts_language(language) else {
        return (Vec::new(), Vec::new(), true);
    };
    let mut parser = Parser::new();
    if parser.set_language(&ts_language).is_err() {
        return (Vec::new(), Vec::new(), true);
    }
    let Some(tree) = parser.parse(source, None) else {
        return (Vec::new(), Vec::new(), true);
    };
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut cursor = tree.walk();
    loop {
        collect_node(cursor.node(), source, &mut symbols, &mut imports);
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return (symbols, imports, tree.root_node().has_error());
            }
        }
    }
}

fn collect_node(
    node: Node<'_>,
    source: &[u8],
    symbols: &mut Vec<SymbolRecord>,
    imports: &mut Vec<ImportRecord>,
) {
    if let Some(kind) = symbol_kind(node.kind())
        && let Some(name_node) = declaration_name(node)
        && let Ok(name) = name_node.utf8_text(source)
    {
        let name = name.trim();
        if !name.is_empty() {
            let signature = signature_text(node, source);
            symbols.push(SymbolRecord {
                name: name.to_owned(),
                kind: if is_test_symbol(name, node.kind()) {
                    SymbolKind::Test
                } else {
                    kind
                },
                start_line: u32::try_from(node.start_position().row + 1).unwrap_or(u32::MAX),
                end_line: u32::try_from(node.end_position().row + 1).unwrap_or(u32::MAX),
                signature,
            });
        }
    }

    if is_import_node(node.kind()) {
        let target_node = ["source", "path", "argument", "module_name", "name"]
            .iter()
            .find_map(|field| node.child_by_field_name(field))
            .unwrap_or(node);
        let text = target_node
            .utf8_text(source)
            .unwrap_or_default()
            .trim()
            .trim_matches(['"', '\'', '<', '>'])
            .replace(['\r', '\n'], " ");
        if !text.is_empty() {
            imports.push(ImportRecord {
                target: truncate_chars(&text, 320),
                start_line: u32::try_from(node.start_position().row + 1).unwrap_or(u32::MAX),
                end_line: u32::try_from(node.end_position().row + 1).unwrap_or(u32::MAX),
            });
        }
    }
}

fn symbol_kind(kind: &str) -> Option<SymbolKind> {
    Some(match kind {
        "function_item"
        | "function_definition"
        | "function_declaration"
        | "function_declarator"
        | "function_expression" => SymbolKind::Function,
        "method_definition" | "method_declaration" | "constructor_declaration" => {
            SymbolKind::Method
        }
        "class_definition" | "class_declaration" => SymbolKind::Class,
        "struct_item" | "struct_specifier" | "struct_declaration" => SymbolKind::Struct,
        "enum_item" | "enum_specifier" | "enum_declaration" => SymbolKind::Enum,
        "interface_declaration" => SymbolKind::Interface,
        "trait_item" => SymbolKind::Trait,
        "mod_item" | "module" | "module_declaration" => SymbolKind::Module,
        "type_item" | "type_alias_declaration" => SymbolKind::Type,
        "const_item" | "constant_declaration" => SymbolKind::Constant,
        _ => return None,
    })
}

fn declaration_name(mut node: Node<'_>) -> Option<Node<'_>> {
    for _ in 0..64 {
        if matches!(
            node.kind(),
            "identifier" | "field_identifier" | "qualified_identifier" | "type_identifier"
        ) {
            return Some(node);
        }
        if let Some(name) = node.child_by_field_name("name") {
            return Some(name);
        }
        node = node.child_by_field_name("declarator")?;
    }
    None
}

fn is_import_node(kind: &str) -> bool {
    matches!(
        kind,
        "use_declaration"
            | "import_statement"
            | "import_from_statement"
            | "import_declaration"
            | "using_directive"
            | "preproc_include"
            | "package_clause"
            | "import_spec"
            | "import_spec_list"
    )
}

fn signature_text(node: Node<'_>, source: &[u8]) -> Option<String> {
    let end = node
        .child_by_field_name("body")
        .map_or(node.end_byte(), |body| body.start_byte());
    let end = end.min(node.start_byte().saturating_add(640));
    let text = source.get(node.start_byte()..end)?;
    let text = String::from_utf8_lossy(text);
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!normalized.is_empty()).then(|| truncate_chars(&normalized, 320))
}

fn is_test_symbol(name: &str, kind: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("test_")
        || lower.ends_with("_test")
        || lower.starts_with("test") && matches!(kind, "method_declaration" | "method_definition")
}

fn lexical_terms(text: &str) -> (BTreeMap<String, u32>, u32) {
    let mut terms = BTreeMap::new();
    let mut count = 0_u32;
    for token in vexa_types::lexical_tokens(text) {
        *terms.entry(token).or_insert(0) += 1;
        count = count.saturating_add(1);
    }
    (terms, count)
}

fn link_related_tests(files: &mut [FileRecord]) {
    let test_paths: Vec<(String, String)> = files
        .iter()
        .filter(|file| is_test_path(&file.path))
        .map(|file| (file.path.clone(), stem(&file.path)))
        .collect();
    for file in files.iter_mut().filter(|file| !is_test_path(&file.path)) {
        let own_stem = stem(&file.path);
        if own_stem.len() < 2 {
            continue;
        }
        file.related_tests = test_paths
            .iter()
            .filter(|(_, test_stem)| test_stem.contains(&own_stem) || own_stem.contains(test_stem))
            .map(|(path, _)| path.clone())
            .take(16)
            .collect();
    }
}

fn is_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/test/")
        || lower.contains("/tests/")
        || lower.contains("/__tests__/")
        || lower.ends_with("_test.rs")
        || lower.ends_with("_test.go")
        || lower.contains(".test.")
        || lower.contains(".spec.")
        || lower.starts_with("test_")
}

fn stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .replace("test", "")
        .replace("spec", "")
        .replace(['_', '-', '.'], "")
}

fn is_sensitive_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let lower_path = Path::new(&lower);
    let name = lower_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let sensitive_extension = lower_path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|ext| matches!(ext, "pem" | "key" | "p12" | "pfx"));
    matches!(
        name,
        ".env"
            | ".env.local"
            | ".env.production"
            | ".env.development"
            | "credentials"
            | "credentials.json"
            | "secrets.json"
            | "id_rsa"
            | "id_ed25519"
            | ".npmrc"
            | ".pypirc"
            | "netrc"
            | ".netrc"
    ) || name.starts_with(".env.")
        || sensitive_extension
        || format!("/{lower}").contains("/.ssh/")
        || format!("/{lower}").contains("/.aws/")
        || format!("/{lower}").contains("/.config/gcloud/")
}

/// Conservative local secret guard; intentionally excludes entire suspect files.
/// Detection is heuristic and cannot guarantee that all secrets are identified.
#[must_use]
pub fn contains_secret(text: &str) -> bool {
    if text.contains("-----BEGIN ") && text.contains("PRIVATE KEY-----") {
        return true;
    }
    for token in text.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))) {
        if [
            "ghp_",
            "gho_",
            "ghu_",
            "ghs_",
            "github_pat_",
            "sk_live_",
            "sk-proj-",
        ]
        .iter()
        .any(|prefix| token.starts_with(prefix) && token.len() >= prefix.len() + 20)
            || (token.starts_with("AKIA") && token.len() == 20)
        {
            return true;
        }
    }
    text.lines().any(|line| {
        let Some((key, value)) = line.split_once('=').or_else(|| line.split_once(':')) else {
            return false;
        };
        let key = key
            .trim()
            .trim_matches(['"', '\'', ' '])
            .to_ascii_lowercase();
        let value = value.trim().trim_matches(['"', '\'', ',', ';', ' ']);
        matches!(
            key.as_str(),
            "password"
                | "passwd"
                | "api_key"
                | "apikey"
                | "access_token"
                | "client_secret"
                | "secret_key"
        ) && value.len() >= 8
            && !value.contains([' ', '$', '{', '('])
            && !matches!(
                value.to_ascii_lowercase().as_str(),
                "changeme" | "placeholder" | "example" | "your_api_key"
            )
    })
}

fn is_generated_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let extension = Path::new(&lower)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    lower.contains("/generated/")
        || lower.contains("/gen/")
        || lower.ends_with(".min.js")
        || matches!(extension, "map" | "lock" | "log")
        || lower.ends_with(".pb.go")
        || lower.ends_with(".designer.cs")
}

fn is_vendor_path(path: &str) -> bool {
    let lower = format!("/{}/", path.to_ascii_lowercase());
    [
        "/vendor/",
        "/vendors/",
        "/node_modules/",
        "/third_party/",
        "/third-party/",
        "/external/",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|byte| *byte == 0)
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn modified_ms(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or_default()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or_default()
}

fn truncate_chars(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_and_cpp_declarators_include_prototypes_and_qualified_methods() {
        let (symbols, _, _) = parse_structure(Language::C, b"int parse_frame_header(const char *p);\nint parse_frame_header(const char *p) { return 0; }");
        assert_eq!(
            symbols
                .iter()
                .filter(|s| s.name == "parse_frame_header")
                .map(|s| s.start_line)
                .collect::<std::collections::BTreeSet<_>>(),
            [1, 2].into_iter().collect()
        );
        let (symbols, _, _) = parse_structure(Language::Cpp, b"class IndexWindow { bool contains(int p) const; };\nbool IndexWindow::contains(int p) const { return true; }");
        assert!(symbols.iter().any(|s| s.name == "contains"));
        assert!(symbols.iter().any(|s| s.name == "IndexWindow::contains"));
    }

    #[test]
    fn imports_store_the_actual_relative_module_path() {
        let (_, imports, _) =
            parse_structure(Language::TypeScript, b"import { helper } from './helper';");
        assert_eq!(imports[0].target, "./helper");
    }

    #[test]
    fn secrets_in_source_never_enter_lexical_cache() -> Result<()> {
        let dir = tempfile::tempdir()?;
        fs::write(dir.path().join("safe.rs"), "fn safe() {}")?;
        let synthetic = format!("const TOKEN: &str = \"{}{}\";", "ghp_", "a".repeat(36));
        fs::write(dir.path().join("config.rs"), synthetic)?;
        fs::write(dir.path().join(".env.staging"), "PASSWORD=testfixture")?;
        fs::create_dir(dir.path().join(".ssh"))?;
        fs::write(dir.path().join(".ssh/config"), "Host private")?;
        let snapshot = build_index(dir.path(), None, &IndexOptions::default())?;
        assert_eq!(snapshot.files.len(), 1);
        assert_eq!(snapshot.files[0].path, "safe.rs");
        Ok(())
    }

    #[test]
    fn content_hash_overrides_matching_size_and_timestamp() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("lib.rs");
        fs::write(&path, "fn before() {}")?;
        let snapshot = build_index(dir.path(), None, &IndexOptions::default())?;
        let modified = fs::metadata(&path)?.modified()?;
        fs::write(&path, "fn after_() {}")?;
        fs::File::options()
            .write(true)
            .open(&path)?
            .set_modified(modified)?;
        let refreshed = build_index(dir.path(), Some(&snapshot), &IndexOptions::default())?;
        assert_ne!(refreshed.files[0].sha256, snapshot.files[0].sha256);
        assert_eq!(refreshed.files[0].symbols[0].name, "after_");
        Ok(())
    }

    #[test]
    fn concurrent_cache_writers_leave_complete_json() -> Result<()> {
        let dir = tempfile::tempdir()?;
        fs::write(dir.path().join("lib.rs"), "fn stable() {}")?;
        let snapshot = build_index(dir.path(), None, &IndexOptions::default())?;
        let path = dir.path().join("cache/index.json");
        std::thread::scope(|scope| -> Result<()> {
            let handles: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| save_snapshot(&path, &snapshot)))
                .collect();
            for handle in handles {
                handle
                    .join()
                    .map_err(|_| anyhow::anyhow!("writer panicked"))??;
            }
            Ok(())
        })?;
        assert_eq!(load_snapshot(&path)?.files.len(), 1);
        Ok(())
    }

    #[test]
    fn vendor_override_is_honored() -> Result<()> {
        let dir = tempfile::tempdir()?;
        fs::create_dir(dir.path().join("vendor"))?;
        fs::write(dir.path().join("vendor/lib.rs"), "fn vendored() {}")?;
        assert!(
            build_index(dir.path(), None, &IndexOptions::default())?
                .files
                .is_empty()
        );
        let options = IndexOptions {
            include_vendor: true,
            ..IndexOptions::default()
        };
        assert_eq!(build_index(dir.path(), None, &options)?.files.len(), 1);
        Ok(())
    }
    use std::io::Write;

    #[test]
    fn indexes_symbols_and_skips_secrets() -> Result<()> {
        let dir = tempfile::tempdir()?;
        fs::create_dir_all(dir.path().join("src"))?;
        fs::write(
            dir.path().join("src/lib.rs"),
            "use std::path::Path;\npub fn player_manager() -> u32 { 7 }\n",
        )?;
        fs::write(dir.path().join(".env"), "SUPER_SECRET=value\n")?;

        let snapshot = build_index(dir.path(), None, &IndexOptions::default())?;
        assert_eq!(snapshot.files.len(), 1);
        assert_eq!(snapshot.files[0].path, "src/lib.rs");
        assert!(
            snapshot.files[0]
                .symbols
                .iter()
                .any(|symbol| symbol.name == "player_manager")
        );
        assert!(
            snapshot.files[0]
                .imports
                .iter()
                .any(|item| item.target.contains("std::path"))
        );
        Ok(())
    }

    #[test]
    fn vexaignore_is_respected() -> Result<()> {
        let dir = tempfile::tempdir()?;
        fs::write(dir.path().join("keep.py"), "def keep():\n    return 1\n")?;
        fs::write(dir.path().join("drop.py"), "def drop():\n    return 2\n")?;
        let mut ignore = fs::File::create(dir.path().join(".vexaignore"))?;
        writeln!(ignore, "drop.py")?;
        let snapshot = build_index(dir.path(), None, &IndexOptions::default())?;
        assert!(snapshot.files.iter().any(|file| file.path == "keep.py"));
        assert!(!snapshot.files.iter().any(|file| file.path == "drop.py"));
        Ok(())
    }

    #[test]
    fn source_hash_changes_after_edit() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("main.go");
        fs::write(&path, "package main\nfunc First() {}\n")?;
        let first = build_index(dir.path(), None, &IndexOptions::default())?;
        std::thread::sleep(std::time::Duration::from_millis(5));
        fs::write(&path, "package main\nfunc Second() {}\n")?;
        let second = build_index(dir.path(), Some(&first), &IndexOptions::default())?;
        assert_ne!(first.files[0].sha256, second.files[0].sha256);
        assert!(
            second.files[0]
                .symbols
                .iter()
                .any(|symbol| symbol.name == "Second")
        );
        Ok(())
    }
}
