#!/usr/bin/env bash
set -euo pipefail

target="x86_64-unknown-linux-gnu"
output_dir="dist"
skip_build=0

usage() {
  cat <<'EOF'
Usage: scripts/package.sh [--target TARGET] [--output-dir DIR] [--skip-build]

Packages the stable BOOT launcher, vexa-core binary, and project license into a versioned .tar.gz.
The caller must build the requested target first when --skip-build is used.
EOF
}

while (($# > 0)); do
  case "$1" in
    --target)
      target="${2:?--target requires a value}"
      shift 2
      ;;
    --output-dir)
      output_dir="${2:?--output-dir requires a value}"
      shift 2
      ;;
    --skip-build)
      skip_build=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

case "$target" in
  x86_64-unknown-linux-gnu|aarch64-unknown-linux-musl) ;;
  *)
    echo "Unsupported Linux release target: $target" >&2
    exit 2
    ;;
esac

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

version="$(cargo metadata --no-deps --format-version 1 | sed -n 's/.*"name":"vexa-cli","version":"\([^"]*\)".*/\1/p' | head -n 1)"
if [[ -z "$version" ]]; then
  echo "Could not determine vexa-cli version from cargo metadata" >&2
  exit 1
fi

if [[ "$skip_build" -eq 0 ]]; then
  cargo build --locked --release --package vexa-boot --package vexa-cli --target "$target"
fi

target_root="${CARGO_TARGET_DIR:-target}"
boot_binary="$target_root/$target/release/vexa-boot"
core_binary="$target_root/$target/release/vexa-core"
for binary in "$boot_binary" "$core_binary"; do
  if [[ ! -x "$binary" ]]; then
    echo "Expected release binary was not found or executable: $binary" >&2
    exit 1
  fi
done

mkdir -p "$output_dir"
output_dir_abs="$(cd "$output_dir" && pwd)"
package_name="vexa-$version-$target"
stage_root="$output_dir_abs/.stage-$package_name"
stage_dir="$stage_root/$package_name"
archive="$output_dir_abs/$package_name.tar.gz"

rm -rf "$stage_root"
mkdir -p "$stage_dir" "$output_dir_abs"
trap 'rm -rf "$stage_root"' EXIT

install -m 0755 "$boot_binary" "$stage_dir/vexa-boot"
install -m 0755 "$core_binary" "$stage_dir/vexa-core"
install -m 0644 LICENSE "$stage_dir/LICENSE"
install -m 0644 THIRD_PARTY_LICENSES.txt "$stage_dir/THIRD_PARTY_LICENSES.txt"

source_date_epoch="${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct 2>/dev/null || printf '0')}"
if [[ ! "$source_date_epoch" =~ ^[0-9]+$ ]]; then
  echo "SOURCE_DATE_EPOCH must be an integer Unix timestamp" >&2
  exit 2
fi

tar \
  --sort=name \
  --mtime="@$source_date_epoch" \
  --owner=0 \
  --group=0 \
  --numeric-owner \
  -C "$stage_root" \
  -cf - "$package_name" | gzip -n > "$archive"

echo "$archive"

