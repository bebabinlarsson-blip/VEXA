# Verification

## Install
Offline Windows: `./install.ps1 -ArchivePath <zip> -ChecksumsPath <SHA256SUMS> -Version <version> -NoModifyPath -NoCodex`.
Offline Linux: `sh install.sh --archive <tar.gz> --checksums <SHA256SUMS> --version <version> --no-modify-path --no-codex`.

## Build
`cargo build --locked --workspace`
`cargo build --locked --release --package vexa-boot --package vexa-cli --target x86_64-pc-windows-msvc`

## Unit tests
`cargo test --locked --workspace --all-features`
`python -m unittest discover -s benchmarks/tools -p test_model_ab.py`

## Integration tests
`cargo run --locked --package vexa-cli -- benchmark --output benchmark-results/regression.json`
`python benchmarks/tools/verify.py --binary target/debug/vexa-core.exe --output benchmark-results/verification.json`
`./scripts/smoke-package.ps1 -Archive <zip>`
`sh scripts/smoke-package.sh <tar.gz>`

## Typecheck
`cargo check --locked --workspace --all-targets --all-features`

## Lint
`cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`
`cargo audit`
`cargo deny check`

## Format/check
`cargo fmt --all -- --check`

## Dev server, production preview, browser/e2e
None configured. VEXA is a local CLI and context service.
