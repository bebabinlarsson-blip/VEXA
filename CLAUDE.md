# VEXA

NO HAIKU.
Use Sonnet 5.5 for persistent implementation.
Use Opus 5.5 only for Senior/GOAT boundaries according to Laya routing (.ai/ROUTING.md).

Rust 1.90 workspace: INDEX, SEEK, PACK, MEMORY, VERIFY, CORE, UPDATE, BOOT, CLI, and shared types. See `.ai/PROJECT.md` for architecture and `.ai/VERIFY.md` for commands. Source lives in `crates/`; synthetic evidence lives in `benchmarks/`; installers and package smoke are in root scripts and `scripts/`.

Canonical checks: `cargo fmt --all -- --check`, `cargo test --locked --workspace --all-features`, and `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`. Run relevant benchmarks and package smoke after consequential changes.

Preserve MIT, unrelated edits, exact evidence, prompt constraints, secrets exclusions, verified checksums, and safe rollback. Keep generated local outputs ignored. Never present fake adapters as real model measurements. Never commit unless asked.
