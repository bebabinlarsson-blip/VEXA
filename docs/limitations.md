# Limitations

This page records the current integration and validation boundaries of VEXA. For setup details, see [Integrations](integrations.md); for the deterministic pipeline, see [Architecture](architecture.md).

## Codex and ChatGPT Work

VEXA's supported automatic prompt-time integration is the Codex `UserPromptSubmit` lifecycle hook. The supplied registration scripts add `vexa hook codex-user-prompt-submit` to the per-user Codex hook file and preserve unrelated hook configuration. Non-managed hooks must still be reviewed and trusted in Codex before they run.

OpenAI documents the same Codex hook runtime for Codex and ChatGPT Work. Ordinary ChatGPT chats do not expose this transparent `UserPromptSubmit` lifecycle hook. Plugins and MCP servers can provide tools, resources, instructions, and model-visible results in ChatGPT, but that is a separate integration path and does not guarantee a local command runs before every ordinary chat message.

## Additive context only

For `UserPromptSubmit`, VEXA can return plain-text context or `hookSpecificOutput.additionalContext`, which Codex adds as developer context for the turn. The hook can also block a submission using Codex's documented blocking result, but it has no supported field for replacing or rewriting the user's prompt. VEXA therefore treats the submitted prompt as immutable.

The example hook sets `additionalContextLimit` to 2500 tokens. Codex may spill larger additional-context output to disk and send a shorter preview to the model, so VEXA should keep prompt-time context compact.

## Models

The hook input includes Codex's active `model` slug as metadata. The integration scripts do not select, override, or hardcode a model identifier, and VEXA should not require a particular model name for the hook contract to work.

## Semantic retrieval

The shipped correctness path is deterministic, and no semantic reranker or embedding model is shipped by default. The architecture keeps semantic reranking as an optional layer outside that foundation; the default path does not require model weights or a GPU.

## Benchmark evidence

Actual deterministic retrieval, seeded-claim verification, compression curves, cold/warm latency, memory, CPU time, and footprint measurements are recorded in [Benchmark results](benchmark-results.md). The corpus currently contains 15 authored retrieval tasks across nine languages and three fixture repositories; these are not 50–100 independent coding tasks or proof of broad repository accuracy. The fixture validator checks structure only; the Rust runner measures retrieval/guards.

Matched frontier-model A/B task success, patch correctness, actual provider-token savings, TTFT, reasoning tokens, and frontier repository hallucination rates are NOT MEASURED. No semantic model should be added to hide these missing gates. The small fixtures can produce larger packets than their indexed source because immutable instructions and provenance have overhead; see the actual reduction measurements. Research targets are not release promises.

## Operating-system validation

Windows x86_64 and Linux x86_64 have each passed 77 Rust tests. Linux ran in a Debian 12 container through Docker Desktop, with clippy, all 15 retrieval tasks, all 13 seeded claims, and four supplied-log classifications passing. Both packages passed offline install, manual upgrade, reinstall, rollback in both directions, hook registration, checksum rejection, and uninstall with user configuration preserved. See [Linux environment evidence](../benchmarks/results/environment-linux-x64.json). Container timings are diagnostics, not hardware performance evidence.

CI is configured for Ubuntu 24.04 x86_64, Ubuntu 24.04 ARM, and Windows 2025; hosted CI has not executed. The release workflow packages Windows x86_64, Linux x86_64 glibc, and Linux ARM64 musl. ARM musl emulation, ARM glibc tests, and native Raspberry Pi validation are distinct evidence levels.

ARM64 musl passed 77 Rust tests under emulation. Its optimized, statically linked cross-built release passed 15/15 retrieval tasks, 13/13 seeded claims, 4/4 supplied logs, and the complete offline installer lifecycle smoke under ARM64 emulation. [ARM environment and artifact hashes](../benchmarks/results/environment-linux-arm64-musl.json) identify the actual build and runtime images. Native ARM hardware and ARM glibc execution remain unverified.

The current CI and release matrices do not validate macOS or Windows on ARM, and no release artifact for those platforms is listed in [Releasing VEXA](releasing.md). Platform support claims should stay within the environments that are actually tested or packaged.

The Codex adapter has been exercised through its JSON contract with unknown model metadata. A live user-trusted Codex turn still requires host trust review. No unsupported trust bypass or private application interception is used.

First use still performs full deterministic indexing, and refresh hashes all accepted files. Million-line indexing latency and Pi-class hardware performance are unverified. The warm JSONL process is available to clients, but the prompt hook currently starts a separate process each turn. Automatic conversation learning, complete caller/callee analysis, neural inference, and pre-tool read rewriting are not shipped.
