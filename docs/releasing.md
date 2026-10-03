# Releasing VEXA

VEXA releases are built by GitHub Actions from an existing `v<version>` tag. The tag must match the `[workspace.package]` version in `Cargo.toml`, for example `v0.1.0` for version `0.1.0`.

The release workflow runs formatting, clippy, tests, `cargo-deny`, `cargo-audit`, package layout checks, and offline install/uninstall plus Codex hook lifecycle smoke checks on native Windows and Linux before it creates a GitHub Release. The supported release artifacts are:

- `x86_64-pc-windows-msvc` as a ZIP archive.
- `x86_64-unknown-linux-gnu` as a `tar.gz` archive.
- `aarch64-unknown-linux-musl` as a `tar.gz` archive built with `cross` on an Ubuntu runner.

Each platform archive contains the stable BOOT launcher (`vexa-boot` or `vexa-boot.exe`), the versioned CLI core (`vexa-core` or `vexa-core.exe`), the project license, and `THIRD_PARTY_LICENSES.txt`. Package validation fails if a required file is missing.

Each target also gets an SPDX JSON SBOM generated from the tagged Cargo workspace. The workflow creates build-provenance and SBOM attestations for each package, then produces one `SHA256SUMS` file over every package and SBOM. The checksum manifest receives its own attestation.

The SBOM is a Cargo metadata inventory of the workspace, including dependencies beyond a single binary's linked contents. Local `cargo sbom` generation has executed and produced SPDX 2.3 JSON. GitHub attestations require the hosted workflow; they are not present in local package outputs.

The final publish job has the only `contents: write` permission in the workflow. It creates a draft release, uploads the complete bundle, and converts the draft to a published release only after all validation, security, package, SBOM, attestation, and checksum jobs succeed. If a draft already exists, a rerun may replace draft assets; a previously published release causes the workflow to stop instead of mutating it.

## Preparing a release

1. Ensure the intended version is already present in the workspace metadata and the change has passed normal CI.
2. Create and push an annotated `v<version>` tag pointing at the exact commit to release.
3. Let the tag-triggered `Release` workflow complete, or use its manual dispatch with the existing tag name.
4. After publication, verify the release and its assets before distributing links.

GitHub repository settings should protect `v*` tags with a ruleset that restricts creation, update, and deletion to maintainers or the release identity. Enable GitHub Release Immutability before public releases so a published release locks its tag and assets and receives GitHub's release attestation.

## Verifying a release

With GitHub CLI installed, verify the release and a downloaded asset:

```sh
gh release verify v0.1.0
gh release verify-asset v0.1.0 vexa-0.1.0-x86_64-unknown-linux-gnu.tar.gz
gh attestation verify vexa-0.1.0-x86_64-unknown-linux-gnu.tar.gz -R bebabinlarsson-blip/VEXA
```

Verify checksums independently with native tools:

```sh
sha256sum --check SHA256SUMS
```

On Windows PowerShell, compare an individual entry with `Get-FileHash -Algorithm SHA256`.

## Action pinning and tool versions

The workflows pin GitHub Actions to commit SHAs that were resolved from their official repositories when this release automation was added. The comments beside those SHAs record the moving major tag that was resolved. Tooling installed from crates.io is version-pinned (`cargo-audit`, `cargo-sbom`, and `cross`) and uses `--locked`.

When updating an action, resolve the intended official tag again and review the upstream release notes before replacing its commit SHA. Do not invent or hand-edit action SHAs.

The Rust toolchain is pinned in `rust-toolchain.toml`. Hosted runners are explicitly `ubuntu-24.04` and `windows-2025` so changes to GitHub's `*-latest` aliases do not silently change the release environment.
