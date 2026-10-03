#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
    echo "Usage: scripts/smoke-package.sh ARCHIVE.tar.gz" >&2
    exit 2
fi
repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
archive=$(CDPATH= cd -- "$(dirname -- "$1")" && pwd)/$(basename -- "$1")
[ -f "$archive" ] || { echo "Package archive not found: $archive" >&2; exit 1; }
name=$(basename -- "$archive" .tar.gz)
case "$name" in vexa-*-x86_64-unknown-linux-gnu|vexa-*-aarch64-unknown-linux-musl) ;; *) echo "Unexpected VEXA package name: $name" >&2; exit 1 ;; esac
version=$(printf '%s\n' "$name" | sed -E 's/^vexa-(.*)-(x86_64-unknown-linux-gnu|aarch64-unknown-linux-musl)$/\1/')
temp=$(mktemp -d "${TMPDIR:-/tmp}/vexa-package-smoke.XXXXXX")
trap 'rm -rf "$temp"' EXIT HUP INT TERM

home="$temp/home"
install_root="$temp/local/share/vexa"
codex_home="$temp/codex"
config_home="$temp/config"
config_root="$config_home/vexa"
archive_name=$(basename -- "$archive")
checksums="$temp/SHA256SUMS"
bad_checksums="$temp/BAD-SHA256SUMS"
mkdir -p "$home" "$codex_home" "$config_home/vexa"
hash=$(sha256sum "$archive" | awk '{print tolower($1)}')
printf '%s  %s\n' "$hash" "$archive_name" > "$checksums"
printf '%064d  %s\n' 0 "$archive_name" > "$bad_checksums"
cat > "$codex_home/hooks.json" <<'EOF'
{"metadata":{"keep":true},"hooks":{"UserPromptSubmit":[{"matcher":"*","hooks":[{"type":"command","command":"other-hook"}]}]}}
EOF
printf '{"telemetry":false}\n' > "$config_root/config.json"

export HOME="$home" XDG_DATA_HOME="$temp/local/share" XDG_CONFIG_HOME="$config_home" XDG_STATE_HOME="$temp/state" XDG_CACHE_HOME="$temp/cache" CODEX_HOME="$codex_home" VEXA_INSTALL_ROOT="$install_root" VEXA_CONFIG_ROOT="$config_root" VEXA_CACHE_ROOT="$temp/cache/vexa"
install_script="$repo_root/install.sh"
uninstall_script="$repo_root/scripts/uninstall.sh"

mkdir -p "$temp/unsafe-install" "$temp/foreign-directory"
printf '%s\n' 'VEXA per-user install v1' > "$temp/unsafe-install/.install-marker"
printf '%s\n' 'preserve foreign contents' > "$temp/foreign-directory/keep.txt"
ln -s "$temp/foreign-directory" "$temp/unsafe-install/versions"
if VEXA_INSTALL_ROOT="$temp/unsafe-install" sh "$install_script" --version "$version" --archive "$archive" --checksums "$checksums" --no-codex --no-modify-path; then
    echo 'Installer accepted a versions symlink' >&2; exit 1
fi
if VEXA_INSTALL_ROOT="$temp/unsafe-install" sh "$uninstall_script" --keep-path --keep-codex; then
    echo 'Uninstaller accepted a versions symlink' >&2; exit 1
fi
[ "$(cat "$temp/foreign-directory/keep.txt")" = 'preserve foreign contents' ] || { echo 'Symlink target contents changed' >&2; exit 1; }

old_version='0.0.0-smoke-old'
target=$(printf '%s\n' "$name" | sed -E 's/^vexa-.*-(x86_64-unknown-linux-gnu|aarch64-unknown-linux-musl)$/\1/')
old_name="vexa-$old_version-$target"
mkdir -p "$temp/old-package"
tar -xzf "$archive" -C "$temp/old-package"
mv "$temp/old-package/$name" "$temp/old-package/$old_name"
old_archive="$temp/$old_name.tar.gz"
tar -czf "$old_archive" -C "$temp/old-package" "$old_name"
old_hash=$(sha256sum "$old_archive" | awk '{print tolower($1)}')
printf '%s  %s\n' "$old_hash" "$(basename -- "$old_archive")" > "$temp/OLD-SHA256SUMS"
sh "$install_script" --version "$old_version" --archive "$old_archive" --checksums "$temp/OLD-SHA256SUMS" --no-codex
sh "$install_script" --version "$version" --archive "$archive" --checksums "$checksums"
[ -x "$install_root/vexa" ] || { echo 'Installer did not install the BOOT launcher' >&2; exit 1; }
"$install_root/vexa" doctor --json > "$temp/doctor.json"
grep -Eq '"platform_supported"[[:space:]]*:[[:space:]]*true' "$temp/doctor.json"
grep -q 'vexa hook codex-user-prompt-submit' "$codex_home/hooks.json"
grep -q 'other-hook' "$codex_home/hooks.json"
current_before_bad_install=$(cat "$install_root/current.json")
launcher_hash_before_bad_install=$(sha256sum "$install_root/vexa" | awk '{print tolower($1)}')
if sh "$install_script" --version "$version" --archive "$archive" --checksums "$bad_checksums" --no-codex; then
    echo 'Installer accepted a corrupt checksum manifest' >&2
    exit 1
fi
[ "$(cat "$install_root/current.json")" = "$current_before_bad_install" ] || { echo 'Failed checksum changed the active installation' >&2; exit 1; }
[ "$(sha256sum "$install_root/vexa" | awk '{print tolower($1)}')" = "$launcher_hash_before_bad_install" ] || { echo 'Failed checksum changed the active BOOT launcher' >&2; exit 1; }
"$install_root/vexa" doctor --json > "$temp/doctor-after-rejected-package.json"
grep -Eq '"platform_supported"[[:space:]]*:[[:space:]]*true' "$temp/doctor-after-rejected-package.json"

sh "$install_script" --version "$version" --archive "$archive" --checksums "$checksums"
"$install_root/vexa" hook codex-register >/dev/null
hook_count=$(grep -o 'vexa hook codex-user-prompt-submit' "$codex_home/hooks.json" | wc -l | tr -d ' ')
[ "$hook_count" = 1 ] || { echo 'Reinstall duplicated the VEXA Codex handler' >&2; exit 1; }
"$install_root/vexa" rollback --json > "$temp/rollback-old.json"
grep -Eq "\"version\"[[:space:]]*:[[:space:]]*\"$old_version\"" "$install_root/current.json"
"$install_root/vexa" rollback --json > "$temp/rollback-new.json"
grep -Eq "\"version\"[[:space:]]*:[[:space:]]*\"$version\"" "$install_root/current.json"
sh "$uninstall_script" --version "$version" --keep-path
grep -Eq "\"version\"[[:space:]]*:[[:space:]]*\"$old_version\"" "$install_root/current.json"
[ ! -e "$install_root/versions/$version" ] || { echo 'Version removal retained the selected directory' >&2; exit 1; }
"$install_root/vexa" doctor --json > "$temp/doctor-after-version-removal.json"
sh "$install_script" --version "$version" --archive "$archive" --checksums "$checksums"
sh "$uninstall_script"
[ ! -e "$install_root" ] || { echo 'Uninstaller left the managed install root behind' >&2; exit 1; }
[ -f "$config_root/config.json" ] || { echo 'Uninstaller removed user configuration without purge option' >&2; exit 1; }
! grep -q 'vexa hook codex-user-prompt-submit' "$codex_home/hooks.json"
grep -q 'other-hook' "$codex_home/hooks.json"
echo "Offline Linux installer lifecycle smoke passed for $name."
