# Security and privacy model

VEXA is local-first. Telemetry is disabled by default and the deterministic pipeline does not need a VEXA cloud service.

Repository indexing obeys `.gitignore`, `.vexaignore`, built-in generated/vendor exclusions, and sensitive-path rules. Candidate source is canonicalized beneath the repository root; symlink targets outside the root are excluded. Common credential files, private keys, token stores, and environment-secret files are excluded from normal PACK output.

Repository content is never treated as an instruction source. PACK separates trusted metadata from untrusted repository evidence and carries file hashes and line provenance for included source.

VEXA does not execute repository scripts while indexing or retrieving. Build/test verification is an explicit action and must follow the caller's normal trust and execution policy. An unobserved command result is `unknown`, never `passed`.

Updates are accepted only from VEXA's fixed official GitHub repository over HTTPS. Asset URLs match the exact release path; redirects are restricted to GitHub's asset hosts. Bounded archives reject traversal, unexpected files, links, and duplicate members. SHA-256 is verified before extraction; health checks precede activation. Failed verification/self-tests preserve the selected version. Lifecycle locks exclude concurrent updater transactions.

Checksums fetched from the same release provide integrity under the HTTPS/GitHub trust model, not independent publisher authentication. The release workflow creates GitHub attestations; the installed updater does not independently verify them. No commercial Windows code-signing certificate or embedded publisher signing key is shipped.

Secret detection is conservative and heuristic; it can miss novel secrets and exclude legitimate examples. File-boundary and packet-marker protections are tested, but prompt-injection immunity in a frontier model is not established by labeling source as untrusted.

Users can inspect a packet before sending it to a frontier provider. Clearing project memory or indexes does not delete source repositories.

## Threats covered by regression tests

- prompt-injection strings embedded in source comments or docs;
- secret files adjacent to relevant source;
- symlink traversal outside the repository root;
- stale index hashes and stale memory facts;
- corrupt local index state;
- misleading filenames and duplicate symbols;
- updater checksum mismatch and interrupted staging;
- false build/test claims when no execution result exists.

## Out of scope

VEXA does not intercept TLS, inject into proprietary binaries, steal credentials, or patch private application internals. Ordinary ChatGPT web/chat has no supported invisible local pre-submit hook; integrations use documented Codex hooks, local MCP/plugin surfaces, or an explicitly configured client/gateway.

