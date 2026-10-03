# Opt-in model A/B benchmark

`benchmarks/tools/model_ab.py` runs a provider-neutral matched comparison over
the suite's `single_query` tasks. It skips the mutation and symlink scenarios.
The suite contains 15 deterministic retrieval fixtures, not a collection of
50–100 coding tasks. This runner does not measure coding success or patch
correctness; both are recorded as `NOT MEASURED`.

No model provider is configured or contacted automatically. To run an
experiment, supply your own adapter as a trusted argv JSON array. The adapter
receives one JSON request on stdin:

```json
{
  "task_id": "python-retry-header",
  "arm": "baseline",
  "model": "provider/model-id",
  "settings": {"temperature": 0},
  "prompt": "...",
  "context": "..."
}
```

It must return one JSON object with `model`, `settings`, `answer`, and
`claims`; it may also return `correction_packet`, `usage`, and `ttft_ms`.
Claims are objects accepted by `vexa-core verify claims` (`file_exists`,
`file_hash`, `symbol_exists`, or `signature_exists`). Optional `category`
groups verification totals; otherwise claim kind is the category. The runner
rejects a returned model or settings mismatch. It records measured adapter
round-trip latency and supplied usage as separate evidence. Missing usage and
TTFT remain `null`; no token or TTFT values are inferred.

Example (PowerShell):

```powershell
$adapter = @((Get-Command python).Source, 'my_adapter.py') | ConvertTo-Json -Compress
python benchmarks/tools/model_ab.py `
  --adapter-argv $adapter `
  --model "provider/model-id" `
  --settings '{"temperature":0}' `
  --binary target/x86_64-pc-windows-msvc/release/vexa-core.exe `
  --output artifacts/model-ab.json `
  --allow-send-fixtures
```

`--allow-send-fixtures` is required because source fixtures are sent to the
adapter. Before staging, the runner rejects symlinks, skips sensitive paths
before reading them, rejects common secret-like content, and bounds file sizes.
For each task it refreshes a fresh isolated VEXA index, validates manifest paths
and source hashes, and uses the eligible source from that manifest for both
arms. Each arm gets an isolated repository, empty `.git`, cache, and config
roots, with ignore control files retained. The baseline receives the full
eligible source in its context. The VEXA arm receives `vexa-core prepare
--json`. That packet embeds the task prompt, and the adapter request also
includes it separately, so VEXA receives a duplicated prompt in this harness.
Neither arm receives expected answer metadata. Arm order is randomized from
`--seed`.

The unsupported structured-claim rate is `(contradicted + unknown) / checkable
claims`. Verified, contradicted, and unknown totals remain separate per
category. Unknown means the verifier could not establish truth; contradicted
means verifier evidence conflicts with the claim. An empty claim set has a
`null` rate. This rate does not identify hallucinations and measures only the
adapter's checkable structured claims, not all statements in its prose answer.
Model identity is adapter-supplied and self-reported. The runner checks it
matches the requested label but cannot authenticate the provider. Results are
measurements only when produced by a real user-supplied adapter. The
included unittest adapter doubles validate the contract and are not benchmark
results. Future user-supplied tasks should provide a safe fixture repository,
a stable task ID and prompt, and claims that can be checked by the existing
VEXA claims verifier; the runner currently reads only the checked-in suite.
