# Benchmark methodology

VEXA reports targets separately from measurements. No target is published as achieved until a reproducible benchmark run produces the number.

The deterministic corpus measures:

- relevant-file recall: retrieved required files / expected required files;
- relevant-symbol recall: retrieved required symbols / expected required symbols;
- packet reduction: `1 - estimated_packet_tokens / estimated_source_tokens`;
- preparation latency with p50 and p95 wall-clock values;
- checkable repository-claim contradictions in verification fixtures;
- index freshness after edits and renames;
- adversarial behavior for secrets, symlinks, prompt injection, misleading names, duplicate symbols, malformed code, and giant logs.

Compression curves use budgets representing roughly 100%, 75%, 50%, 30%, 20%, 10%, and 5% of the candidate source. The useful operating point is the lowest budget before retrieval/task-quality metrics materially degrade. VEXA does not optimize a compression percentage independently of task success.

Frontier-model A/B results require the same model, provider settings, task, and scoring harness on control and experiment runs. The repository includes deterministic local benchmarks that do not need model credentials; model-backed task-success experiments are a separate opt-in layer so missing API credentials never becomes a reason to fabricate results.

