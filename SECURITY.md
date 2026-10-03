# Security policy

VEXA is currently a `0.1.0` development project. Security fixes are made against the current development line; this repository does not yet claim long-term support for older release branches.

## Reporting a vulnerability

Please avoid publishing exploitable details, credentials, private repository content, or proof-of-concept secrets in a public issue.

If this repository's GitHub **Security** tab offers private vulnerability reporting, use that channel. If no private reporting channel is available, open a minimal public issue that says a private security report is needed without including exploit details, then continue through a private contact method provided by the repository owner.

A useful report includes the affected VEXA version or commit, platform, entry point, impact, reproduction conditions, and whether the issue crosses a repository-root, update-verification, hook, or local-data boundary. Use synthetic data in reproductions whenever possible.

## Security boundaries

The implemented security model is documented in [docs/security-model.md](docs/security-model.md). Key current properties include:

- repository indexing/retrieval does not execute repository code;
- repository content is packed as untrusted evidence rather than policy instructions;
- `.gitignore`, `.vexaignore`, sensitive-path, generated, and vendor exclusions are active by default;
- candidate source paths are constrained to the canonical repository root, including symlink handling;
- live hashes are checked before indexed symbol/signature claims are treated as verified;
- update staging fetches only from the configured official VEXA repository and verifies the selected release artifact against that release's SHA-256 manifest before finalizing the staged artifact;
- the updater does not execute or activate a downloaded payload;
- the platform installers restrict release downloads to the official GitHub repository, verify the archive digest before extraction, and run the staged core's `doctor` command before changing the active BOOT manifest;
- local-data deletion refuses to recursively remove paths outside the current user's home directory.

These properties do not make arbitrary repository contents trustworthy and do not replace the host model/provider's own security controls. The Codex hook sends selected context into the active Codex turn after local preparation.

## Dependency and release checks

Normal CI runs formatting, Clippy, and tests. The heavy/release workflows additionally run dependency policy checks with `cargo-deny` and vulnerability auditing with `cargo-audit`. Release packaging generates SPDX SBOMs, SHA-256 manifests, and GitHub attestations as described in [docs/releasing.md](docs/releasing.md).

For privacy and local-storage behavior, see [docs/privacy.md](docs/privacy.md).
