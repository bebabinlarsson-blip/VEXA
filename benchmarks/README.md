# VEXA deterministic benchmark corpus

This directory contains regression fixtures and machine-readable expectations for `vexa benchmark`. Task contracts contain expectations, not measured outcomes. Observed runs are preserved under `results/`; see [benchmark results](../docs/benchmark-results.md) for their scope and limitations.

`benchmark-suite.v1.json` is the entry point. Every task listed by the suite conforms to `schema/benchmark-task.schema.json` and names a fixture repository, a prompt, expected relevant files, and expected relevant symbols. Adversarial tasks may also declare guards such as paths that must stay out of a context packet.

The fixture repositories are copied before each task so mutation scenarios cannot contaminate later runs. Normal tasks begin from a fresh index. The stale-index task explicitly indexes first and then replaces one file from `mutations/` before retrieval. The symlink task materializes an escape link only for that task; platforms that cannot create a symlink may report the task as skipped rather than failed.

Run `./benchmarks/tools/validate.ps1` from the repository root to parse JSON and verify fixture/task references. This validator does not invoke VEXA or measure results. Run `cargo run --locked --package vexa-cli -- benchmark --output benchmark-results/regression.json` for actual retrieval and guard verification. Python measurement and opt-in provider adapter tools are described in [benchmark results](../docs/benchmark-results.md) and [model benchmarks](../docs/model-benchmarks.md).

