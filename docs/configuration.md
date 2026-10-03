# Configuration

VEXA stores per-user configuration as JSON. Use the CLI to discover the actual platform-specific path:

```sh
vexa config path
```

If the file does not exist, VEXA uses built-in defaults. `vexa config set` validates the new value before saving it, and `vexa config reset` writes the default configuration.

```sh
vexa config show
vexa config set profile standard
vexa config set max_context_tokens 2200
vexa config reset
```

The current default object matches [`../examples/vexa-config.json`](../examples/vexa-config.json):

```json
{
  "profile": "auto",
  "max_context_tokens": 1800,
  "warm_timeout_seconds": 600,
  "auto_update": true,
  "update_channel": "stable",
  "telemetry": false,
  "semantic": false,
  "allow_workspace_execution": false
}
```

## Current keys

| Key | Accepted values | Default | Current runtime effect |
| --- | --- | --- | --- |
| `profile` | `auto`, `lite`, `standard`, `enhanced` | `auto` | Active. Controls the operating profile used when `prepare --mode auto` is selected. Auto profile detection currently resolves to lite/standard/enhanced from local hardware. |
| `max_context_tokens` | integer `128..=1000000` | `1800` | Active. Default context budget for `prepare`; `--max-context-tokens` overrides it for one request. The Codex hook clamps its prompt-time budget to `128..=2200`. |
| `warm_timeout_seconds` | integer `0..=86400` | `600` | Active. JSONL protocol exits after this many idle seconds, with a one-second floor for zero. The one-shot Codex hook starts a process per prompt; it does not share this warm process. |
| `auto_update` | boolean | `true` | Active for an installed Codex prompt hook. After emitting context, launches a detached update command when its local daily eligibility marker permits. No permanent service. Set false for explicit updates only. |
| `update_channel` | `stable`, `beta`, `dev` | `stable` | Persisted and validated, but the current updater does not select releases by this field; it queries the official latest-release endpoint. |
| `telemetry` | boolean | `false` | Persisted and shown by `vexa status`. No telemetry transport or analytics endpoint exists in the current Rust crates. |
| `semantic` | boolean | `false` | Persisted and shown by `vexa status`. No semantic model/reranker is shipped or invoked; current SEEK semantic scores are zero. |
| `allow_workspace_execution` | boolean | `false` | Persisted and validated, but current indexing/retrieval does not execute repository code and no runtime path consumes this flag. |

Boolean values accepted by `vexa config set` are `true/false`, `1/0`, `yes/no`, and `on/off`, case-insensitively.

## Request-level overrides

`vexa prepare` and `vexa inspect-packet` accept request-scoped settings independently of the saved configuration:

```sh
vexa prepare --cwd . \
  --prompt "Find the parser implementation" \
  --mode lite \
  --max-context-tokens 1200
```

Supported `--mode` values are `auto`, `lite`, `standard`, and `enhanced`. `--model` records host model metadata in the request; VEXA does not require or hardcode a particular model slug.

Diagnostics can be supplied repeatedly:

```sh
vexa prepare --cwd . --prompt "Fix the failing parser" --diagnostic "error: unexpected token"
```

## Codex hook interaction

The Codex hook loads the same configuration. It uses `profile` and derives its context budget from `max_context_tokens`, clamped to a maximum of 2200 tokens and a minimum of 128. The active Codex model slug is received from the hook input rather than selected by VEXA.

See [Integrations](integrations.md) for hook registration and trust behavior and [Limitations](limitations.md) for the additive-context boundary.

## Isolated roots and expansion

`VEXA_INSTALL_ROOT`, `VEXA_CONFIG_ROOT`, and `VEXA_CACHE_ROOT` override their respective local data directories. `CODEX_HOME` selects the host hook configuration directory. Empty VEXA root overrides are rejected. These are useful for portable installs and isolated smoke tests.

`prepare --include-path src/file.rs` and protocol `include_paths` request additional indexed files. Excluded, sensitive, missing, and out-of-root paths remain rejected. Responses expose `expansion_candidates`; omitted evidence sets `needs_expansion`. Budgets and token totals are UTF-8 byte-count estimates divided by four, not provider tokenizer counts.
