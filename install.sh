#!/bin/sh
set -eu

repository='bebabinlarsson-blip/VEXA'
api_base="https://api.github.com/repos/$repository"
user_agent='vexa-installer/1'
install_marker='VEXA per-user install v1'
version_arg=''
local_archive=''
local_checksums=''
no_modify_path=0
no_codex=0

usage() {
    cat <<'EOF'
Usage: ./install.sh [--version VERSION] [--archive FILE --checksums FILE] [--no-modify-path] [--no-codex]

Installs an official VEXA GitHub release for the current user.
EOF
}

fail() {
    echo "vexa installer: $*" >&2
    exit 1
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version)
            [ "$#" -ge 2 ] || fail '--version requires a value'
            version_arg=$2
            shift 2
            ;;
        --archive)
            [ "$#" -ge 2 ] || fail '--archive requires a value'
            local_archive=$2
            shift 2
            ;;
        --checksums)
            [ "$#" -ge 2 ] || fail '--checksums requires a value'
            local_checksums=$2
            shift 2
            ;;
        --no-modify-path)
            no_modify_path=1
            shift
            ;;
        --no-codex)
            no_codex=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            fail "unknown argument '$1'"
            ;;
    esac
done

command -v tar >/dev/null 2>&1 || fail "'tar' is required to extract the verified release archive"
command -v gzip >/dev/null 2>&1 || fail "'gzip' is required to inspect the verified release archive"
command -v mktemp >/dev/null 2>&1 || fail "'mktemp' is required"
if [ -z "$local_archive" ] && [ -z "$local_checksums" ]; then
    if command -v curl >/dev/null 2>&1; then
        downloader='curl'
    elif command -v wget >/dev/null 2>&1; then
        downloader='wget'
    else
        fail "'curl' or 'wget' is required to download GitHub release files"
    fi
fi

if command -v sha256sum >/dev/null 2>&1; then
    sha_tool='sha256sum'
elif command -v shasum >/dev/null 2>&1; then
    sha_tool='shasum'
else
    fail "no SHA-256 utility found; install 'sha256sum' (coreutils) or 'shasum' before installing VEXA"
fi

fetch() {
    url=$1
    destination=$2
    case "$url" in
        https://api.github.com/repos/bebabinlarsson-blip/VEXA/*|https://github.com/bebabinlarsson-blip/VEXA/releases/download/*) ;;
        *) fail "refusing non-official download URL '$url'" ;;
    esac
    if [ "$downloader" = 'curl' ]; then
        curl -fsSL --proto '=https' --tlsv1.2 -A "$user_agent" "$url" -o "$destination"
    else
        wget -q --https-only --user-agent="$user_agent" -O "$destination" "$url"
    fi
}

sha_file() {
    if [ "$sha_tool" = 'sha256sum' ]; then
        sha256sum "$1" | awk '{print tolower($1)}'
    else
        shasum -a 256 "$1" | awk '{print tolower($1)}'
    fi
}

case "$(uname -s 2>/dev/null || true)" in
    Linux) ;;
    *) fail 'this installer supports Linux; use install.ps1 on Windows' ;;
esac

arch=$(uname -m 2>/dev/null || true)
case "$arch" in
    x86_64|amd64) arch='x86_64' ;;
    aarch64|arm64) arch='aarch64' ;;
    *) fail "unsupported Linux architecture '$arch'" ;;
esac

libc='unknown'
if command -v getconf >/dev/null 2>&1 && getconf GNU_LIBC_VERSION >/dev/null 2>&1; then
    libc='glibc'
elif command -v ldd >/dev/null 2>&1; then
    ldd_text=$(ldd --version 2>&1 || true)
    case "$ldd_text" in
        *musl*|*MUSL*) libc='musl' ;;
        *GLIBC*|*glibc*|*GNU\ libc*) libc='glibc' ;;
    esac
fi

case "$arch:$libc" in
    x86_64:glibc) target='x86_64-unknown-linux-gnu' ;;
    aarch64:glibc|aarch64:musl) target='aarch64-unknown-linux-musl' ;;
    x86_64:musl) fail 'x86_64 musl was detected, but this VEXA release set does not publish an x86_64 musl artifact' ;;
    *) fail "unable to determine a supported Linux libc for $arch; detected '$libc'" ;;
esac

: "${HOME:?Unable to determine the current user home directory}"
data_home=${XDG_DATA_HOME:-"$HOME/.local/share"}
install_root=${VEXA_INSTALL_ROOT:-"$data_home/vexa"}
case "$install_root" in
    ''|'/'|"$HOME") fail "unsafe install root '$install_root'" ;;
esac
[ ! -L "$install_root" ] || fail 'install root must not be a symbolic link'
[ ! -L "$install_root/versions" ] || fail 'versions directory must not be a symbolic link'
[ ! -L "$install_root/updates" ] || fail 'updates directory must not be a symbolic link'
if [ -e "$install_root" ]; then
    physical_install=$(CDPATH= cd -- "$install_root" && pwd -P)
    physical_home=$(CDPATH= cd -- "$HOME" && pwd -P)
    case "$physical_install" in '/'|"$physical_home") fail "unsafe install root '$physical_install'" ;; esac
    if [ -f "$install_root/.install-marker" ]; then
        [ "$(sed -n '1p' "$install_root/.install-marker")" = "$install_marker" ] || fail "install marker in '$install_root' is not recognized"
    elif [ -n "$(ls -A "$install_root" 2>/dev/null || true)" ]; then
        fail "refusing to claim non-empty unmarked install root '$install_root'"
    fi
fi

temp_root=$(mktemp -d "${TMPDIR:-/tmp}/vexa-install.XXXXXX")
cleanup() {
    rm -rf "$temp_root"
}
trap cleanup EXIT HUP INT TERM

metadata="$temp_root/release.json"
if [ -n "$local_archive" ] || [ -n "$local_checksums" ]; then
    [ -n "$local_archive" ] && [ -n "$local_checksums" ] || fail '--archive and --checksums must be provided together'
    [ -n "$version_arg" ] || fail '--version is required with a local package'
    case "$version_arg" in v*) release_tag=$version_arg ;; *) release_tag="v$version_arg" ;; esac
    printf '%s\n' "$release_tag" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$' || fail "invalid version '$version_arg'"
    resolved_tag=$release_tag
elif [ -n "$version_arg" ]; then
    case "$version_arg" in v*) release_tag=$version_arg ;; *) release_tag="v$version_arg" ;; esac
    printf '%s\n' "$release_tag" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$' || fail "invalid VEXA version '$version_arg'"
    fetch "$api_base/releases/tags/$release_tag" "$metadata"
else
    fetch "$api_base/releases/latest" "$metadata"
    release_tag=$(sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$metadata" | head -n 1)
fi

if [ -z "$local_archive" ]; then
    [ -n "${release_tag:-}" ] || fail 'official GitHub release metadata did not contain a tag_name'
    resolved_tag=$(sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$metadata" | head -n 1)
    [ "$resolved_tag" = "$release_tag" ] || fail "release metadata tag '$resolved_tag' does not match requested tag '$release_tag'"
    grep -Eq '"draft"[[:space:]]*:[[:space:]]*false' "$metadata" || fail 'refusing a draft or malformed GitHub release'
fi
version=${release_tag#v}
asset="vexa-$version-$target.tar.gz"

checksums="$temp_root/SHA256SUMS"
archive="$temp_root/$asset"
release_base="https://github.com/$repository/releases/download/$release_tag"
if [ -n "$local_archive" ]; then
    [ "$(basename -- "$local_archive")" = "$asset" ] || fail "local package must be named '$asset'"
    cp -- "$local_checksums" "$checksums"
else
    fetch "$release_base/SHA256SUMS" "$checksums"
fi

checksum_result=$(awk -v wanted="$asset" '
    length($1) == 64 && $1 ~ /^[0-9A-Fa-f]+$/ {
        name=$2
        sub(/^\*/, "", name)
        if (name == wanted) {
            count++
            hash=tolower($1)
        }
    }
    END {
        if (count == 1) print hash
        else exit 3
    }
' "$checksums") || fail "SHA256SUMS must contain exactly one checksum for '$asset'"

if [ -n "$local_archive" ]; then cp -- "$local_archive" "$archive"; else fetch "$release_base/$asset" "$archive"; fi
actual=$(sha_file "$archive")
[ "$actual" = "$checksum_result" ] || fail "SHA-256 mismatch for '$asset'; expected $checksum_result, got $actual. The archive was not extracted."
echo "Verified SHA-256 for '$asset'."

package_root="vexa-$version-$target"
archive_limit=$((512 * 1024 * 1024))
compressed_size=$(wc -c < "$archive" | tr -d ' ')
[ "$compressed_size" -le "$archive_limit" ] || fail 'verified archive exceeds the 512 MiB compressed size limit'
unpacked_tar="$temp_root/archive.tar"
gzip -dc "$archive" | head -c $((archive_limit + 1)) > "$unpacked_tar" || true
unpacked_size=$(wc -c < "$unpacked_tar" | tr -d ' ')
[ "$unpacked_size" -le "$archive_limit" ] || fail 'verified archive exceeds the 512 MiB uncompressed size limit'
listing="$temp_root/archive.list"
tar --quoting-style=escape -tvf "$unpacked_tar" > "$listing" || fail 'verified archive has invalid tar structure or requires GNU tar with escape quoting support'
awk -v root="$package_root" -v max="$archive_limit" '
    BEGIN { expected["vexa-boot"]=1; expected["vexa-core"]=1; expected["LICENSE"]=1; expected["THIRD_PARTY_LICENSES.txt"]=1 }
    {
        type=substr($0, 1, 1)
        name=$NF
        if (type == "d" && (name == root || name == root "/")) { roots++; next }
        if (type != "-" || index(name, root "/") != 1) { print "unexpected archive member: " $0 > "/dev/stderr"; exit 1 }
        member=substr(name, length(root) + 2)
        if (!(member in expected) || index(member, "/") != 0) { print "unexpected archive member: " name > "/dev/stderr"; exit 1 }
        if ($3 !~ /^[0-9]+$/) { print "invalid archive member size: " $0 > "/dev/stderr"; exit 1 }
        if (++seen[member] != 1) { print "duplicate archive member: " name > "/dev/stderr"; exit 1 }
        total += $3
        if (total > max) { print "archive members exceed 512 MiB" > "/dev/stderr"; exit 1 }
    }
    END {
        if (roots != 1) { print "archive must contain exactly one package root directory" > "/dev/stderr"; exit 1 }
        for (member in expected) if (seen[member] != 1) { print "archive is missing " member > "/dev/stderr"; exit 1 }
    }
' "$listing" || fail 'verified archive does not have the expected safe package layout'

extract_root="$temp_root/extract"
mkdir -p "$extract_root"
tar -xf "$unpacked_tar" -C "$extract_root"
[ -x "$extract_root/$package_root/vexa-boot" ] || fail "verified archive does not contain executable '$package_root/vexa-boot'"
[ -x "$extract_root/$package_root/vexa-core" ] || fail "verified archive does not contain executable '$package_root/vexa-core'"
[ -f "$extract_root/$package_root/THIRD_PARTY_LICENSES.txt" ] || fail "verified archive does not contain '$package_root/THIRD_PARTY_LICENSES.txt'"

versions_root="$install_root/versions"
version_dir="$versions_root/$version"
[ ! -L "$version_dir" ] || fail 'installed version must not be a symbolic link'
mkdir -p "$versions_root"
printf '%s\n' "$install_marker" > "$install_root/.install-marker"
stage_dir="$versions_root/.stage-$version-$$"
mv "$extract_root/$package_root" "$stage_dir"

replaced_dir=''
if [ -e "$version_dir" ]; then
    replaced_dir="$versions_root/.replaced-$version-$(date +%Y%m%d%H%M%S)-$$"
    mv "$version_dir" "$replaced_dir"
fi
if ! mv "$stage_dir" "$version_dir"; then
    [ -n "$replaced_dir" ] && mv "$replaced_dir" "$version_dir"
    fail "could not activate staged version directory '$version_dir'"
fi

if ! VEXA_INSTALL_ROOT="$install_root" "$version_dir/vexa-core" install-activate --version "$version" > "$temp_root/activation.json"; then
    rm -rf "$version_dir"
    if [ -n "$replaced_dir" ] && [ -e "$replaced_dir" ]; then
        mv "$replaced_dir" "$version_dir"
    fi
    fail 'verified installation activation or health check failed; the new version was not activated'
fi

previous_version=''
if [ -f "$install_root/previous.json" ]; then
    previous_version=$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$install_root/previous.json" | head -n 1)
fi
launcher="$install_root/vexa"
launcher_tmp="$install_root/.vexa-$$"
cp "$version_dir/vexa-boot" "$launcher_tmp"
chmod 0755 "$launcher_tmp"
mv -f "$launcher_tmp" "$launcher"

path_contains() {
    case ":$PATH:" in *":$1:"*) return 0 ;; *) return 1 ;; esac
}

if [ "$no_modify_path" -eq 0 ]; then
    if ! path_contains "$install_root"; then
        case "${SHELL:-}" in
            */zsh) profile="$HOME/.zshrc" ;;
            */bash) profile="$HOME/.bashrc" ;;
            *) profile="$HOME/.profile" ;;
        esac
        if ! grep -F '# >>> vexa installer >>>' "$profile" >/dev/null 2>&1; then
            escaped_install_root=$(printf '%s' "$install_root" | sed 's/[\\$"`]/\\&/g')
            {
                printf '\n# >>> vexa installer >>>\n'
                printf 'case ":$PATH:" in\n'
                printf '  *":%s:"*) ;;\n' "$escaped_install_root"
                printf '  *) export PATH="%s:$PATH" ;;\n' "$escaped_install_root"
                printf 'esac\n'
                printf '# <<< vexa installer <<<\n'
            } >> "$profile"
            echo "Added '$install_root' to PATH in '$profile'."
        fi
    fi
fi
case ":$PATH:" in *":$install_root:"*) ;; *) PATH="$install_root:$PATH"; export PATH ;; esac

if [ "$no_codex" -eq 0 ]; then
    if ! "$launcher" hook codex-register; then
        echo 'Codex registration failed; VEXA itself is installed.' >&2
    fi
fi

echo "Installed VEXA $version to '$version_dir'."
if [ -n "$previous_version" ] && [ "$previous_version" != "$version" ]; then
    echo "Previous version $previous_version remains installed for rollback."
fi
echo "Open a new terminal if '$install_root' was just added to PATH."
