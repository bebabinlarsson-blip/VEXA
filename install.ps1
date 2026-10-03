[CmdletBinding()]
param(
    [string]$Version,
    [string]$InstallRoot,
    [string]$ArchivePath,
    [string]$ChecksumsPath,
    [switch]$NoModifyPath,
    [switch]$NoCodex
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$Repository = 'bebabinlarsson-blip/VEXA'
$ApiBase = "https://api.github.com/repos/$Repository"
$UserAgent = 'vexa-installer/1'
$Target = 'x86_64-pc-windows-msvc'
$InstallMarker = 'VEXA per-user install v1'

function Resolve-DefaultInstallRoot {
    if (-not [string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
        return (Join-Path $env:LOCALAPPDATA 'VEXA')
    }
    return (Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'VEXA')
}

function Invoke-GitHubJson {
    param([Parameter(Mandatory = $true)][string]$Uri)

    $headers = @{
        'Accept' = 'application/vnd.github+json'
        'User-Agent' = $UserAgent
        'X-GitHub-Api-Version' = '2022-11-28'
    }
    try {
        return ((Invoke-WebRequest -UseBasicParsing -Headers $headers -Uri $Uri).Content | ConvertFrom-Json)
    } catch {
        throw "Unable to resolve the official VEXA release from '$Uri'. $($_.Exception.Message)"
    }
}

function Invoke-Download {
    param(
        [Parameter(Mandatory = $true)][string]$Uri,
        [Parameter(Mandatory = $true)][string]$Destination
    )

    if (-not $Uri.StartsWith("https://github.com/$Repository/releases/download/", [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing non-official release download URL: $Uri"
    }
    Invoke-WebRequest -UseBasicParsing -Headers @{ 'User-Agent' = $UserAgent } -Uri $Uri -OutFile $Destination
    if (-not (Test-Path -LiteralPath $Destination -PathType Leaf)) {
        throw "Download did not create '$Destination'."
    }
}

function Get-ExpectedChecksum {
    param(
        [Parameter(Mandatory = $true)][string]$ChecksumsPath,
        [Parameter(Mandatory = $true)][string]$AssetName
    )

    $hashes = @()
    foreach ($line in Get-Content -LiteralPath $ChecksumsPath) {
        if ($line -match '^(?<hash>[0-9A-Fa-f]{64})\s+[* ]?(?<name>.+)$' -and $Matches.name -eq $AssetName) {
            $hashes += $Matches.hash.ToLowerInvariant()
        }
    }
    if ($hashes.Count -ne 1) {
        throw "SHA256SUMS must contain exactly one checksum for '$AssetName'; found $($hashes.Count)."
    }
    return $hashes[0]
}

function Get-ContainedChildPath {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$Leaf
    )

    if ([IO.Path]::IsPathRooted($Leaf) -or [IO.Path]::GetFileName($Leaf) -ne $Leaf) {
        throw "Refusing a non-child path component '$Leaf'."
    }
    if (Test-Path -LiteralPath $Root) {
        $rootItem = Get-Item -LiteralPath $Root -Force
        if (($rootItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Refusing a reparse point as a checked directory: '$Root'."
        }
    }
    $rootFull = [IO.Path]::GetFullPath($Root).TrimEnd('\', '/')
    $childFull = [IO.Path]::GetFullPath((Join-Path $rootFull $Leaf))
    $prefix = $rootFull + [IO.Path]::DirectorySeparatorChar
    if (-not $childFull.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase) -or
        [IO.Path]::GetDirectoryName($childFull).TrimEnd('\', '/') -ne $rootFull) {
        throw "Refusing a path outside its checked parent: '$childFull'."
    }
    if (Test-Path -LiteralPath $childFull) {
        $childItem = Get-Item -LiteralPath $childFull -Force
        if (($childItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Refusing a reparse point as a checked child: '$childFull'."
        }
    }
    return $childFull
}

function Test-PathEntry {
    param([string]$PathValue, [string]$Entry)
    if ([string]::IsNullOrWhiteSpace($PathValue)) { return $false }
    $needle = $Entry.TrimEnd('\')
    foreach ($part in $PathValue.Split(';')) {
        if ($part.Trim().TrimEnd('\').Equals($needle, [StringComparison]::OrdinalIgnoreCase)) {
            return $true
        }
    }
    return $false
}

function Add-UserPathEntry {
    param([Parameter(Mandatory = $true)][string]$Entry)

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not (Test-PathEntry -PathValue $userPath -Entry $Entry)) {
        $newPath = if ([string]::IsNullOrWhiteSpace($userPath)) { $Entry } else { "$($userPath.TrimEnd(';'));$Entry" }
        [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
        Write-Host "Added '$Entry' to the current user's PATH."
    }
    if (-not (Test-PathEntry -PathValue $env:Path -Entry $Entry)) {
        $env:Path = "$Entry;$env:Path"
    }
}

function Write-TextAtomically {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Content
    )

    $directory = Split-Path -Parent $Path
    $temp = Join-Path $directory ('.vexa-' + [Guid]::NewGuid().ToString('N') + '.tmp')
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    try {
        [IO.File]::WriteAllText($temp, $Content, $utf8)
        Move-Item -LiteralPath $temp -Destination $Path -Force
    } finally {
        if (Test-Path -LiteralPath $temp) { Remove-Item -LiteralPath $temp -Force }
    }
}

if ([string]::IsNullOrWhiteSpace($InstallRoot)) {
    $InstallRoot = Resolve-DefaultInstallRoot
}
$InstallRoot = [IO.Path]::GetFullPath($InstallRoot)
$existingMarker = Join-Path $InstallRoot '.install-marker'
if (Test-Path -LiteralPath $InstallRoot) {
    if (Test-Path -LiteralPath $existingMarker) {
        if ((Get-Content -LiteralPath $existingMarker -Raw).Trim() -ne $InstallMarker) {
            throw "Refusing to use '$InstallRoot' because its install marker is not recognized."
        }
    } elseif (@(Get-ChildItem -LiteralPath $InstallRoot -Force).Count -gt 0) {
        throw "Refusing to claim non-empty unmarked install root '$InstallRoot'."
    }
}

$arch = if (-not [string]::IsNullOrWhiteSpace($env:PROCESSOR_ARCHITEW6432)) {
    $env:PROCESSOR_ARCHITEW6432
} else {
    $env:PROCESSOR_ARCHITECTURE
}
if ($arch -notin @('AMD64', 'x86_64')) {
    throw "Unsupported Windows architecture '$arch'. VEXA currently publishes Windows x86_64 only."
}

if ((-not [string]::IsNullOrWhiteSpace($ArchivePath)) -xor (-not [string]::IsNullOrWhiteSpace($ChecksumsPath))) {
    throw 'ArchivePath and ChecksumsPath must be provided together.'
}
$useLocalPackage = -not [string]::IsNullOrWhiteSpace($ArchivePath)
if ($useLocalPackage -and [string]::IsNullOrWhiteSpace($Version)) {
    throw 'Version is required when installing a local package.'
}

if ($useLocalPackage) {
    $tag = if ($Version.StartsWith('v', [StringComparison]::OrdinalIgnoreCase)) { $Version } else { "v$Version" }
    if ($tag -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$') { throw "Invalid VEXA version '$Version'." }
    $resolvedVersion = $tag.Substring(1)
    $assetName = "vexa-$resolvedVersion-$Target.zip"
    if ([IO.Path]::GetFileName($ArchivePath) -ne $assetName) { throw "Local package must be named '$assetName'." }
} elseif ([string]::IsNullOrWhiteSpace($Version)) {
    $release = Invoke-GitHubJson -Uri "$ApiBase/releases/latest"
} else {
    $tag = if ($Version.StartsWith('v', [StringComparison]::OrdinalIgnoreCase)) { $Version } else { "v$Version" }
    if ($tag -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$') {
        throw "Invalid VEXA version '$Version'."
    }
    $release = Invoke-GitHubJson -Uri "$ApiBase/releases/tags/$([Uri]::EscapeDataString($tag))"
}

if (-not $useLocalPackage) {
    if ($release.draft -eq $true) { throw 'Refusing to install a draft GitHub release.' }
    $releaseTag = [string]$release.tag_name
    if ($releaseTag -notmatch '^v(?<version>[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?)$') {
        throw "Official release returned an invalid tag '$releaseTag'."
    }
    $resolvedVersion = $Matches.version
    $assetName = "vexa-$resolvedVersion-$Target.zip"
}
$checksumName = 'SHA256SUMS'
$asset = @()
$checksumAsset = @()
if (-not $useLocalPackage) {
    $asset = @($release.assets | Where-Object { $_.name -eq $assetName })
    $checksumAsset = @($release.assets | Where-Object { $_.name -eq $checksumName })
    if ($asset.Count -ne 1) { throw "Release '$releaseTag' does not contain exactly one '$assetName' asset." }
    if ($checksumAsset.Count -ne 1) { throw "Release '$releaseTag' does not contain exactly one SHA256SUMS asset." }
}

$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ('vexa-install-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tempRoot | Out-Null
try {
    $downloadChecksumsPath = Join-Path $tempRoot $checksumName
    $downloadArchivePath = Join-Path $tempRoot $assetName
    if ($useLocalPackage) { Copy-Item -LiteralPath $ChecksumsPath -Destination $downloadChecksumsPath }
    else { Invoke-Download -Uri ([string]$checksumAsset[0].browser_download_url) -Destination $downloadChecksumsPath }
    $expected = Get-ExpectedChecksum -ChecksumsPath $downloadChecksumsPath -AssetName $assetName
    if ($useLocalPackage) { Copy-Item -LiteralPath $ArchivePath -Destination $downloadArchivePath }
    else { Invoke-Download -Uri ([string]$asset[0].browser_download_url) -Destination $downloadArchivePath }
    $actual = (Get-FileHash -LiteralPath $downloadArchivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        throw "SHA-256 mismatch for '$assetName'. Expected $expected, got $actual. The archive was not extracted."
    }
    Write-Host "Verified SHA-256 for '$assetName'."

    $extractRoot = Join-Path $tempRoot 'extract'
    New-Item -ItemType Directory -Path $extractRoot | Out-Null
    $packageRootName = "vexa-$resolvedVersion-$Target"
    $archiveLimit = 512L * 1024L * 1024L
    if ((Get-Item -LiteralPath $downloadArchivePath).Length -gt $archiveLimit) {
        throw 'Verified archive exceeds the 512 MiB compressed size limit.'
    }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [IO.Compression.ZipFile]::OpenRead($downloadArchivePath)
    try {
        if ($zip.Entries.Count -lt 4 -or $zip.Entries.Count -gt 5) {
            throw 'Verified archive must contain exactly the package root and four required files.'
        }
        $expectedFiles = @(
            "$packageRootName/vexa-boot.exe",
            "$packageRootName/vexa-core.exe",
            "$packageRootName/LICENSE",
            "$packageRootName/THIRD_PARTY_LICENSES.txt"
        )
        $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
        $seenFiles = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        $uncompressedSize = 0L
        foreach ($entry in $zip.Entries) {
            $member = $entry.FullName
            if ($member.Contains('\') -or -not $seen.Add($member)) {
                throw "Verified archive contains unexpected path '$($entry.FullName)'."
            }
            $isRootDirectory = [String]::Equals($member, "$packageRootName/", [StringComparison]::Ordinal)
            $isExpectedFile = $false
            foreach ($expectedFile in $expectedFiles) {
                if ([String]::Equals($member, $expectedFile, [StringComparison]::Ordinal)) { $isExpectedFile = $true; break }
            }
            if (-not $isRootDirectory -and -not $isExpectedFile) {
                throw "Verified archive contains unexpected path '$($entry.FullName)'."
            }
            if ($isRootDirectory) {
                if ($entry.Length -ne 0 -or $entry.CompressedLength -ne 0) { throw 'Package root directory entry must be empty.' }
            } else {
                if (-not $seenFiles.Add($member)) { throw "Verified archive contains duplicate file '$member'." }
                $uncompressedSize += $entry.Length
                if ($uncompressedSize -gt $archiveLimit) { throw 'Verified archive exceeds the 512 MiB uncompressed file size limit.' }
            }
            $unixType = ($entry.ExternalAttributes -shr 16) -band 0xF000
            if ($unixType -ne 0 -and $unixType -ne $(if ($isRootDirectory) { 0x4000 } else { 0x8000 })) {
                throw "Verified archive contains a link or non-file entry '$member'."
            }
            if (-not $isRootDirectory -and ($entry.ExternalAttributes -band [int][IO.FileAttributes]::Directory) -ne 0) {
                throw "Verified archive file is marked as a directory: '$member'."
            }
            if (($entry.ExternalAttributes -band [int][IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Verified archive contains a reparse point '$member'."
            }
        }
        if ($seenFiles.Count -ne $expectedFiles.Count) { throw 'Verified archive is missing one or more required files.' }
    } finally {
        $zip.Dispose()
    }
    Expand-Archive -LiteralPath $downloadArchivePath -DestinationPath $extractRoot
    $packageRoot = Join-Path $extractRoot $packageRootName
    $packageBoot = Join-Path $packageRoot 'vexa-boot.exe'
    $packageCore = Join-Path $packageRoot 'vexa-core.exe'
    $packageThirdPartyLicenses = Join-Path $packageRoot 'THIRD_PARTY_LICENSES.txt'
    if (-not (Test-Path -LiteralPath $packageBoot -PathType Leaf)) {
        throw "Verified archive does not contain the expected '$packageRootName\vexa-boot.exe'."
    }
    if (-not (Test-Path -LiteralPath $packageCore -PathType Leaf)) {
        throw "Verified archive does not contain the expected '$packageRootName\vexa-core.exe'."
    }
    if (-not (Test-Path -LiteralPath $packageThirdPartyLicenses -PathType Leaf)) {
        throw "Verified archive does not contain the expected '$packageRootName\THIRD_PARTY_LICENSES.txt'."
    }

    $versionsRoot = Get-ContainedChildPath -Root $InstallRoot -Leaf 'versions'
    $versionDir = Get-ContainedChildPath -Root $versionsRoot -Leaf $resolvedVersion
    $markerPath = Join-Path $InstallRoot '.install-marker'
    New-Item -ItemType Directory -Path $versionsRoot -Force | Out-Null
    Write-TextAtomically -Path $markerPath -Content ($InstallMarker + [Environment]::NewLine)

    $stageDir = Get-ContainedChildPath -Root $versionsRoot -Leaf ('.stage-' + $resolvedVersion + '-' + [Guid]::NewGuid().ToString('N'))
    Move-Item -LiteralPath $packageRoot -Destination $stageDir
    $replacedDir = $null
    if (Test-Path -LiteralPath $versionDir) {
        $replacedDir = Get-ContainedChildPath -Root $versionsRoot -Leaf ('.replaced-' + $resolvedVersion + '-' + (Get-Date -Format 'yyyyMMddHHmmss') + '-' + [Guid]::NewGuid().ToString('N'))
        Move-Item -LiteralPath $versionDir -Destination $replacedDir
    }
    try {
        Move-Item -LiteralPath $stageDir -Destination $versionDir
        $versionCore = Join-Path $versionDir 'vexa-core.exe'
        $oldInstallRoot = $env:VEXA_INSTALL_ROOT
        $env:VEXA_INSTALL_ROOT = $InstallRoot
        try {
            & $versionCore doctor
            $doctorExit = $LASTEXITCODE
            if ($doctorExit -ne 0) { throw "'vexa doctor' failed with exit code $doctorExit." }
            $activation = & $versionCore install-activate --version $resolvedVersion
            if ($LASTEXITCODE -ne 0) { throw 'Verified installation could not be activated.' }
            $activationState = $activation | ConvertFrom-Json
            $previousVersion = $null
            if ($activationState.PSObject.Properties.Name -contains 'previous') {
                $previousVersion = [string]$activationState.previous.version
            }
        } finally {
            if ($null -eq $oldInstallRoot) {
                Remove-Item Env:VEXA_INSTALL_ROOT -ErrorAction SilentlyContinue
            } else {
                $env:VEXA_INSTALL_ROOT = $oldInstallRoot
            }
        }
        if ($doctorExit -ne 0) {
            throw "'vexa doctor' failed with exit code $doctorExit. The new version was not activated."
        }
    } catch {
        if (Test-Path -LiteralPath $versionDir) {
            $versionDir = Get-ContainedChildPath -Root $versionsRoot -Leaf ([IO.Path]::GetFileName($versionDir))
            Remove-Item -LiteralPath $versionDir -Recurse -Force
        }
        if ($null -ne $replacedDir -and (Test-Path -LiteralPath $replacedDir)) {
            $replacedDir = Get-ContainedChildPath -Root $versionsRoot -Leaf ([IO.Path]::GetFileName($replacedDir))
            $versionDir = Get-ContainedChildPath -Root $versionsRoot -Leaf $resolvedVersion
            Move-Item -LiteralPath $replacedDir -Destination $versionDir
        }
        throw
    }

    $bootSource = Join-Path $versionDir 'vexa-boot.exe'
    $launcher = Join-Path $InstallRoot 'vexa.exe'
    $launcherTemp = Join-Path $InstallRoot ('.vexa-' + [Guid]::NewGuid().ToString('N') + '.exe')
    Copy-Item -LiteralPath $bootSource -Destination $launcherTemp
    Move-Item -LiteralPath $launcherTemp -Destination $launcher -Force

    if (-not $NoModifyPath) {
        Add-UserPathEntry -Entry $InstallRoot
    } elseif (-not (Test-PathEntry -PathValue $env:Path -Entry $InstallRoot)) {
        $env:Path = "$InstallRoot;$env:Path"
    }

    if (-not $NoCodex) {
        & $launcher hook codex-register
        if ($LASTEXITCODE -ne 0) {
            Write-Warning "VEXA Codex registration exited with code $LASTEXITCODE. VEXA itself is installed."
        }
    }

    Write-Host "Installed VEXA $resolvedVersion to '$versionDir'."
    if ($null -ne $previousVersion -and $previousVersion -ne $resolvedVersion) {
        Write-Host "Previous version $previousVersion remains installed for rollback."
    }
    Write-Host "Open a new terminal if this terminal did not previously include '$InstallRoot' on PATH."
} finally {
    if (Test-Path -LiteralPath $tempRoot) {
        $tempRootFull = [IO.Path]::GetFullPath($tempRoot)
        $tempBaseFull = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\', '/')
        if (-not $tempRootFull.StartsWith($tempBaseFull + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
            throw "Refusing to recursively remove temporary path outside the system temp directory: '$tempRootFull'."
        }
        Remove-Item -LiteralPath $tempRoot -Recurse -Force
    }
}
