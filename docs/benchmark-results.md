# Benchmark results

Recorded 2026-10-03 Europe/Stockholm on Windows x64 10.0.26200, Intel i5-10600KF (6 cores/12 threads), 32 GiB RAM, Rust 1.90.0. Binary hashes, environment, and footprint are in [environment evidence](../benchmarks/results/environment-windows-x64.json). Local desktop measurements are not controlled hardware rankings.

## Actual deterministic results

[Release-binary report](../benchmarks/results/windows-x64-fixtures.json): **15/15 tasks passed**, no errors/skips, all six adversarial guards passed. Required-file and required-symbol packet recall were 100% on 17 expected file/symbol observations across nine languages and three small fixture repositories. SEEK symbol recall counts symbols present in returned candidate files; PACK symbol recall requires the indexed symbol line range in an emitted excerpt. This small corpus is not a broad accuracy guarantee.

[Seeded verifier report](../benchmarks/results/windows-x64-verification.json): 13/13 claims classified correctly and four supplied-log cases matched expected status. False acceptance was 0/9 deliberately false or unsupported claims. This measures verifier correctness, **not frontier-model hallucination incidence**.

The [earlier failed report](../benchmarks/results/windows-x64-before-selection.json) retains 13/15 passes before generated-log exclusion and duplicate/weak-candidate selection changes. Its indexed-source baseline included generated logs; its reduction numbers are not directly comparable to the current baseline.

## Compression and research gates

At the default 1800-token estimate budget, median reduction against indexed source was **-41.41%**: the packet is larger than source for the median tiny fixture. Immutable prompts, provenance, and source boundaries have overhead. The >=70% median reduction research gate is **not achieved** by this corpus. Passing recall alone is insufficient.

The [executed curve](../benchmarks/results/windows-x64-performance-fixture.json) uses per-task budget = requested fraction of indexed UTF-8 source bytes/4, rounded up, with a 128-token floor. Protected prompts remain intact even when they exceed the budget. Fractions are requested budgets, not achieved compression percentages. Every insufficient-budget run returned nonzero and saved its report.

| Requested source fraction | Tasks passed | Required file/symbol recall | Median estimated reduction |
| --- | ---: | ---: | ---: |
| 100% | 13/15 | 88.24% | 17.32% |
| 75% | 13/15 | 88.24% | 39.50% |
| 50% | 7/15 | 52.94% | 57.74% |
| 30% | 0/15 | 0% | 77.03% |
| 20% | 0/15 | 0% | 77.03% |
| 10% | 0/15 | 0% | 72.97% |
| 5% | 0/15 | 0% | 72.97% |

Severe-budget apparent savings come from lost evidence and are unusable. This curve does not establish a safe frontier task-success elbow.

One [actual VEXA-repository request](../benchmarks/results/windows-x64-performance-vexa.json) for `build_index` emitted 1524 estimated tokens against 135966 indexed-source token estimates (98.88% less than reading the entire indexed repository). This is one full-source baseline, not savings over normal Codex exploration, a task-success result, or the corpus median.

## Performance and footprint

Cold values include process startup and fresh indexing with five distinct caches. Warm values use a persistent JSONL process after one discarded warmup, 30 round-trip samples, nearest-rank p95. Codex hooks start a fresh process per prompt, so persistent-protocol numbers are not warm-hook latency. RAM is Windows PeakWorkingSetSize; CPU is cumulative user+kernel time including warmup.

| Workload | Cold p95 | Warm round-trip p95 | Peak working set | Warm total CPU |
| --- | ---: | ---: | ---: | ---: |
| Small polyglot fixture | 79.23 ms | 32.91 ms | 13.20 MiB | 0.703 s / 31 requests |
| VEXA repository | 411.34 ms | 132.04 ms | 20.51 MiB | 3.531 s / 31 requests |

Default-budget fixture preparation p50/p95 was 33.25/43.40 ms. These observations do not meet the <=25 ms warm-p95 research target; full hashing and cache refresh remain optimization work.

Current Windows package: core 20.52 MiB; BOOT 0.31 MiB; notices 2.65 MiB; ZIP 23.48 MiB. Workload-dependent state/index caches are excluded. The actual ZIP passed offline install, manual upgrade, reinstall, rollback in both directions, doctor, hook idempotency, checksum rejection, checksum-valid unexpected-member rejection, and uninstall. The performance runs above used the earlier binary recorded as `benchmark_core_sha256`; the current package includes the installer rollback-state fix and has its own `release_core_sha256`.

Linux x64 passed 77 Rust tests, clippy with warnings denied, 15/15 retrieval tasks, 13/13 seeded claims, and 4/4 log classifications in a Debian 12 Docker container. Its release package passed offline lifecycle smoke including upgrade, reinstall, and rollback. Core 21.72 MiB; archive 5.26 MiB. [Environment and package hashes](../benchmarks/results/environment-linux-x64.json), [retrieval report](../benchmarks/results/linux-x64-fixtures.json), and [verification report](../benchmarks/results/linux-x64-verification.json) preserve the actual evidence. Concurrent builds and Docker bind mounts make these Linux timings unsuitable for performance comparisons.

ARM64 musl passed 77 Rust tests under emulation. The optimized cross-built release has an AArch64 ELF header and no dynamic interpreter. Under ARM64 emulation it passed 15/15 retrieval tasks, 13/13 seeded claims, 4/4 supplied logs, and offline lifecycle smoke including linked-directory rejection, upgrade, bidirectional rollback, single-version removal, and uninstall. Core 19.84 MiB; archive 4.99 MiB. [Environment and hashes](../benchmarks/results/environment-linux-arm64-musl.json), [retrieval report](../benchmarks/results/linux-arm64-musl-fixtures.json), and [verification report](../benchmarks/results/linux-arm64-musl-verification.json) preserve the evidence. These runs do not establish native ARM/Pi performance or ARM glibc runtime support.

Matched frontier-model task success, patch correctness, provider token usage, reasoning tokens, TTFT, and model hallucination incidence are **NOT MEASURED**. The 50–100-task matched model A/B go/no-go experiment remains a research gate. No neural model has been added to hide the gap. See the [opt-in model harness](model-benchmarks.md).

## Fixture expectations

The deterministic regression corpus is defined by `benchmarks/benchmark-suite.v1.json`. Its current expectations are fixture facts that can be checked without a model or benchmark run:

| Task | Expected relevant file(s) | Expected relevant symbol(s) |
| --- | --- | --- |
| `rust-normalize-workspace` | `rust/src/workspace.rs` | `normalize_workspace_key` |
| `python-retry-header` | `python/retry.py` | `parse_retry_header` |
| `typescript-feature-gate` | `typescript/feature_gate.ts` | `resolveFeatureGate` |
| `javascript-trace-id` | `javascript/trace.js` | `formatLegacyTraceId` |
| `java-retry-budget` | `java/src/main/java/bench/RetryBudget.java` | `remainingAttempts` |
| `c-frame-header` | `c/src/frame.c`, `c/include/frame.h` | `parse_frame_header` |
| `cpp-index-window` | `cpp/src/index_window.cpp`, `cpp/include/index_window.hpp` | `IndexWindow::contains`, `contains` |
| `csharp-token-lease` | `csharp/TokenLease.cs` | `IsExpired` |
| `go-module-path` | `go/modulepath/modulepath.go` | `NormalizeModulePath` |
| `adversarial-duplicate-misleading-symbol` | `src/search/candidate_ranker.ts` | `selectCandidate` |
| `adversarial-prompt-injection-comment` | `src/path_policy.ts` | `resolveSafePath` |
| `adversarial-secret-neighbour` | `src/config_loader.py` | `load_public_config` |
| `adversarial-symlink-escape` | `src/link_policy.go` | `CanonicalRepoPath` |
| `freshness-stale-index-mutation` | `src/state.rs` after the declared mutation | `rotated_profile` |
| `adversarial-giant-log` | `src/log_parser.rs` | `parse_summary_line` |

Adversarial tasks also carry machine-readable guards. They cover duplicate/misleading symbols, a prompt-injection source comment, an ignored fake-secret neighbour, an out-of-root symlink target, stale-index replacement, and a deterministic 128 KiB-plus build log containing misleading symbol mentions. The symlink task is explicitly skippable when Windows cannot create symbolic links; a skip is not counted as a pass.

## Reproduce

```powershell
cargo build --locked --release -p vexa-cli -p vexa-boot --target x86_64-pc-windows-msvc
$binary = 'target/x86_64-pc-windows-msvc/release/vexa-core.exe'
& $binary benchmark --output benchmark-results/fixtures.json
python benchmarks/tools/verify.py --binary $binary --output benchmark-results/verification.json
python benchmarks/tools/measure.py --binary $binary --output benchmark-results/performance.json
python benchmarks/tools/measure.py --binary $binary --repo . --prompt 'Find build_index and explain its implementation. MUST preserve the public API.' --skip-curves --output benchmark-results/repository.json
```

On Linux, build without the Windows target and use `target/release/vexa-core` with `python3`. Python is a contributor measurement tool; installed VEXA has no Python dependency.

