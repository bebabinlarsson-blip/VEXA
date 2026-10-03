# Privacy

VEXA is designed around a local deterministic indexing and retrieval path. This page describes what the current source code stores locally and when it performs network-visible actions.

## Repository data

Indexing reads repository files locally and writes VEXA-owned cache/index state under the current user's per-user cache/data location. The default indexer respects `.gitignore` and `.vexaignore` and excludes configured sensitive paths, generated content, and vendor content by default. Candidate paths are canonicalized beneath the repository root; out-of-root symlink targets are excluded from normal retrieval.

PACK output can contain excerpts from repository source. Use `vexa inspect-packet` to inspect the packet that VEXA would produce for a prompt before using it with an external model workflow.

Project-memory primitives are also local. `vexa memory inspect` reports the current memory summary and `vexa memory clear` removes the per-repository memory file. `vexa uninstall --indexes`, `--cache`, `--config`, or `--all` can remove VEXA-owned local state; those operations do not delete the source repository.

## Codex integration

The Codex `UserPromptSubmit` integration runs VEXA locally and returns selected context as `hookSpecificOutput.additionalContext`. Codex then makes that additional context model-visible for the active turn. Once supplied to Codex, that content is handled under the privacy/data policy of the user's Codex/ChatGPT workspace and model provider; VEXA cannot keep model-visible context local to the machine.

The hook does not replace or rewrite the user's original prompt. It receives the host-provided active model slug as metadata and does not hardcode a model.

Ordinary ChatGPT chats do not expose the same transparent local `UserPromptSubmit` hook. See [Integrations](integrations.md).

## Network activity

The deterministic pipeline does not require a VEXA cloud service. Explicit updates contact the official GitHub latest-release endpoint and selected release/checksum assets. Installed Codex hooks also start eligible automatic updates after emitting context, at most daily by default. Disable that network path with `vexa config set auto_update false`. No source, prompts, memory facts, or index data are sent to the updater. Installers contact GitHub unless local archive/checksum inputs are supplied.

No telemetry transport or analytics endpoint is implemented in the current Rust crates. The configuration includes a `telemetry` boolean that defaults to `false`, but changing that stored flag does not currently create a telemetry network path.

External model or host integrations can of course create their own network traffic outside VEXA's local deterministic path.

## Local configuration

The configuration file is per-user. Discover its exact path with:

```sh
vexa config path
```

The default configuration keeps `telemetry`, `semantic`, and `allow_workspace_execution` false. See [Configuration](configuration.md) for which settings are currently operational and which are only persisted for future lifecycle work.

## Sensitive repositories

Ignore and sensitive-path rules reduce accidental inclusion; they are not a substitute for reviewing context that is allowed to leave a sensitive environment. Repository filenames, symbols, source excerpts, hashes, and Git-derived metadata may appear in a generated packet when they are relevant and not excluded.

For the complete trust model and tested adversarial cases, see [Security model](security-model.md). For current integration and validation boundaries, see [Limitations](limitations.md).
