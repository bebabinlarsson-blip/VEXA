#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
temp=$(mktemp "${TMPDIR:-/tmp}/vexa-action-pins.XXXXXX")
trap 'rm -f "$temp"' EXIT HUP INT TERM

command -v curl >/dev/null 2>&1 || { echo 'curl is required to validate pinned GitHub Actions' >&2; exit 1; }
grep -hE '^[[:space:]]*uses:[[:space:]]*[^[:space:]#]+' "$repo_root"/.github/workflows/*.yml \
    | sed -E 's/^[[:space:]]*uses:[[:space:]]*([^[:space:]#]+).*/\1/' \
    | sort -u > "$temp"

count=0
while IFS= read -r action_ref; do
    action=${action_ref%@*}
    ref=${action_ref##*@}
    if [ "$action" = "$action_ref" ] || [ "${#ref}" -ne 40 ] || ! printf '%s\n' "$ref" | grep -Eq '^[0-9a-fA-F]{40}$'; then
        echo "GitHub Action is not pinned to a full 40-character commit SHA: $action_ref" >&2
        exit 1
    fi
    case "$action" in */*)
        owner=${action%%/*}
        remainder=${action#*/}
        repository="$owner/${remainder%%/*}"
        ;;
        *) echo "Invalid GitHub Action reference '$action_ref'" >&2; exit 1 ;;
    esac
    response=$(curl -fsSL --retry 3 --retry-delay 1 \
        -H 'Accept: application/vnd.github+json' \
        -H 'User-Agent: vexa-action-pin-check/1' \
        "https://api.github.com/repos/$repository/commits/$ref") || {
        echo "GitHub API could not resolve $repository@$ref" >&2
        exit 1
    }
    if ! printf '%s\n' "$response" | grep -Eiq '"sha"[[:space:]]*:[[:space:]]*"'"$ref"'"'; then
        echo "GitHub API response did not confirm exact commit $repository@$ref" >&2
        exit 1
    fi
    count=$((count + 1))
    echo "Resolved $repository@$ref"
done < "$temp"

[ "$count" -gt 0 ] || { echo 'No GitHub Actions were found to validate' >&2; exit 1; }
echo "Validated $count unique pinned GitHub Actions."
