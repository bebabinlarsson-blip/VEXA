# Integrations

## OpenAI Codex

VEXA integrates with the current Codex lifecycle-hook interface through the `UserPromptSubmit` event. Codex invokes the configured command with one JSON object on standard input immediately before a user prompt is sent to the model. The event includes the submitted `prompt` plus common Codex fields such as `session_id`, `turn_id`, `cwd`, `model`, `permission_mode`, and `transcript_path`.

The VEXA hook command is:

```text
vexa hook codex-user-prompt-submit
```

VEXA may return hook-specific `additionalContext`. Codex adds that text as developer context for the turn. The structured success shape is:

```json
{
  "hookSpecificOutput": {
    "hookEventName": "UserPromptSubmit",
    "additionalContext": "Relevant context selected by VEXA."
  }
}
```

Plain text on standard output is also treated as additional developer context by Codex. A hook can block submission with `{"decision":"block","reason":"..."}` or exit code `2` plus a reason on standard error, but VEXA's context adapter is intended to enrich normal turns rather than use blocking as its default behavior. This integration does **not** replace, rewrite, or mutate the user's submitted prompt; `UserPromptSubmit` exposes additive context and blocking behavior, not a supported prompt-replacement field.

A minimal supported hook definition is available at [`examples/codex-hooks.json`](../examples/codex-hooks.json):

```json
{
  "hooks": {
    "UserPromptSubmit": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "vexa hook codex-user-prompt-submit",
            "statusMessage": "Loading VEXA context",
            "timeout": 30,
            "additionalContextLimit": 2500
          }
        ]
      }
    ]
  }
}
```

Codex loads per-user hooks from `~/.codex/hooks.json` (or from the user Codex home when `CODEX_HOME` is set). The registration scripts merge this handler into that file instead of replacing the file or any existing event, matcher group, or handler:

```powershell
./scripts/register-codex.ps1
```

```sh
./scripts/register-codex.sh
```

The registration scripts call `vexa hook codex-register`; removal calls `vexa hook codex-unregister`. These native commands use structural JSON merging and do not require `jq`, Python, or Node.js. Registration is per-user and idempotent, serialized with a local file lock and atomically written. It refuses to rewrite a file whose relevant hook structure cannot be parsed safely.

After adding or changing a non-managed hook, Codex requires an explicit trust review of the exact hook definition. Start Codex, run `/hooks`, inspect the VEXA `UserPromptSubmit` entry, and trust it there. Trust is recorded against the hook definition's current hash, so later changes can require review again. The registration scripts do not bypass or pre-approve this trust step.

`additionalContextLimit` controls how much hook-provided context Codex passes directly to the model before spilling oversized output to a temporary file and providing a shorter preview. VEXA's example sets the current default-sized limit of `2500` tokens explicitly. Keep injected context compact; context from multiple hooks is cumulative. Codex hooks can also be disabled globally with the Codex `features.hooks` setting, and managed enterprise policy can restrict execution to managed hooks only.

To remove only VEXA's handler while preserving all unrelated user hooks:

```powershell
./scripts/unregister-codex.ps1
```

```sh
./scripts/unregister-codex.sh
```

Unregistration retains the hook file and removes only VEXA's handler; other top-level metadata and hooks remain in place.

This lifecycle hook targets the Codex runtime, including Codex-capable Work surfaces. An ordinary ChatGPT conversation does not provide the same transparent global `UserPromptSubmit` hook. ChatGPT plugin/MCP integrations can expose tools, resources, instructions, and model-visible tool results, but tool invocation there is a separate mechanism and should not be described as automatic prompt replacement or guaranteed per-message interception.

Current OpenAI references:

- [Hooks — ChatGPT Learn](https://learn.chatgpt.com/docs/hooks)
- [Package your plugin — OpenAI Developers](https://developers.openai.com/plugins/build/plugins)
- [MCP server concepts — OpenAI Developers](https://developers.openai.com/plugins/concepts/mcp-server)
