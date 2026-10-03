[CmdletBinding()]
param([Parameter(Mandatory = $true)][string]$Archive)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$archivePath = [IO.Path]::GetFullPath($Archive)
if (-not (Test-Path -LiteralPath $archivePath -PathType Leaf)) { throw "Package archive not found: $archivePath" }
$packageName = [IO.Path]::GetFileNameWithoutExtension($archivePath)
if ($packageName -notmatch '^vexa-(?<version>[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?)-x86_64-pc-windows-msvc$') {
    throw "Unexpected Windows package name: $packageName"
}
$version = $Matches.version
$temp = Join-Path ([IO.Path]::GetTempPath()) ('vexa-package-smoke-' + [Guid]::NewGuid().ToString('N'))
$smokeHome = Join-Path $temp 'home'
$localAppData = Join-Path $temp 'local'
$appData = Join-Path $temp 'roaming'
$codexHome = Join-Path $temp 'codex'
$installRoot = Join-Path $localAppData 'VEXA'
$configRoot = Join-Path $appData 'VEXA'
$configFile = Join-Path $configRoot 'config.json'
$checksumsPath = Join-Path $temp 'SHA256SUMS'
$badChecksumsPath = Join-Path $temp 'BAD-SHA256SUMS'
$malformedArchivePath = Join-Path $temp ([IO.Path]::GetFileName($archivePath))
$malformedChecksumsPath = Join-Path $temp 'MALFORMED-SHA256SUMS'
$savedEnv = @{}
foreach ($name in @('HOME', 'USERPROFILE', 'LOCALAPPDATA', 'APPDATA', 'CODEX_HOME', 'VEXA_INSTALL_ROOT', 'VEXA_CONFIG_ROOT', 'VEXA_CACHE_ROOT', 'PATH')) {
    $savedEnv[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}

try {
    New-Item -ItemType Directory -Path $smokeHome, $localAppData, $appData, $codexHome, (Split-Path -Parent $configFile) -Force | Out-Null
    $archiveHash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    Set-Content -LiteralPath $checksumsPath -Value "$archiveHash  $([IO.Path]::GetFileName($archivePath))"
    Set-Content -LiteralPath $badChecksumsPath -Value (('0' * 64) + "  $([IO.Path]::GetFileName($archivePath))")
    $utf8NoBom = [Text.UTF8Encoding]::new($false)
    [IO.File]::WriteAllText($configFile, '{}', $utf8NoBom)
    [IO.File]::WriteAllText((Join-Path $codexHome 'hooks.json'), '{"metadata":{"keep":true},"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"other-hook"}]}]}}', $utf8NoBom)

    $env:HOME = $smokeHome
    $env:LOCALAPPDATA = $localAppData
    $env:APPDATA = $appData
    $env:CODEX_HOME = $codexHome
    $env:VEXA_INSTALL_ROOT = $installRoot
    $env:VEXA_CONFIG_ROOT = $configRoot
    $env:VEXA_CACHE_ROOT = Join-Path $temp 'cache'
    $env:PATH = "$installRoot;$env:PATH"

    $unsafeRoot = Join-Path $temp 'unsafe-install'
    $foreignDirectory = Join-Path $temp 'foreign-directory'
    New-Item -ItemType Directory -Path $unsafeRoot, $foreignDirectory -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $unsafeRoot '.install-marker') -Value 'VEXA per-user install v1'
    Set-Content -LiteralPath (Join-Path $foreignDirectory 'keep.txt') -Value 'preserve foreign contents'
    New-Item -ItemType Junction -Path (Join-Path $unsafeRoot 'versions') -Target $foreignDirectory | Out-Null
    $unsafeInstallRejected = $false
    try {
        & (Join-Path $PSScriptRoot '..\install.ps1') -Version $version -InstallRoot $unsafeRoot -ArchivePath $archivePath -ChecksumsPath $checksumsPath -NoModifyPath -NoCodex
    } catch { $unsafeInstallRejected = $true }
    $unsafeUninstallRejected = $false
    try { & (Join-Path $PSScriptRoot 'uninstall.ps1') -InstallRoot $unsafeRoot -KeepPath -KeepCodex } catch { $unsafeUninstallRejected = $true }
    if (-not $unsafeInstallRejected -or -not $unsafeUninstallRejected -or
        (Get-Content -LiteralPath (Join-Path $foreignDirectory 'keep.txt') -Raw).Trim() -ne 'preserve foreign contents') {
        throw 'Installer/uninstaller failed to reject a versions junction and preserve its target'
    }

    # Repack the same binaries under a synthetic older version to exercise manual upgrade selection.
    # This is lifecycle test data, never a release artifact or a performance measurement.
    $oldVersion = '0.0.0-smoke-old'
    $oldPackageName = "vexa-$oldVersion-x86_64-pc-windows-msvc"
    $oldStage = Join-Path $temp 'old-package'
    Expand-Archive -LiteralPath $archivePath -DestinationPath $oldStage
    Move-Item -LiteralPath (Join-Path $oldStage $packageName) -Destination (Join-Path $oldStage $oldPackageName)
    $oldArchive = Join-Path $temp ($oldPackageName + '.zip')
    Add-Type -AssemblyName System.IO.Compression
    [IO.Compression.ZipFile]::CreateFromDirectory($oldStage, $oldArchive, [IO.Compression.CompressionLevel]::NoCompression, $false)
    $oldHash = (Get-FileHash -LiteralPath $oldArchive -Algorithm SHA256).Hash.ToLowerInvariant()
    $oldChecksums = Join-Path $temp 'OLD-SHA256SUMS'
    Set-Content -LiteralPath $oldChecksums -Value "$oldHash  $([IO.Path]::GetFileName($oldArchive))"
    & (Join-Path $PSScriptRoot '..\install.ps1') -Version $oldVersion -InstallRoot $installRoot -ArchivePath $oldArchive -ChecksumsPath $oldChecksums -NoModifyPath -NoCodex
    & (Join-Path $PSScriptRoot '..\install.ps1') -Version $version -InstallRoot $installRoot -ArchivePath $archivePath -ChecksumsPath $checksumsPath -NoModifyPath
    $launcher = Join-Path $installRoot 'vexa.exe'
    $doctor = & $launcher doctor --json | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or -not $doctor.platform_supported) { throw 'Installed BOOT/core doctor smoke failed' }

    $hooksPath = Join-Path $codexHome 'hooks.json'
    $hooks = Get-Content -LiteralPath $hooksPath -Raw | ConvertFrom-Json
    $vexaHandlers = @($hooks.hooks.UserPromptSubmit | ForEach-Object { $_.hooks } | ForEach-Object { $_ } | Where-Object { $_.command -eq 'vexa hook codex-user-prompt-submit' })
    if ($vexaHandlers.Count -ne 1) { throw 'Installer did not register exactly one VEXA Codex handler' }
    if (-not ($hooks | ConvertTo-Json -Depth 100).Contains('other-hook')) { throw 'Installer removed an unrelated hook' }
    $currentBeforeBadInstall = Get-Content -LiteralPath (Join-Path $installRoot 'current.json') -Raw
    $launcherHashBeforeBadInstall = (Get-FileHash -LiteralPath $launcher -Algorithm SHA256).Hash
    $badInstallFailed = $false
    try {
        & (Join-Path $PSScriptRoot '..\install.ps1') -Version $version -InstallRoot $installRoot -ArchivePath $archivePath -ChecksumsPath $badChecksumsPath -NoModifyPath -NoCodex
    } catch { $badInstallFailed = $true }
    if (-not $badInstallFailed) { throw 'Installer accepted a corrupt checksum manifest' }
    if ((Get-Content -LiteralPath (Join-Path $installRoot 'current.json') -Raw) -ne $currentBeforeBadInstall) { throw 'Failed checksum changed the active installation' }
    if ((Get-FileHash -LiteralPath $launcher -Algorithm SHA256).Hash -ne $launcherHashBeforeBadInstall) { throw 'Failed checksum changed the active BOOT launcher' }
    $doctor = & $launcher doctor --json | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or -not $doctor.platform_supported) { throw 'Existing installation failed after rejecting a corrupt package' }

    Copy-Item -LiteralPath $archivePath -Destination $malformedArchivePath
    Add-Type -AssemblyName System.IO.Compression
    $zip = [IO.Compression.ZipFile]::Open($malformedArchivePath, [IO.Compression.ZipArchiveMode]::Update)
    try { $null = $zip.CreateEntry("$packageName/unexpected.txt") } finally { $zip.Dispose() }
    $malformedHash = (Get-FileHash -LiteralPath $malformedArchivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    Set-Content -LiteralPath $malformedChecksumsPath -Value "$malformedHash  $([IO.Path]::GetFileName($malformedArchivePath))"
    $malformedInstallFailed = $false
    try {
        & (Join-Path $PSScriptRoot '..\install.ps1') -Version $version -InstallRoot $installRoot -ArchivePath $malformedArchivePath -ChecksumsPath $malformedChecksumsPath -NoModifyPath -NoCodex
    } catch { $malformedInstallFailed = $true }
    if (-not $malformedInstallFailed) { throw 'Installer accepted an archive containing an unexpected package entry' }
    if ((Get-Content -LiteralPath (Join-Path $installRoot 'current.json') -Raw) -ne $currentBeforeBadInstall) { throw 'Rejected archive structure changed the active installation' }
    if ((Get-FileHash -LiteralPath $launcher -Algorithm SHA256).Hash -ne $launcherHashBeforeBadInstall) { throw 'Rejected archive structure changed the active BOOT launcher' }

    & (Join-Path $PSScriptRoot '..\install.ps1') -Version $version -InstallRoot $installRoot -ArchivePath $archivePath -ChecksumsPath $checksumsPath -NoModifyPath
    $hooks = Get-Content -LiteralPath $hooksPath -Raw | ConvertFrom-Json
    $vexaHandlers = @($hooks.hooks.UserPromptSubmit | ForEach-Object { $_.hooks } | ForEach-Object { $_ } | Where-Object { $_.command -eq 'vexa hook codex-user-prompt-submit' })
    if ($vexaHandlers.Count -ne 1) { throw 'Reinstall duplicated the VEXA Codex handler' }

    $rollback = & $launcher rollback --json | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or $rollback.current.version -ne $oldVersion) { throw 'Manual upgrade/reinstall lost rollback metadata' }
    $rollback = & $launcher rollback --json | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or $rollback.current.version -ne $version) { throw 'Rollback did not retain the newer version' }

    & (Join-Path $PSScriptRoot 'uninstall.ps1') -InstallRoot $installRoot -Version $version -KeepPath
    $selectedAfterRemoval = Get-Content -LiteralPath (Join-Path $installRoot 'current.json') -Raw | ConvertFrom-Json
    if ($selectedAfterRemoval.version -ne $oldVersion) { throw 'Version removal failed to select retained verified core' }
    if (Test-Path -LiteralPath (Join-Path $installRoot "versions/$version")) { throw 'Version removal retained the selected directory' }
    $doctor = & $launcher doctor --json | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or -not $doctor.platform_supported) { throw 'Retained version failed after version removal' }
    & (Join-Path $PSScriptRoot '..\install.ps1') -Version $version -InstallRoot $installRoot -ArchivePath $archivePath -ChecksumsPath $checksumsPath -NoModifyPath

    & (Join-Path $PSScriptRoot 'uninstall.ps1') -InstallRoot $installRoot -KeepPath
    if (Test-Path -LiteralPath $installRoot) { throw 'Uninstaller left the managed install root behind' }
    if (-not (Test-Path -LiteralPath $configFile)) { throw 'Uninstaller removed user configuration without purge option' }
    $hooks = Get-Content -LiteralPath $hooksPath -Raw | ConvertFrom-Json
    if (($hooks | ConvertTo-Json -Depth 100).Contains('vexa hook codex-user-prompt-submit')) { throw 'Uninstaller left the VEXA hook behind' }
    if (-not ($hooks | ConvertTo-Json -Depth 100).Contains('other-hook')) { throw 'Uninstaller removed an unrelated hook' }
    Write-Output "Offline Windows installer lifecycle smoke passed for $packageName."
}
finally {
    foreach ($name in $savedEnv.Keys) { [Environment]::SetEnvironmentVariable($name, $savedEnv[$name], 'Process') }
    if (Test-Path -LiteralPath $temp) {
        $tempFull = [IO.Path]::GetFullPath($temp)
        $tempBaseFull = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\', '/')
        if (-not $tempFull.StartsWith($tempBaseFull + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
            throw "Refusing to recursively remove smoke path outside the system temp directory: '$tempFull'."
        }
        for ($attempt = 0; $attempt -lt 10; $attempt++) {
            try {
                Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction Stop
                break
            } catch {
                if ($attempt -eq 9) { throw }
                Start-Sleep -Milliseconds 200
            }
        }
    }
}
