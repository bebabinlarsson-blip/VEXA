# VEXA

VEXA is a local-first Rust context coprocessor for coding agents and frontier-model workflows. It indexes a repository without executing repository code, retrieves a small evidence set with deterministic signals, verifies live file state, and packs provenance-linked context for a model or host integration.

VEXA `0.1.0` is an unreleased deterministic foundation: local indexing, evidence retrieval, provenance-preserving context packets, machine-checkable verification, Codex hooks, and a versioned installation lifecycle. Measured results and remaining research gates are recorded in [Benchmark results](docs/benchmark-results.md). No semantic model is shipped.

## What is implemented

The root Rust workspace contains the following crates:

| Crate | Responsibility |
| --- | --- |
| `vexa-types` | Versioned shared request, index, evidence, memory, and benchmark types. |
| `vexa-index` | Repository walking, ignore rules, hashing, language metadata, symbols, imports, Git state, and cached snapshots. |
| `vexa-seek` | Deterministic lexical, symbol, path, graph, Git, test, diagnostic, and recency ranking. The semantic score is currently zero. |
| `vexa-pack` | Context budgeting, protected-constraint handling, provenance, and explicit untrusted-repository evidence boundaries. |
| `vexa-memory` | Local provenance-linked project memory primitives and stale-fact invalidation. |
| `vexa-verify` | Live file/hash/symbol/signature checks and deterministic build/test log reduction. |
| `vexa-update` | Official GitHub release discovery, exact target selection, bounded checksum-verified extraction, health checking, activation, and verified rollback. |
| `vexa-core` | Deterministic orchestration, hardware-profile selection, indexing, retrieval, packing, and local protocol behavior. |
| `vexa-boot` | Stable launcher for a versioned install tree; validates `current.json` and starts `versions/<version>/vexa-core`. |
| `vexa-cli` | The `vexa` command surface used by packaged distributions and the Codex hook adapter. |

The main request path is:

```text
prompt + repository
        |
        v
      CORE
        |
        +--> INDX: hash/index current repository state
        +--> SEEK: rank deterministic evidence candidates
        +--> PACK: build a bounded, provenance-linked packet
        +--> VFY: verify live claims when requested
        +--> MEM: inspect/invalidate local project facts
```

See [Architecture](docs/architecture.md) and [Limitations](docs/limitations.md) for the detailed trust and validation boundaries.

## Install

Per-user installer assets are implemented for Windows and Linux. They resolve an official GitHub release, download the exact platform archive and `SHA256SUMS`, verify SHA-256 before extraction, stage a versioned install, run `vexa-core doctor`, then update the BOOT launcher/current manifest only after the health check succeeds. They do not require administrator access, Python, Node.js, Rust, or Git.

No public `0.1.0` release is available yet. Until publication, build from this source and use the offline package inputs described in [Installing VEXA](docs/installing.md). Online install and update require published official assets.

Run installers from a trusted local checkout or verified release asset; do not pipe a network response directly into a shell:

```powershell
.\install.ps1
```

```sh
sh ./install.sh
```

Current release targets are:

- `x86_64-pc-windows-msvc` as ZIP;
- `x86_64-unknown-linux-gnu` as `tar.gz`;
- `aarch64-unknown-linux-musl` as a cross-built `tar.gz`.

When a release is published, the workflow also creates `SHA256SUMS`, SPDX SBOMs, and GitHub attestations. See [Installing VEXA](docs/installing.md) and [Releasing VEXA](docs/releasing.md) for the exact lifecycle.

The Windows offline installer smoke exercises the actual packaged BOOT/core layout, checksum rejection, idempotent hook registration, and uninstall. Linux lifecycle checks run in CI; see [Limitations](docs/limitations.md) for executed platform evidence.

For development from source, use the pinned Rust `1.90.0` toolchain:

```sh
cargo build --locked --release --package vexa-cli --package vexa-boot
```

The user-facing installed launcher is named `vexa`/`vexa.exe`; it dispatches to the selected version's `vexa-core`. The public CLI examples below assume that launcher is on `PATH`.

## Quick start

Inspect the environment and current configuration:

```sh
vexa version
vexa doctor
vexa config show
```

Create or refresh an index for the current repository:

```sh
vexa index refresh --cwd .
vexa index status --cwd .
```

Prepare model context from an explicit prompt:

```sh
vexa prepare --cwd . --prompt "Find the code that validates release checksums"
```

Inspect the packet without sending it anywhere:

```sh
vexa inspect-packet --cwd . --prompt "Explain the update staging path"
```

Machine-readable output is available on commands that expose `--json`, for example:

```sh
vexa prepare --cwd . --prompt "Find the index refresh path" --json
vexa status --cwd . --json
```

The CLI includes `prepare`, `inspect-packet`, `index`, `inspect`, `verify`, `memory`, `doctor`, `status`, `config`, `benchmark`, `update`, `rollback`, `uninstall`, `version`, `hook`, and `protocol`. Use `--help` on a command for its complete options. `verify claims` accepts a JSON array and returns a correction packet plus nonzero status for contradicted or unknown claims. `prepare --include-path src/file.rs` explicitly expands evidence within the indexed, non-sensitive repository boundary.

## Codex integration

VEXA supports the current Codex `UserPromptSubmit` lifecycle hook. The hook command is:

```text
vexa hook codex-user-prompt-submit
```

Registration scripts merge that handler into the per-user Codex `hooks.json` while preserving unrelated hooks:

```powershell
./scripts/register-codex.ps1
```

```sh
./scripts/register-codex.sh
```

Both scripts call VEXA's native structural JSON registration command; no `jq` is required. Unrelated hooks are preserved. Codex requires the resulting non-managed hook to be reviewed and trusted through `/hooks`.

The integration returns `hookSpecificOutput.additionalContext`. It does not replace or rewrite the submitted user prompt. Ordinary ChatGPT conversations do not expose the same transparent local `UserPromptSubmit` lifecycle hook. See [Integrations](docs/integrations.md).

## Configuration

Use the CLI instead of guessing the platform-specific config path:

```sh
vexa config path
vexa config show
vexa config set profile lite
vexa config set max_context_tokens 2200
vexa config reset
```

The default configuration is also shown in [`examples/vexa-config.json`](examples/vexa-config.json). Only settings actually consumed by the current runtime should be treated as active behavior; several reserved fields are persisted for later lifecycle work. See [Configuration](docs/configuration.md).

## Update and rollback

`vexa update --check` queries the official VEXA GitHub release endpoint for a newer compatible release. `vexa update` downloads the exact selected release and its `SHA256SUMS`, verifies the checksum, safely extracts it, runs a bounded health check, and atomically switches BOOT's current version:

```sh
vexa update --check
vexa update
```

Failed verification or health checks preserve the selected version. The running process continues using its existing binary; the next invocation uses the new version. Older version files remain available for rollback.

`vexa rollback` verifies both installed cores and performs the manifest switch. Installed Codex hooks also launch an eligible update check at most daily after returning context, when `auto_update` is true. Disable this with `vexa config set auto_update false`.

## Uninstall and local data removal

The CLI can remove VEXA-owned local state inside the current user's home directory:

```sh
vexa uninstall --indexes
vexa uninstall --cache
vexa uninstall --config
vexa uninstall --all
```

This command does not delete the running binary. Use the platform uninstall scripts for installer-managed binaries, PATH changes, and optional Codex integration cleanup:

```powershell
.\scripts\uninstall.ps1
```

```sh
sh ./scripts/uninstall.sh
```

They preserve user configuration/state by default; pass `-PurgeUserData` on Windows or `--purge-user-data` on Linux to remove it too. Codex integration can also be removed separately with:

```powershell
./scripts/unregister-codex.ps1
```

or:

```sh
./scripts/unregister-codex.sh
```

## Security and privacy

VEXA's deterministic indexing/retrieval path is local-first. It does not execute repository code while indexing or retrieving. `.gitignore`, `.vexaignore`, generated/vendor exclusions, sensitive-path rules, canonical repository-root checks, hashes, and provenance are part of the implemented boundary.

Telemetry defaults to false, and no telemetry transport or analytics endpoint exists. Explicit updates and eligible automatic update checks contact GitHub; no repository context is sent by the updater. Codex integration returns selected repository context as model-visible additional context, subject to the provider and workspace policy of the Codex session.

Read [Security](SECURITY.md), [Security model](docs/security-model.md), and [Privacy](docs/privacy.md) before using VEXA with sensitive repositories.

## Benchmarks

The deterministic benchmark harness and corpus live under [`benchmarks/`](benchmarks/). The runner measures retrieval recall, guard behavior, estimated packet-token reduction, and preparation latency against isolated fixture copies, with stale-index, symlink, secret-neighbour, prompt-injection, duplicate-symbol, and giant-log cases.

[`docs/benchmark-results.md`](docs/benchmark-results.md) records real executions and links the raw reports. Fixture recall and packet-size estimates do not establish frontier-model task success or token savings. Matched model A/B experiments remain a separate research gate.

The static corpus can be validated without running VEXA:

```powershell
powershell -ExecutionPolicy Bypass -File benchmarks/tools/validate.ps1
```

See [Benchmark methodology](docs/benchmarks.md) and [Benchmark results](docs/benchmark-results.md).

## Development

The project uses Rust `1.90.0`, edition 2024, `unsafe_code = "forbid"`, and workspace Clippy rules that deny `unwrap_used` and `expect_used`.

The normal CI checks are:

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
```

CI runs workspace tests on Ubuntu 24.04 x86_64, Ubuntu 24.04 ARM, and Windows 2025. Installer smoke runs on x86_64 Linux and Windows. ARM musl packaging is a separate cross-build; see [Limitations](docs/limitations.md) for recorded evidence and [Roadmap](ROADMAP.md) for research work.

## License

VEXA is licensed under the [MIT License](LICENSE).
