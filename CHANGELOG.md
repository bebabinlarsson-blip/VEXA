# Changelog

All notable project changes should be recorded here. VEXA currently identifies itself as version `0.1.0`; this checkout contains no Git release tag, so the entries below describe the present development line rather than a claimed published release.

## Unreleased

### Added

- Deterministic Rust indexing, retrieval, packing, verification, memory, update-staging, and orchestration crates.
- CLI preparation and explicit expansion, index management, inspection, claim correction packets, provenance-linked memory storage/inspection/clearing, diagnostics, configuration, real update/rollback, local-data removal, Codex hooks, and an idle-expiring JSONL protocol.
- Codex `UserPromptSubmit` integration using additive developer context through `vexa hook codex-user-prompt-submit`.
- Conservative per-user Codex hook registration and unregistration scripts for PowerShell and POSIX shell.
- Deterministic benchmark corpus and runner covering polyglot retrieval, freshness, guard behavior, symlink escape, secret-neighbour, prompt-injection, duplicate/misleading symbols, and giant-log reduction.
- CI, dependency-policy/security checks, release packaging, SPDX SBOM generation, checksum manifests, and GitHub attestations.
- `vexa-boot` stable launcher in the root workspace for a versioned install tree selected by `current.json`.
- Per-user Windows and Linux installer/uninstaller assets with official-release URL checks, SHA-256 verification, versioned install directories, doctor-before-activation, PATH handling, and optional Codex registration.

### Boundaries

- No semantic model or embedding runtime is shipped by default; current SEEK semantic scores are zero.
- Actual deterministic benchmark reports are recorded. Broad model task success and token-efficiency research gates remain unproven.
- Windows offline lifecycle smoke has executed; Linux and ARM evidence is recorded separately from configured workflows.
- Checksums trust official GitHub HTTPS; independently authenticated signatures and commercial Windows code signing are not shipped.
