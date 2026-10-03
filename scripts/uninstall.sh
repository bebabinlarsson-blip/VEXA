#!/bin/sh
set -eu

install_marker='VEXA per-user install v1'
version_arg=''
purge_user_data=0
keep_path=0
keep_codex=0

usage() {
    cat <<'EOF'
Usage: scripts/uninstall.sh [--version VERSION] [--purge-user-data] [--keep-path] [--keep-codex]

Without --version, removes all installer-managed VEXA binaries for the current user.
EOF
}

fail() {
    echo "vexa uninstaller: $*" >&2
    exit 1
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version) [ "$#" -ge 2 ] || fail '--version requires a value'; version_arg=$2; shift 2 ;;
        --purge-user-data) purge_user_data=1; shift ;;
        --keep-path) keep_path=1; shift ;;
        --keep-codex) keep_codex=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) fail "unknown argument '$1'" ;;
    esac
done

: "${HOME:?Unable to determine the current user home directory}"
data_home=${XDG_DATA_HOME:-"$HOME/.local/share"}
install_root=${VEXA_INSTALL_ROOT:-"$data_home/vexa"}
marker="$install_root/.install-marker"
versions_root="$install_root/versions"
launcher="$install_root/vexa"
case "$install_root" in ''|'/'|"$HOME") fail "unsafe install root '$install_root'" ;; esac
for checked_directory in "$install_root" "$versions_root" "$install_root/updates"; do
    [ ! -L "$checked_directory" ] || fail "refusing symbolic-link directory '$checked_directory'"
done

marked_install() {
    [ -f "$marker" ] && [ "$(sed -n '1p' "$marker")" = "$install_marker" ]
}
if [ -e "$install_root" ]; then
    marked_install || fail "refusing uninstall because '$install_root' is not marked as a VEXA installer directory"
    physical_install=$(CDPATH= cd -- "$install_root" && pwd -P)
    physical_home=$(CDPATH= cd -- "$HOME" && pwd -P)
    case "$physical_install" in '/'|"$physical_home") fail "unsafe install root '$physical_install'" ;; esac
fi

read_manifest_version() {
    file=$1
    [ -f "$file" ] || return 1
    schema=$(sed -n 's/.*"schema_version"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$file" | head -n 1)
    [ "$schema" = '1' ] || return 1
    value=$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$file" | head -n 1)
    case "$value" in ''|*[!0-9A-Za-z.+-]*) return 1 ;; esac
    printf '%s\n' "$value"
}

write_manifest() {
    manifest_version=$1
    printf '{"schema_version":1,"version":"%s"}' "$manifest_version" > "$install_root/current.json.tmp"
    mv -f "$install_root/current.json.tmp" "$install_root/current.json"
}

if [ -n "$version_arg" ]; then
    version=${version_arg#v}
    printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$' || fail "invalid version '$version_arg'"
    marked_install || fail "refusing version removal because '$install_root' is not marked as a VEXA installer directory"
    version_dir="$versions_root/$version"
    [ ! -L "$version_dir" ] || fail 'installed version must not be a symbolic link'
    [ -d "$version_dir" ] || fail "VEXA version $version is not installed"
    current=$(read_manifest_version "$install_root/current.json" 2>/dev/null || true)
    previous=$(read_manifest_version "$install_root/previous.json" 2>/dev/null || true)
    if [ "$current" = "$version" ]; then
        previous=$(read_manifest_version "$install_root/previous.json" 2>/dev/null || true)
        if [ -n "$previous" ] && [ "$previous" != "$version" ] && [ -d "$versions_root/$previous" ]; then
            "$launcher" rollback --json >/dev/null || fail 'verified rollback failed; refusing version removal'
            rm -f "$install_root/previous.json"
            echo "Activated previous VEXA version $previous."
        else
            if [ "$keep_codex" -eq 0 ]; then
                "$launcher" hook codex-unregister || fail 'Codex unregister failed; retaining the last installed version'
            fi
            rm -f "$install_root/current.json" "$install_root/previous.json" "$launcher"
        fi
    fi
    if [ "$current" = "$version" ] || [ "$previous" = "$version" ]; then
        rm -f "$install_root/previous.json" "$install_root/updates/state.json" "$install_root/updates/state.json.bak"
    fi
    rm -rf "$version_dir"
    echo "Removed VEXA version $version."
    exit 0
fi

if [ "$keep_codex" -eq 0 ]; then
    if [ -x "$launcher" ]; then
        "$launcher" hook codex-unregister || fail 'Codex unregister failed; retaining binaries for retry'
    fi
fi

if [ "$keep_path" -eq 0 ]; then
    for profile in "$HOME/.bashrc" "$HOME/.zshrc" "$HOME/.profile"; do
        [ -f "$profile" ] || continue
        if grep -F '# >>> vexa installer >>>' "$profile" >/dev/null 2>&1; then
            tmp="$profile.vexa-$$.tmp"
            awk '
                $0 == "# >>> vexa installer >>>" { skip=1; next }
                $0 == "# <<< vexa installer <<<" { skip=0; next }
                !skip { print }
            ' "$profile" > "$tmp"
            mv -f "$tmp" "$profile"
        fi
    done
fi

if [ -e "$install_root" ]; then
    marked_install || fail "refusing recursive removal because '$install_root' is not marked as a VEXA installer directory"
    case "$install_root" in ''|'/'|"$HOME") fail "unsafe install root '$install_root'" ;; esac
    rm -rf "$install_root"
fi

if [ "$purge_user_data" -eq 1 ]; then
    config_home=${XDG_CONFIG_HOME:-"$HOME/.config"}
    state_home=${XDG_STATE_HOME:-"$HOME/.local/state"}
    cache_home=${XDG_CACHE_HOME:-"$HOME/.cache"}
    for user_data in "$config_home/vexa" "$state_home/vexa" "$cache_home/vexa"; do
        [ ! -L "$user_data" ] || fail "refusing symbolic-link user-data directory '$user_data'"
    done
    rm -rf "$config_home/vexa" "$state_home/vexa" "$cache_home/vexa"
fi

echo 'VEXA binaries were uninstalled for the current user.'
if [ "$purge_user_data" -eq 0 ]; then
    echo 'User configuration/state was preserved. Use --purge-user-data to remove it.'
fi
