mod benchmark;
mod config;
mod hooks;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use vexa_core::{
    CoreConfig, cached_index_path, detect_profile, prepare, prepare_additive_context, refresh_index,
};
use vexa_index::load_snapshot;
use vexa_memory::{clear as clear_memory, inspect as inspect_memory, load as load_memory};
use vexa_types::{OperatingProfile, PrepareRequest, VEXA_API_VERSION};
use vexa_update::{PlatformTarget, Updater};
use vexa_verify::{Claim, reduce_log, verify_claim};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser, Debug)]
#[command(name = "vexa", version = VERSION, about = "Verified Efficiency eXecution Accelerator")]
struct Cli {
    #[arg(long, global = true, help = "Emit extra local diagnostics to stderr")]
    verbose: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Prepare(PrepareArgs),
    InspectPacket(PrepareArgs),
    Index(IndexArgs),
    Inspect(RepoArgs),
    Verify(VerifyArgs),
    Memory(MemoryArgs),
    Doctor(OutputArgs),
    Status(StatusArgs),
    Config(ConfigArgs),
    Benchmark(BenchmarkArgs),
    Update(UpdateArgs),
    /// Internal installer entry point, after archive checksum and structure validation.
    #[command(hide = true)]
    InstallActivate {
        #[arg(long)]
        version: String,
    },
    Rollback(OutputArgs),
    Uninstall(UninstallArgs),
    Version(OutputArgs),
    Hook(HookArgs),
    Protocol(ProtocolArgs),
}

#[derive(Args, Debug, Clone)]
struct PrepareArgs {
    #[arg(long, default_value = ".")]
    cwd: PathBuf,
    #[arg(long, conflicts_with = "prompt_file")]
    prompt: Option<String>,
    #[arg(long)]
    prompt_file: Option<PathBuf>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long, value_enum, default_value = "auto")]
    mode: ProfileArg,
    #[arg(long)]
    max_context_tokens: Option<usize>,
    #[arg(long = "diagnostic")]
    diagnostics: Vec<String>,
    /// Add an indexed, non-sensitive relative path to the evidence candidates.
    #[arg(long = "include-path")]
    include_paths: Vec<String>,
    #[arg(long)]
    json: bool,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
enum ProfileArg {
    Auto,
    Lite,
    Standard,
    Enhanced,
}

impl From<ProfileArg> for OperatingProfile {
    fn from(value: ProfileArg) -> Self {
        match value {
            ProfileArg::Auto => Self::Auto,
            ProfileArg::Lite => Self::Lite,
            ProfileArg::Standard => Self::Standard,
            ProfileArg::Enhanced => Self::Enhanced,
        }
    }
}

#[derive(Args, Debug, Clone)]
struct RepoArgs {
    #[arg(long, default_value = ".")]
    cwd: PathBuf,
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
struct IndexArgs {
    #[command(subcommand)]
    command: IndexCommand,
}

#[derive(Subcommand, Debug)]
enum IndexCommand {
    Refresh(RepoArgs),
    Rebuild(RepoArgs),
    Status(RepoArgs),
}

#[derive(Args, Debug)]
struct VerifyArgs {
    #[arg(long, default_value = ".", global = true)]
    cwd: PathBuf,
    #[command(subcommand)]
    command: VerifyCommand,
}

#[derive(Subcommand, Debug)]
enum VerifyCommand {
    /// Verify a JSON array of machine-checkable claims and emit a small repair packet.
    Claims {
        #[arg(long)]
        file: Option<PathBuf>,
    },
    File {
        path: String,
    },
    Hash {
        path: String,
        sha256: String,
    },
    Symbol {
        path: String,
        symbol: String,
    },
    Signature {
        path: String,
        symbol: String,
        signature: String,
    },
    Log {
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long)]
        exit_code: Option<i32>,
    },
}

#[derive(Args, Debug)]
struct MemoryArgs {
    #[arg(long, default_value = ".", global = true)]
    cwd: PathBuf,
    #[command(subcommand)]
    command: MemoryCommand,
}

#[derive(Subcommand, Debug)]
enum MemoryCommand {
    Inspect,
    Clear,
    /// Store a user-supplied JSON fact linked to the current hash of a source file.
    Set {
        key: String,
        value: String,
        #[arg(long)]
        source: String,
    },
}

#[derive(Args, Debug, Clone)]
struct OutputArgs {
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
struct StatusArgs {
    #[arg(long, default_value = ".")]
    cwd: PathBuf,
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
struct ConfigArgs {
    #[command(subcommand)]
    command: ConfigCommand,
}

#[derive(Subcommand, Debug)]
enum ConfigCommand {
    Show,
    Path,
    Set { key: String, value: String },
    Reset,
}

#[derive(Args, Debug)]
struct BenchmarkArgs {
    /// Per-task budget as a fraction of indexed source tokens (0 < value <= 1).
    #[arg(long)]
    source_fraction: Option<f64>,
    /// Evidence budget used by every fixture, for compression/recall curves.
    #[arg(long, default_value_t = 1800)]
    max_context_tokens: usize,
    #[arg(long, default_value = "benchmarks/benchmark-suite.v1.json")]
    suite: PathBuf,
    #[arg(long)]
    output: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
struct UpdateArgs {
    #[arg(long)]
    check: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
#[allow(clippy::struct_excessive_bools)]
struct UninstallArgs {
    #[arg(long)]
    cache: bool,
    #[arg(long)]
    indexes: bool,
    #[arg(long)]
    config: bool,
    #[arg(long)]
    all: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
struct HookArgs {
    #[command(subcommand)]
    command: HookCommand,
}

#[derive(Subcommand, Debug)]
enum HookCommand {
    #[command(name = "codex-user-prompt-submit")]
    UserPromptSubmit,
    /// Merge VEXA into hooks.json; trust review in Codex is still required.
    #[command(name = "codex-register")]
    Register,
    /// Remove only VEXA's registered handler.
    #[command(name = "codex-unregister")]
    Unregister,
}

#[derive(Args, Debug)]
struct ProtocolArgs {
    #[arg(long, default_value = "jsonl")]
    format: String,
}

#[derive(Debug, Deserialize)]
struct CodexPromptHookInput {
    cwd: PathBuf,
    prompt: String,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HookSpecificOutput {
    hook_event_name: &'static str,
    additional_context: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CodexPromptHookOutput {
    hook_specific_output: HookSpecificOutput,
}

fn main() {
    let cli = Cli::parse();
    if let Err(error) = run(cli) {
        eprintln!("vexa: {error:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Prepare(args) => run_prepare(&args, false, cli.verbose),
        Command::InspectPacket(args) => run_prepare(&args, true, cli.verbose),
        Command::Index(args) => run_index(args, cli.verbose),
        Command::Inspect(args) => run_inspect(&args),
        Command::Verify(args) => run_verify(args),
        Command::Memory(args) => run_memory(&args),
        Command::Doctor(args) => run_doctor(&args),
        Command::Status(args) => run_status(&args),
        Command::Config(args) => run_config(args),
        Command::Benchmark(args) => run_benchmark(&args),
        Command::Update(args) => run_update(&args),
        Command::InstallActivate { version } => {
            let state =
                Updater::new(install_root()?.join("updates")).activate_installed(&version)?;
            print_json(&state)
        }
        Command::Rollback(args) => run_rollback(&args),
        Command::Uninstall(args) => run_uninstall(&args),
        Command::Version(args) => run_version(&args),
        Command::Hook(args) => run_hook(&args, cli.verbose),
        Command::Protocol(args) => run_protocol(&args, cli.verbose),
    }
}

fn run_prepare(args: &PrepareArgs, packet_only: bool, verbose: bool) -> Result<()> {
    let config = config::load()?;
    let request = request_from_args(args, &config)?;
    let core = CoreConfig::detect()?;
    let response = prepare(&request, &core)?;
    if verbose {
        eprintln!(
            "vexa: profile={:?} confidence={:.3} evidence={} total_ms={:.2}",
            response.profile,
            response.confidence,
            response.evidence.len(),
            response
                .timings_ms
                .get("total")
                .copied()
                .unwrap_or_default()
        );
    }
    if args.json && !packet_only {
        print_json(&response)
    } else {
        println!("{}", response.packet);
        Ok(())
    }
}

fn request_from_args(args: &PrepareArgs, config: &config::VexaConfig) -> Result<PrepareRequest> {
    let prompt = if let Some(prompt) = &args.prompt {
        prompt.clone()
    } else if let Some(path) = &args.prompt_file {
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?
    } else {
        read_stdin_to_string()?
    };
    if prompt.trim().is_empty() {
        bail!("prompt must not be empty");
    }
    Ok(PrepareRequest {
        vexa_api: VEXA_API_VERSION.to_owned(),
        op: "prepare".to_owned(),
        cwd: args.cwd.clone(),
        prompt,
        model: args.model.clone(),
        mode: match args.mode {
            ProfileArg::Auto => config.profile,
            explicit => explicit.into(),
        },
        max_context_tokens: args.max_context_tokens.unwrap_or(config.max_context_tokens),
        diagnostics: args.diagnostics.clone(),
        include_paths: args.include_paths.clone(),
    })
}

fn run_index(args: IndexArgs, verbose: bool) -> Result<()> {
    let core = CoreConfig::detect()?;
    match args.command {
        IndexCommand::Refresh(repo) => {
            let snapshot = refresh_index(&repo.cwd, &core, false)?;
            emit_index_summary(&snapshot, repo.json, verbose)
        }
        IndexCommand::Rebuild(repo) => {
            let snapshot = refresh_index(&repo.cwd, &core, true)?;
            emit_index_summary(&snapshot, repo.json, verbose)
        }
        IndexCommand::Status(repo) => {
            let path = cached_index_path(&repo.cwd, &core)?;
            let snapshot = load_snapshot(&path)
                .with_context(|| format!("no readable cached index at {}", path.display()))?;
            emit_index_summary(&snapshot, repo.json, verbose)
        }
    }
}

fn emit_index_summary(
    snapshot: &vexa_types::IndexSnapshot,
    json_output: bool,
    verbose: bool,
) -> Result<()> {
    let symbol_count: usize = snapshot.files.iter().map(|file| file.symbols.len()).sum();
    let summary = json!({
        "repository_root": snapshot.repository_root,
        "git_revision": snapshot.git_revision,
        "files": snapshot.files.len(),
        "symbols": symbol_count,
        "indexed_unix_ms": snapshot.indexed_unix_ms,
        "schema_version": snapshot.schema_version,
        "parser_version": snapshot.parser_version,
    });
    if json_output {
        print_json(&summary)
    } else {
        println!("Repository: {}", snapshot.repository_root.display());
        println!("Files: {}", snapshot.files.len());
        println!("Symbols: {symbol_count}");
        println!(
            "Revision: {}",
            snapshot.git_revision.as_deref().unwrap_or("(none)")
        );
        if verbose {
            println!("Parser: {}", snapshot.parser_version);
        }
        Ok(())
    }
}

fn run_inspect(args: &RepoArgs) -> Result<()> {
    let core = CoreConfig::detect()?;
    let snapshot = refresh_index(&args.cwd, &core, false)?;
    let sensitive = snapshot.files.iter().filter(|file| file.sensitive).count();
    let generated = snapshot.files.iter().filter(|file| file.generated).count();
    let vendor = snapshot.files.iter().filter(|file| file.vendor).count();
    let output = json!({
        "repository_root": snapshot.repository_root,
        "files_indexed": snapshot.files.len(),
        "sensitive_files_included": sensitive,
        "generated_files_included": generated,
        "vendor_files_included": vendor,
        "git_revision": snapshot.git_revision,
        "index_schema": snapshot.schema_version,
        "note": "Repository content is indexed as untrusted evidence; default sensitive/generated/vendor exclusions apply before this count."
    });
    if args.json {
        print_json(&output)
    } else {
        println!("{}", serde_json::to_string_pretty(&output)?);
        Ok(())
    }
}

fn run_verify(args: VerifyArgs) -> Result<()> {
    if let VerifyCommand::Log { file, exit_code } = args.command {
        let raw = match file {
            Some(path) => fs::read_to_string(&path)
                .with_context(|| format!("cannot read log {}", path.display()))?,
            None => read_stdin_to_string()?,
        };
        return print_json(&reduce_log(&raw, exit_code));
    }

    let core = CoreConfig::detect()?;
    let snapshot = refresh_index(&args.cwd, &core, false)?;
    let root = &snapshot.repository_root;
    if let VerifyCommand::Claims { file } = args.command {
        let raw = match file {
            Some(path) => fs::read_to_string(path)?,
            None => read_stdin_to_string()?,
        };
        let claims: Vec<Claim> =
            serde_json::from_str(&raw).context("claims input must be a JSON array")?;
        let results: Vec<_> = claims
            .iter()
            .map(|claim| verify_claim(root, Some(&snapshot), claim))
            .collect();
        let corrections: Vec<_> = results
            .iter()
            .filter(|r| r.state != vexa_verify::ClaimState::Verified)
            .collect();
        print_json(
            &json!({"claims": results, "all_verified": corrections.is_empty(),
            "correction_packet": {"instruction": "Repair claims using deterministic evidence. Unknown is not proof of success.", "contradictions_or_unknowns": corrections}}),
        )?;
        if !corrections.is_empty() {
            bail!("one or more claims were not verified");
        }
        return Ok(());
    }
    let claim = match args.command {
        VerifyCommand::File { path } => Claim::FileExists { path },
        VerifyCommand::Hash { path, sha256 } => Claim::FileHash {
            path,
            expected_sha256: sha256,
        },
        VerifyCommand::Symbol { path, symbol } => Claim::SymbolExists { path, symbol },
        VerifyCommand::Signature {
            path,
            symbol,
            signature,
        } => Claim::SignatureExists {
            path,
            symbol,
            signature,
        },
        VerifyCommand::Log { .. } => unreachable!("log handled above"),
        VerifyCommand::Claims { .. } => unreachable!("claims handled above"),
    };
    let result = verify_claim(root, Some(&snapshot), &claim);
    print_json(&result)?;
    if result.state != vexa_verify::ClaimState::Verified {
        bail!("claim was not verified: {}", result.reason);
    }
    Ok(())
}

fn run_memory(args: &MemoryArgs) -> Result<()> {
    let core = CoreConfig::detect()?;
    let index_path = cached_index_path(&args.cwd, &core)?;
    let memory_path = index_path
        .parent()
        .context("cached index path has no parent")?
        .join("memory.json");
    match &args.command {
        MemoryCommand::Inspect => {
            let mut memory = load_memory(&memory_path)?;
            let snapshot = refresh_index(&args.cwd, &core, false)?;
            let hashes = snapshot
                .files
                .iter()
                .map(|file| (file.path.clone(), file.sha256.clone()))
                .collect();
            let invalidation =
                vexa_memory::invalidate(&mut memory, snapshot.git_revision.as_deref(), &hashes);
            vexa_memory::save(&memory_path, &memory)?;
            print_json(
                &json!({"summary": inspect_memory(&memory), "memory": memory, "invalidation": invalidation}),
            )
        }
        MemoryCommand::Clear => {
            let removed = clear_memory(&memory_path)?;
            print_json(&json!({"cleared": removed, "path": memory_path}))
        }
        MemoryCommand::Set { key, value, source } => {
            let snapshot = refresh_index(&args.cwd, &core, false)?;
            let file = snapshot
                .files
                .iter()
                .find(|file| file.path == *source)
                .context(
                    "memory source must be an indexed, non-sensitive repository-relative file",
                )?;
            let value: Value = serde_json::from_str(value).context("memory value must be JSON")?;
            if vexa_index::contains_secret(&value.to_string()) {
                bail!("suspected secret rejected from project memory");
            }
            let mut memory = load_memory(&memory_path)?;
            memory.repository_root = Some(snapshot.repository_root.clone());
            vexa_memory::add(
                &mut memory,
                vexa_types::MemoryFact {
                    key: key.clone(),
                    value,
                    revision: snapshot.git_revision.clone(),
                    created_unix_ms: snapshot.indexed_unix_ms,
                    source: vexa_types::Provenance {
                        path: source.clone(),
                        start_line: 1,
                        end_line: 1,
                        sha256: file.sha256.clone(),
                        git_revision: snapshot.git_revision,
                        observed_unix_ms: snapshot.indexed_unix_ms,
                        trust: vexa_types::TrustLevel::UserProvided,
                        verification: vexa_types::VerificationState::Unverified,
                    },
                },
            );
            vexa_memory::save(&memory_path, &memory)?;
            print_json(
                &json!({"stored": key, "path": memory_path, "note": "File hash links provenance; it does not prove the user-supplied assertion."}),
            )
        }
    }
}

fn run_doctor(args: &OutputArgs) -> Result<()> {
    let core = CoreConfig::detect()?;
    let config_path = config::config_path()?;
    let config_result = config::load();
    let target = PlatformTarget::detect();
    let cache_writable = test_directory_writable(&core.cache_root);
    let install_root = install_root()?;
    let hooks_path = codex_hooks_path();
    let hook_registered = hooks_path
        .as_ref()
        .and_then(|path| fs::read_to_string(path).ok())
        .is_some_and(|text| text.contains("vexa hook codex-user-prompt-submit"));
    let output = json!({
        "version": VERSION,
        "profile": format!("{:?}", detect_profile()).to_ascii_lowercase(),
        "platform_target": target.as_ref().map(ToString::to_string).ok(),
        "platform_supported": target.is_ok(),
        "config_path": config_path,
        "config_valid": config_result.is_ok(),
        "cache_root": core.cache_root,
        "cache_writable": cache_writable,
        "install_root": install_root,
        "codex_hooks_path": hooks_path,
        "codex_hook_registered": hook_registered,
        "telemetry_default_off": true,
        "semantic_required": false,
    });
    if args.json {
        print_json(&output)?;
    } else {
        println!("{}", serde_json::to_string_pretty(&output)?);
    }
    if !cache_writable || config_result.is_err() || target.is_err() {
        bail!("doctor found one or more required environment problems");
    }
    Ok(())
}

fn run_status(args: &StatusArgs) -> Result<()> {
    let core = CoreConfig::detect()?;
    let config = config::load()?;
    let index_path = cached_index_path(&args.cwd, &core)?;
    let index = load_snapshot(&index_path).ok();
    let output = json!({
        "version": VERSION,
        "configured_profile": config.profile,
        "detected_profile": detect_profile(),
        "semantic_enabled": config.semantic,
        "telemetry_enabled": config.telemetry,
        "auto_update": config.auto_update,
        "index_path": index_path,
        "indexed_files": index.as_ref().map(|snapshot| snapshot.files.len()),
        "git_revision": index.and_then(|snapshot| snapshot.git_revision),
    });
    if args.json {
        print_json(&output)
    } else {
        println!("{}", serde_json::to_string_pretty(&output)?);
        Ok(())
    }
}

fn run_config(args: ConfigArgs) -> Result<()> {
    match args.command {
        ConfigCommand::Show => print_json(&config::load()?),
        ConfigCommand::Path => {
            println!("{}", config::config_path()?.display());
            Ok(())
        }
        ConfigCommand::Set { key, value } => {
            let mut current = config::load()?;
            config::set_value(&mut current, &key, &value)?;
            let path = config::save(&current)?;
            print_json(&json!({"updated": key, "path": path, "config": current}))
        }
        ConfigCommand::Reset => {
            let defaults = config::VexaConfig::default();
            let path = config::save(&defaults)?;
            print_json(&json!({"reset": true, "path": path, "config": defaults}))
        }
    }
}

fn run_benchmark(args: &BenchmarkArgs) -> Result<()> {
    let suite_path = args.suite.canonicalize().with_context(|| {
        format!(
            "cannot canonicalize benchmark suite {}",
            args.suite.display()
        )
    })?;
    if suite_path.file_name().and_then(|name| name.to_str()) != Some("benchmark-suite.v1.json") {
        bail!("the current benchmark runner supports benchmark-suite.v1.json");
    }
    let benchmarks_root = suite_path
        .parent()
        .context("benchmark suite path has no parent")?;
    let repository_root = benchmarks_root
        .parent()
        .context("benchmarks directory has no repository parent")?;
    if args
        .source_fraction
        .is_some_and(|value| !(value > 0.0 && value <= 1.0))
    {
        bail!("source_fraction must be greater than zero and at most one");
    }
    let report = benchmark::run_benchmark_suite(
        repository_root,
        args.max_context_tokens,
        args.source_fraction,
    )?;
    let failed = report.aggregate.failed > 0 || report.aggregate.errors > 0;
    if let Some(path) = &args.output {
        let bytes = serde_json::to_vec_pretty(&report)?;
        fs::write(path, bytes).with_context(|| format!("cannot write {}", path.display()))?;
    }
    if args.json || args.output.is_none() {
        print_json(&report)?;
    } else {
        let path = args
            .output
            .as_ref()
            .context("benchmark output path missing")?;
        println!("Benchmark report written to {}", path.display());
    }
    if failed {
        bail!("benchmark has failed tasks; inspect the saved report");
    }
    Ok(())
}

fn run_update(args: &UpdateArgs) -> Result<()> {
    let target = PlatformTarget::detect()?;
    let root = install_root()?.join("updates");
    let updater = Updater::new(&root);
    let update = updater.check_for_update(VERSION, target)?;
    match update {
        None => print_json(
            &json!({"update_available": false, "current_version": VERSION, "target": target.to_string()}),
        ),
        Some(update) if args.check => print_json(&json!({
            "update_available": true,
            "current_version": VERSION,
            "available_version": update.release.version,
            "artifact": update.artifact.name,
            "target": update.target.to_string()
        })),
        Some(update) => {
            let (staged, activated) = updater.stage_and_activate(&update)?;
            let output = json!({
                "update_available": true,
                "staged": true,
                "activation_pending": false,
                "activated": activated.current,
                "version": staged.version,
                "artifact": staged.artifact_path,
                "sha256": staged.sha256,
                "note": "Verified and health-checked; the stable launcher selects this release on the next invocation."
            });
            if args.json {
                print_json(&output)
            } else {
                println!("{}", serde_json::to_string_pretty(&output)?);
                Ok(())
            }
        }
    }
}

fn run_rollback(args: &OutputArgs) -> Result<()> {
    let updater = Updater::new(install_root()?.join("updates"));
    let state = updater.rollback()?;
    let output = json!({
        "current": state.current,
        "previous": state.previous,
        "activation_pending": false,
        "note": "Previous verified version selected for the next launcher invocation."
    });
    if args.json {
        print_json(&output)
    } else {
        println!("{}", serde_json::to_string_pretty(&output)?);
        Ok(())
    }
}

fn run_uninstall(args: &UninstallArgs) -> Result<()> {
    let cache_root = CoreConfig::detect()?.cache_root;
    let config_root = config::config_root()?;
    let mut removed = Vec::<String>::new();
    if args.all || args.cache {
        remove_owned_tree(&cache_root, &mut removed)?;
    } else if args.indexes {
        remove_owned_tree(&cache_root.join("indexes"), &mut removed)?;
    }
    if args.all || args.config {
        remove_owned_tree(&config_root, &mut removed)?;
    }
    let output = json!({
        "removed": removed,
        "binary_removed": false,
        "note": "The running executable cannot safely delete its own launcher. Use the platform uninstall script for binaries and PATH/Codex integration removal."
    });
    if args.json {
        print_json(&output)
    } else {
        println!("{}", serde_json::to_string_pretty(&output)?);
        Ok(())
    }
}

fn run_version(args: &OutputArgs) -> Result<()> {
    if args.json {
        print_json(&json!({"name":"VEXA","version":VERSION,"api":VEXA_API_VERSION}))
    } else {
        println!("VEXA {VERSION}");
        Ok(())
    }
}

fn run_hook(args: &HookArgs, verbose: bool) -> Result<()> {
    match args.command {
        HookCommand::UserPromptSubmit => run_codex_prompt_hook(verbose),
        HookCommand::Register | HookCommand::Unregister => {
            let register = matches!(args.command, HookCommand::Register);
            let path = codex_hooks_path().context("cannot determine Codex hooks path")?;
            hooks::configure(&path, register)?;
            print_json(
                &json!({"hooks_path": path, "registered": register, "trust_review_required": register}),
            )
        }
    }
}

fn run_codex_prompt_hook(verbose: bool) -> Result<()> {
    let input_text = read_stdin_to_string()?;
    let input: CodexPromptHookInput =
        serde_json::from_str(&input_text).context("invalid Codex hook input JSON")?;
    let config = match config::load() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("vexa hook: configuration could not be loaded: {error:#}");
            return Ok(());
        }
    };
    let request = PrepareRequest {
        vexa_api: VEXA_API_VERSION.to_owned(),
        op: "prepare".to_owned(),
        cwd: input.cwd,
        prompt: input.prompt,
        model: input.model,
        mode: config.profile,
        max_context_tokens: config.max_context_tokens.clamp(128, 2200),
        diagnostics: Vec::new(),
        include_paths: Vec::new(),
    };
    let core = match CoreConfig::detect() {
        Ok(core) => core,
        Err(error) => {
            eprintln!("vexa hook: {error:#}");
            return Ok(());
        }
    };
    let response = match prepare_additive_context(&request, &core) {
        Ok(response) => response,
        Err(error) => {
            eprintln!("vexa hook: context preparation failed: {error:#}");
            return Ok(());
        }
    };
    if verbose {
        eprintln!(
            "vexa hook: confidence={:.3} packet_tokens={} total_ms={:.2}",
            response.confidence,
            response.estimated_packet_tokens,
            response
                .timings_ms
                .get("total")
                .copied()
                .unwrap_or_default()
        );
    }
    print_json_compact(&CodexPromptHookOutput {
        hook_specific_output: HookSpecificOutput {
            hook_event_name: "UserPromptSubmit",
            additional_context: response.packet,
        },
    })?;
    if config.auto_update {
        start_eligible_update();
    }
    Ok(())
}

/// Checks happen only after a submitted prompt, at most daily, in an installed layout.
/// The child has no repository input and never holds up prompt context delivery.
fn start_eligible_update() {
    let Ok(root) = install_root() else {
        return;
    };
    if !root.join("current.json").is_file() {
        return;
    }
    let marker = root.join("updates").join("auto-check");
    if fs::metadata(&marker)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|time| time.elapsed().ok())
        .is_some_and(|age| age.as_secs() < 86400)
    {
        return;
    }
    let Some(parent) = marker.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() || fs::write(&marker, b"eligibility-check\n").is_err() {
        return;
    }
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let mut command = std::process::Command::new(executable);
    command
        .args(["update", "--json"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0800_0000);
    }
    let _ = command.spawn();
}

fn run_protocol(args: &ProtocolArgs, verbose: bool) -> Result<()> {
    if args.format != "jsonl" {
        bail!("only --format jsonl is currently supported");
    }
    let core = CoreConfig::detect()?;
    let idle_timeout = std::time::Duration::from_secs(config::load()?.warm_timeout_seconds.max(1));
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let mut stdout = io::stdout().lock();
    while let Ok(line) = receiver.recv_timeout(idle_timeout) {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response: Value = match serde_json::from_str::<PrepareRequest>(&line) {
            Ok(request) => match prepare(&request, &core) {
                Ok(response) => serde_json::to_value(response)?,
                Err(error) => {
                    json!({"vexa_api":VEXA_API_VERSION,"status":"error","error":format!("{error:#}")})
                }
            },
            Err(error) => {
                json!({"vexa_api":VEXA_API_VERSION,"status":"error","error":format!("invalid request JSON: {error}")})
            }
        };
        serde_json::to_writer(&mut stdout, &response)?;
        writeln!(&mut stdout)?;
        stdout.flush()?;
        if verbose {
            eprintln!("vexa protocol: handled one request");
        }
    }
    Ok(())
}

fn install_root() -> Result<PathBuf> {
    if let Some(value) = std::env::var_os("VEXA_INSTALL_ROOT") {
        return Ok(PathBuf::from(value));
    }
    if cfg!(windows) {
        dirs::data_local_dir()
            .context("cannot determine LOCALAPPDATA")
            .map(|path| path.join("VEXA"))
    } else {
        dirs::data_local_dir()
            .context("cannot determine XDG data directory")
            .map(|path| path.join("vexa"))
    }
}

fn codex_hooks_path() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("CODEX_HOME") {
        return Some(PathBuf::from(home).join("hooks.json"));
    }
    dirs::home_dir().map(|home| home.join(".codex").join("hooks.json"))
}

fn test_directory_writable(path: &Path) -> bool {
    if fs::create_dir_all(path).is_err() {
        return false;
    }
    let name = format!(".doctor-{}", std::process::id());
    let probe = path.join(name);
    match fs::write(&probe, b"vexa") {
        Ok(()) => {
            let _ = fs::remove_file(probe);
            true
        }
        Err(_) => false,
    }
}

fn remove_owned_tree(path: &Path, removed: &mut Vec<String>) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let canonical = path
        .canonicalize()
        .with_context(|| format!("cannot canonicalize {}", path.display()))?;
    let home = dirs::home_dir().context("cannot determine user home")?;
    let home = home.canonicalize().unwrap_or(home);
    if canonical == home || !canonical.starts_with(&home) {
        bail!(
            "refusing to remove path outside the current user home: {}",
            canonical.display()
        );
    }
    fs::remove_dir_all(&canonical)
        .with_context(|| format!("cannot remove {}", canonical.display()))?;
    removed.push(canonical.display().to_string());
    Ok(())
}

fn read_stdin_to_string() -> Result<String> {
    let mut value = String::new();
    io::stdin().read_to_string(&mut value)?;
    Ok(value)
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn print_json_compact<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_hook_output_uses_supported_shape() {
        let output = CodexPromptHookOutput {
            hook_specific_output: HookSpecificOutput {
                hook_event_name: "UserPromptSubmit",
                additional_context: "context".to_owned(),
            },
        };
        let value = serde_json::to_value(output)
            .unwrap_or_else(|error| panic!("hook output serialization failed: {error}"));
        assert_eq!(
            value["hookSpecificOutput"]["hookEventName"],
            "UserPromptSubmit"
        );
        assert_eq!(value["hookSpecificOutput"]["additionalContext"], "context");
    }
}
