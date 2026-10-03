#!/bin/sh
set -eu

if [ -n "${VEXA_LAUNCHER:-}" ]; then
    launcher=$VEXA_LAUNCHER
elif command -v vexa >/dev/null 2>&1; then
    launcher=$(command -v vexa)
else
    echo "The 'vexa' executable is not on PATH. No Codex configuration was changed." >&2
    exit 1
fi

exec "$launcher" hook codex-register
