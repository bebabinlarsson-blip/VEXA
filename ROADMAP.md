# VEXA roadmap

This roadmap separates shipped source behavior from integration work and research targets. It does not assign dates or claim unmeasured outcomes.

## Implemented v0.1 foundation

- Deterministic repository indexing with hashes, language metadata, symbols, imports, Git state, ignore rules, and sensitive/generated/vendor filtering.
- Deterministic retrieval and bounded PACK context with provenance and untrusted-evidence labeling.
- Live verification for file, hash, symbol, and signature claims plus deterministic log reduction.
- Local project-memory primitives with provenance and stale-fact invalidation.
- CLI preparation/index/inspection/verification/configuration/status/diagnostic/protocol surfaces.
- Codex `UserPromptSubmit` adapter that supplies additive developer context while preserving the original user prompt.
- Verified update discovery, bounded staging, health checking, activation, and checksum-verified rollback from the official GitHub release source.
- Stable BOOT launcher implementation in the root workspace plus conservative per-user Windows/Linux installer and uninstaller scripts.
- Deterministic benchmark corpus and runner for retrieval, guards, token estimates, latency, freshness, symlink handling, and log reduction.
- CI on Ubuntu 24.04 and Windows 2025, with release packaging for Windows x86_64, Linux x86_64 glibc, and cross-built Linux aarch64 musl.

## Validation and research work remaining

- Keep archive/installer smoke in CI; Windows has executed locally, Linux evidence must come from real CI runs.
- Run matched model A/B experiments over roughly 50–100 meaningful coding tasks across repositories before claiming the core token-efficiency hypothesis is established.
- Improve packet overhead and measured full-hash refresh cost without weakening provenance or freshness. Current small-fixture results do not achieve the compression research gate.
- Validate the musl artifact and performance on native Raspberry Pi hardware. Native ARM glibc CI and cross compilation provide different evidence.

## Research targets

- Measure deterministic retrieval recall, packet reduction, and preparation latency on reproducible runs and publish observed results separately from goals.
- Run controlled model-backed A/B task-success experiments with matched model/provider settings and scoring.
- Evaluate whether an optional semantic reranker improves measured retrieval failures enough to justify its cost. No semantic model is currently shipped, and deterministic exact relationships remain the correctness foundation.
- Explore additional host integrations through documented interfaces such as MCP without claiming transparent prompt interception where the host does not provide it.

See [Limitations](docs/limitations.md) and [Benchmark methodology](docs/benchmarks.md) for the current evidence boundaries.
