# Installing VEXA

Version `0.1.0` is currently unpublished. Online examples below require a future official release. Local packages can be installed using the offline arguments in this page; source builds require Rust only on the build machine.

VEXA installs per user and does not require administrator access, Python, Node.js, Rust, or Git. The installer resolves a published release from the official `bebabinlarsson-blip/VEXA` GitHub repository, downloads `SHA256SUMS` and the exact platform archive into a temporary directory, verifies the archive's SHA-256 digest, and only then extracts it.

Do not pipe a network response into PowerShell or a shell. In particular, do not use `curl | sh`, `curl | iex`, or equivalent commands. Obtain `install.ps1` or `install.sh` from a trusted checkout or from a release that includes the installer in its signed/checksummed assets, verify it before execution, and then run the local file.

## Windows

Run from PowerShell as the user who should own the installation:

```powershell
.\install.ps1
```

The default root is `%LOCALAPPDATA%\VEXA`. The stable BOOT launcher is `%LOCALAPPDATA%\VEXA\vexa.exe`, each release lives under `versions\<version>`, and the selected core is `versions\<version>\vexa-core.exe`. `current.json` is the BOOT manifest and is written atomically as exactly `{"schema_version":1,"version":"<version>"}`. When an earlier active version exists, its manifest is preserved as `previous.json`. The installer adds the install root itself to the current user's PATH and never modifies the machine PATH.

Install a particular published version or skip optional integration/PATH changes:

```powershell
.\install.ps1 -Version 0.1.0
.\install.ps1 -NoModifyPath
.\install.ps1 -NoCodex
```

Windows releases currently support x86_64 (`x86_64-pc-windows-msvc`). Other architectures fail before downloading an artifact.

## Linux

Run the local, verified installer:

```sh
sh ./install.sh
```

The default install root is `${XDG_DATA_HOME:-$HOME/.local/share}/vexa`. The stable BOOT launcher is `<install-root>/vexa`, each release lives under `versions/<version>`, and the selected core is `versions/<version>/vexa-core`. `current.json` is the BOOT manifest and is written atomically as exactly `{"schema_version":1,"version":"<version>"}`. A prior active manifest is retained as `previous.json`. If the install root is missing from PATH, the installer adds one marked block to `.bashrc`, `.zshrc`, or `.profile`; it does not rewrite the rest of the profile.

Supported Linux release combinations are the ones currently produced by release CI:

- x86_64 with glibc: `x86_64-unknown-linux-gnu`
- aarch64/arm64 with glibc or musl: `aarch64-unknown-linux-musl` (the published static musl binary runs on glibc systems, including Raspberry Pi OS)

The installer detects architecture and libc and stops with a diagnostic for other combinations. It accepts either `sha256sum` or `shasum -a 256` and explains what is missing if neither SHA-256 utility exists. Downloads use `curl` when available and otherwise `wget`; `gzip` and GNU `tar` with escape quoting support are required to validate the archive structure before extraction.

Useful options:

```sh
sh ./install.sh --version 0.1.0
sh ./install.sh --no-modify-path
sh ./install.sh --no-codex
```

`VEXA_INSTALL_ROOT` may override the Linux install root. BOOT lives directly in that root, so the installer adds that same directory to PATH when PATH modification is enabled.

For offline staging, pass a local package and its `SHA256SUMS` manifest with an explicit version. The installer still requires exactly one digest entry for the package filename and verifies it before extraction:

```powershell
.\install.ps1 -Version 0.1.0 -ArchivePath .\vexa-0.1.0-x86_64-pc-windows-msvc.zip -ChecksumsPath .\SHA256SUMS
```

```sh
sh ./install.sh --version 0.1.0 --archive ./vexa-0.1.0-x86_64-unknown-linux-gnu.tar.gz --checksums ./SHA256SUMS
```

This mode avoids network access and is used by CI to exercise installation from local release artifacts. A locally supplied checksum file detects accidental corruption; obtain both files from a trusted source.

## Activation, doctor, and Codex

The verified archive is staged into its version directory first. Before changing `current.json`, the installer runs the staged `vexa-core` with `VEXA_INSTALL_ROOT` pointing at the intended installation root and passes:

```text
vexa doctor
```

If doctor fails, the new version is not activated and a replaced copy of the same version is restored when possible. Only after doctor succeeds does the installer atomically place the new BOOT launcher at the install root and atomically replace `current.json`. A successful upgrade leaves the prior active version directory installed and saves its manifest as `previous.json` for rollback/uninstall handling.

After activation, unless `-NoCodex` or `--no-codex` was specified, the installer runs the installed `vexa hook codex-register` command directly. This works when the standalone installer file is downloaded by itself. Registration failure is reported without deleting an otherwise healthy VEXA install because Codex integration is optional.

## Uninstalling

Windows full uninstall removes installer-managed binaries and the user PATH entry while preserving application configuration/state by default:

```powershell
.\scripts\uninstall.ps1
```

Linux:

```sh
sh ./scripts/uninstall.sh
```

Remove one installed version instead of the whole install:

```powershell
.\scripts\uninstall.ps1 -Version 0.1.0
```

```sh
sh ./scripts/uninstall.sh --version 0.1.0
```

If the removed version is active and a retained version exists, the uninstaller uses verified rollback before deleting it. Hash or state disagreement stops removal. Removing the last active version unregisters the hook unless the keep option is set, then removes BOOT and `current.json`. Removing a selected or retained version clears rollback metadata that would reference deleted files; a later install or update records fresh metadata.

Full uninstall options are:

- Windows: `-PurgeUserData`, `-KeepPath`, and `-KeepCodex`.
- Linux: `--purge-user-data`, `--keep-path`, and `--keep-codex`.

`PurgeUserData` removes VEXA configuration/state/cache locations in addition to the versioned install. The installer refuses to claim a non-empty install root without its recognized marker. Uninstallers check that marker before side effects and reject links or reparse points in managed directories. Linked-directory rejection and single-version removal have passed offline smoke on Windows and Linux.

## Verification guarantees

The release archive is never extracted before its digest matches the exact filename entry in the release's `SHA256SUMS`. The download URL is restricted to the official GitHub repository's release path. Temporary downloads are deleted at exit. Before extraction, both installers reject links, duplicate or unexpected entries, nested paths, and compressed or uncompressed archives larger than 512 MiB. The installer never executes a downloaded archive or separate registration script; after activating the verified package it invokes the installed VEXA CLI to register the hook.

For additional release verification, GitHub CLI users can also verify release attestations as described in `docs/releasing.md` after downloading the assets.
