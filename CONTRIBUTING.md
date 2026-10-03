# Contributing to VEXA

VEXA is an MIT-licensed Rust project. Contributions should preserve the project's deterministic, local-first correctness foundation and keep measured results separate from goals or hypotheses.

## Before changing code

Read the relevant design documents first:

- [Architecture](docs/architecture.md)
- [Security model](docs/security-model.md)
- [Limitations](docs/limitations.md)
- [Benchmark methodology](docs/benchmarks.md)
- [Configuration](docs/configuration.md)

The root workspace is pinned to Rust `1.90.0` with edition 2024. `unsafe_code` is forbidden. Workspace Clippy configuration denies `unwrap_used` and `expect_used`.

## Build and checks

Use locked dependencies for reproducible project checks:

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
```

Validate the benchmark fixture/schema corpus separately on Windows PowerShell with:

```powershell
powershell -ExecutionPolicy Bypass -File benchmarks/tools/validate.ps1
```

CI runs the root workspace test suite on Ubuntu 24.04 and Windows 2025. Heavy validation also runs `cargo-deny`, `cargo-audit`, and release-package builds.

Installer/packaging changes should be tested as one lifecycle: the produced archive layout must match what `install.ps1`/`install.sh` require (`vexa-boot` plus `vexa-core`), and activation should occur only after checksum verification and `doctor` succeed.

## Change expectations

Keep changes scoped and explain the observable behavior they alter. New retrieval signals should remain inspectable and deterministic unless the change explicitly introduces an optional research layer. Repository content must remain evidence, not an instruction source.

For changes that alter indexing, ranking, packing, verification, updater safety, or path handling, add focused tests that exercise the failure boundary as well as the success path. Do not turn missing evidence into a passing verification result.

If a change affects the CLI, update public documentation using the command names defined in `crates/vexa-cli/src/main.rs`. If it changes configuration, update `docs/configuration.md` and `examples/vexa-config.json` together when both are in scope.

## Benchmarks and performance claims

Do not copy target numbers into benchmark results. A performance, token-reduction, recall, or task-success claim belongs in project documentation only after a reproducible run has actually produced it.

The deterministic corpus under `benchmarks/` can measure local retrieval behavior without model credentials. Model-backed A/B experiments are separate and must hold model, provider settings, tasks, and scoring constant between control and treatment runs.

## Pull requests

A useful pull request description states the problem, the resulting behavior, and the validation performed. Keep unrelated refactors out of a behavioral fix. Preserve existing user data formats and protocol contracts unless the change intentionally versions them.

By contributing, you agree that your contribution is provided under the repository's [MIT License](LICENSE) and that project interactions follow the [Code of Conduct](CODE_OF_CONDUCT.md).
