[CmdletBinding()]
param(
    [string]$Version,
    [string]$InstallRoot,
    [switch]$PurgeUserData,
    [switch]$KeepPath,
    [switch]$KeepCodex
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$InstallMarker = 'VEXA per-user install v1'

if ([string]::IsNullOrWhiteSpace($InstallRoot)) {
    $base = if (-not [string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) { $env:LOCALAPPDATA } else { [Environment]::GetFolderPath('LocalApplicationData') }
    $InstallRoot = Join-Path $base 'VEXA'
}
$InstallRoot = [IO.Path]::GetFullPath($InstallRoot)
$marker = Join-Path $InstallRoot '.install-marker'
$versionsRoot = Join-Path $InstallRoot 'versions'
$launcher = Join-Path $InstallRoot 'vexa.exe'

function Get-ManifestVersion([string]$Name) {
    $path = Join-Path $InstallRoot $Name
    if (-not (Test-Path -LiteralPath $path)) { return $null }
    try {
        $manifest = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
    } catch {
        return $null
    }
    $value = [string]$manifest.version
    if ($manifest.schema_version -eq 1 -and $value -match '^[0-9A-Za-z.+-]+$') { return $value }
    return $null
}

function Write-Manifest([string]$Name, [string]$Value) {
    $path = Join-Path $InstallRoot $Name
    $temp = $path + '.tmp'
    $utf8 = New-Object Text.UTF8Encoding($false)
    [IO.File]::WriteAllText($temp, '{"schema_version":1,"version":"' + $Value + '"}', $utf8)
    Move-Item -LiteralPath $temp -Destination $path -Force
}

function Remove-UserPathEntry([string]$Entry) {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ([string]::IsNullOrWhiteSpace($userPath)) { return }
    $needle = $Entry.TrimEnd('\')
    $kept = @()
    foreach ($part in $userPath.Split(';')) {
        $trimmed = $part.Trim()
        if (-not [string]::IsNullOrWhiteSpace($trimmed) -and -not $trimmed.TrimEnd('\').Equals($needle, [StringComparison]::OrdinalIgnoreCase)) {
            $kept += $trimmed
        }
    }
    [Environment]::SetEnvironmentVariable('Path', ($kept -join ';'), 'User')
}

function Assert-SafeDirectory([string]$Path, [string]$Boundary) {
    $resolved = [IO.Path]::GetFullPath($Path).TrimEnd('\', '/')
    $boundaryFull = [IO.Path]::GetFullPath($Boundary).TrimEnd('\', '/')
    if (-not $resolved.Equals($boundaryFull, [StringComparison]::OrdinalIgnoreCase) -and
        -not $resolved.StartsWith($boundaryFull + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing a directory outside its checked boundary: '$resolved'."
    }
    $cursor = $resolved
    while ($true) {
        if (Test-Path -LiteralPath $cursor) {
            $item = Get-Item -LiteralPath $cursor -Force
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Refusing a reparse point in a managed directory: '$cursor'."
            }
        }
        if ($cursor.Equals($boundaryFull, [StringComparison]::OrdinalIgnoreCase)) { break }
        $cursor = [IO.Path]::GetDirectoryName($cursor)
    }
}

if ($InstallRoot.TrimEnd('\', '/') -eq [IO.Path]::GetPathRoot($InstallRoot).TrimEnd('\', '/') -or
    $InstallRoot.TrimEnd('\', '/') -eq [Environment]::GetFolderPath('UserProfile').TrimEnd('\', '/')) {
    throw "Unsafe install root '$InstallRoot'."
}
Assert-SafeDirectory $InstallRoot $InstallRoot
Assert-SafeDirectory $versionsRoot $InstallRoot
Assert-SafeDirectory (Join-Path $InstallRoot 'updates') $InstallRoot
if (Test-Path -LiteralPath $InstallRoot) {
    if (-not (Test-Path -LiteralPath $marker) -or (Get-Content -LiteralPath $marker -Raw).Trim() -ne $InstallMarker) {
        throw "Refusing uninstall because '$InstallRoot' is not marked as a VEXA installer directory."
    }
}

if (-not [string]::IsNullOrWhiteSpace($Version)) {
    $Version = $Version.TrimStart('v')
    if ($Version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$') { throw "Invalid version '$Version'." }
    if (-not (Test-Path -LiteralPath $marker) -or (Get-Content -LiteralPath $marker -Raw).Trim() -ne $InstallMarker) {
        throw "Refusing version removal because '$InstallRoot' is not marked as a VEXA installer directory."
    }
    $versionDir = Join-Path $versionsRoot $Version
    Assert-SafeDirectory $versionDir $InstallRoot
    if (-not (Test-Path -LiteralPath $versionDir)) { throw "VEXA version $Version is not installed." }
    $current = Get-ManifestVersion 'current.json'
    $previous = Get-ManifestVersion 'previous.json'
    if ($current -eq $Version) {
        $previous = Get-ManifestVersion 'previous.json'
        if ($null -ne $previous -and $previous -ne $Version -and (Test-Path -LiteralPath (Join-Path $versionsRoot $previous))) {
            & $launcher rollback --json | Out-Null
            if ($LASTEXITCODE -ne 0) { throw 'Verified rollback failed; refusing version removal.' }
            Remove-Item -LiteralPath (Join-Path $InstallRoot 'previous.json') -Force -ErrorAction SilentlyContinue
            Write-Host "Activated previous VEXA version $previous."
        } else {
            if (-not $KeepCodex) {
                & $launcher hook codex-unregister
                if ($LASTEXITCODE -ne 0) { throw 'Codex unregister failed; retaining the last installed version.' }
            }
            Remove-Item -LiteralPath (Join-Path $InstallRoot 'current.json') -Force -ErrorAction SilentlyContinue
            Remove-Item -LiteralPath $launcher -Force -ErrorAction SilentlyContinue
        }
    }
    if ($current -eq $Version -or $previous -eq $Version) {
        foreach ($metadata in @('previous.json', 'updates/state.json', 'updates/state.json.bak')) {
            Remove-Item -LiteralPath (Join-Path $InstallRoot $metadata) -Force -ErrorAction SilentlyContinue
        }
    }
    Assert-SafeDirectory $versionDir $InstallRoot
    Remove-Item -LiteralPath $versionDir -Recurse -Force
    Write-Host "Removed VEXA version $Version."
    exit 0
}

if (-not $KeepCodex -and (Test-Path -LiteralPath $launcher -PathType Leaf)) {
    & $launcher hook codex-unregister
    if ($LASTEXITCODE -ne 0) { throw 'Codex unregister failed; retaining binaries for retry.' }
}

if (-not $KeepPath) { Remove-UserPathEntry $InstallRoot }

if (Test-Path -LiteralPath $InstallRoot) {
    if (-not (Test-Path -LiteralPath $marker) -or (Get-Content -LiteralPath $marker -Raw).Trim() -ne $InstallMarker) {
        throw "Refusing recursive removal because '$InstallRoot' is not marked as a VEXA installer directory."
    }
    Assert-SafeDirectory $InstallRoot $InstallRoot
    Remove-Item -LiteralPath $InstallRoot -Recurse -Force
}

if ($PurgeUserData) {
    $paths = @()
    if (-not [string]::IsNullOrWhiteSpace($env:APPDATA)) { $paths += (Join-Path $env:APPDATA 'VEXA') }
    if (-not [string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) { $paths += (Join-Path $env:LOCALAPPDATA 'VEXA') }
    foreach ($path in $paths | Select-Object -Unique) {
        if (Test-Path -LiteralPath $path) {
            Assert-SafeDirectory $path ([IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($path)))
            Remove-Item -LiteralPath $path -Recurse -Force
        }
    }
}

Write-Host 'VEXA binaries were uninstalled for the current user.'
if (-not $PurgeUserData) { Write-Host 'User configuration/state was preserved. Use -PurgeUserData to remove it.' }
