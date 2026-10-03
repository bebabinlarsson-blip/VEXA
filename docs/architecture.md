# Architecture

VEXA is a local context coprocessor. It reduces repository exploration and repeated context sent to a frontier model while preserving exact evidence and machine-checkable constraints.

The deterministic request path is:

1. **CORE** receives a versioned request and chooses a hardware profile and context budget.
2. **INDX** refreshes a repository manifest. It records file hashes, parser/language metadata, symbols, imports, Git state, generated/vendor classification, lexical statistics, and related tests. It never executes repository code to build the index.
3. **SEEK** creates candidates with independent lexical, symbol, path, graph, Git, test, diagnostic, and recency signals.
4. **PACK** verifies current hashes before including source, retains the current user instruction, preserves protected constraints, marks repository text as untrusted evidence, and emits a compact packet with provenance.
5. **VFY** checks live paths, hashes, parsed symbols, exact signatures, and supplied build/test logs. These are separate from executing a build or test. Missing execution status and incomplete parser evidence remain unknown.
6. **MEM** stores structured, provenance-linked project facts locally and invalidates stale facts when their source changes.

The optional semantic layer is deliberately outside the correctness foundation. Lite mode works without model weights or a GPU. A semantic reranker can be added only when measured retrieval failures justify it and the deterministic signals remain authoritative for exact relationships.

Refresh hashes every accepted file, then reuses symbols/terms only when the content hash and parser version match. It does not yet use filesystem watchers or a persisted inverted index. This favors freshness over million-line repository latency. Import edges resolve by path/stem heuristics; a complete caller/callee graph is not implemented. Hardware profiles currently change candidate limits; they do not run a hardware calibration benchmark or load different models.

Retrieval confidence is a heuristic score, not a calibrated correctness probability. Strong queries prune weak lexical-only matches while retaining exact-symbol, graph, diagnostic, and test anchors; weak queries expand relationships. Additional paths remain requestable. Low recall under severe budgets is reported rather than hidden.

Memory facts are stored explicitly through the CLI and invalidated on inspection against current hashes/revision. They are not automatically learned from conversations or injected into every PACK packet. A matching source hash establishes freshness, not the truth of a user assertion.

## Trust boundary

User instructions and VEXA metadata are authority. Repository text, comments, docs, generated code, compiler output, and retrieved snippets are evidence. PACK labels repository material as untrusted content so source text such as `ignore previous instructions` never becomes a VEXA policy instruction.

## Local protocol

The canonical interface is newline-delimited JSON over stdin/stdout. `schemas/vexa-api-v1.schema.json` describes the current `prepare` request/response. Named pipes on Windows and Unix-domain sockets on Linux can be added as acceleration layers without changing the logical API.

## Codex integration

Current Codex supports a `UserPromptSubmit` hook that receives the original prompt and model metadata and can add model-visible context. It does not provide a prompt replacement field. The VEXA hook therefore treats the original prompt as immutable and emits only an additive VEXA packet. See `docs/integrations.md` for the concrete adapter and limitations.

