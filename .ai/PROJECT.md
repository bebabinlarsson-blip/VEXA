# Project

## PROJECT PURPOSE
VEXA is an MIT-licensed local context coprocessor: deterministic repository indexing, retrieval, exact evidence packing, provenance, verification, and explicit memory.

## STACK
Rust 1.90, edition 2024, Cargo workspace with ten crates. Tree-sitter parses source; gix reads Git state. PowerShell and POSIX shell handle installation and packaging. Python standard-library tools measure benchmarks.

## IMPORTANT DIRECTORIES
- `crates/`: implementation and Rust tests.
- `benchmarks/`: synthetic fixtures, task contracts, measurement tools, recorded results.
- `scripts/`: package and installer lifecycle smoke checks.
- `docs/`, `schemas/`, `examples/`: contracts and usage.
- `.github/workflows/`: quality and release gates.
- `third-party/`: original license notices and required corresponding source.

## ARCHITECTURAL BOUNDARIES
INDEX reads eligible source; SEEK chooses candidates; PACK preserves exact excerpts and user constraints. VERIFY checks fresh source. CORE coordinates these stages. CLI exposes JSON and Codex hooks. BOOT selects versioned cores. UPDATE verifies official release packages and maintains rollback metadata.

## IMPORTANT INTERFACES
CLI `prepare --json`, JSONL `protocol`, Codex `UserPromptSubmit` additional context, versioned schemas, installed `current.json` and updater state.

## GENERATED FILES
`target/`, `.vexa/`, `dist/`, and `benchmark-results/` are local outputs. `THIRD_PARTY_LICENSES.txt` and `third-party/` come from `tools/generate_notices.py`. Recorded benchmark JSON must describe real runs.

## DO-NOT-CASUALLY-EDIT AREAS
Preserve MIT LICENSE, upstream notices, fixture bytes, packet trust boundaries, immutable user instructions, checksum verification, atomic selection, and unrelated user files. Do not claim model performance, Linux/ARM support, or native hardware validation without execution evidence.
