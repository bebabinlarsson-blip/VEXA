[CmdletBinding()]
param(
    [string]$Target = "x86_64-pc-windows-msvc",
    [string]$OutputDir = "dist",
    [switch]$SkipBuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if ($Target -ne "x86_64-pc-windows-msvc") {
    throw "Unsupported Windows release target: $Target"
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Push-Location $repoRoot
try {
    $metadata = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    $package = $metadata.packages | Where-Object { $_.name -eq "vexa-cli" } | Select-Object -First 1
    if (-not $package) {
        throw "Could not determine vexa-cli version from cargo metadata"
    }
    $version = $package.version

    if (-not $SkipBuild) {
        cargo build --locked --release --package vexa-boot --package vexa-cli --target $Target
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }
    }

    $bootBinary = Join-Path $repoRoot "target\$Target\release\vexa-boot.exe"
    $coreBinary = Join-Path $repoRoot "target\$Target\release\vexa-core.exe"
    foreach ($binary in @($bootBinary, $coreBinary)) {
        if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
            throw "Expected release binary was not found: $binary"
        }
    }

    $outputDirAbs = if ([IO.Path]::IsPathRooted($OutputDir)) {
        [IO.Path]::GetFullPath($OutputDir)
    } else {
        [IO.Path]::GetFullPath((Join-Path $repoRoot $OutputDir))
    }
    $packageName = "vexa-$version-$Target"
    $outputDirAbs = [IO.Path]::GetFullPath($outputDirAbs)
    $stageRoot = [IO.Path]::GetFullPath((Join-Path $outputDirAbs ".stage-$packageName"))
    $stageDir = Join-Path $stageRoot $packageName
    $archive = Join-Path $outputDirAbs "$packageName.zip"

    New-Item -ItemType Directory -Path $outputDirAbs -Force | Out-Null
    if ([IO.Path]::GetDirectoryName($stageRoot) -ne $outputDirAbs -or $stageRoot -eq $outputDirAbs) {
        throw "Refusing to recursively remove staging path outside the output directory: '$stageRoot'."
    }
    if (Test-Path -LiteralPath $stageRoot) {
        Remove-Item -LiteralPath $stageRoot -Recurse -Force
    }
    New-Item -ItemType Directory -Path $stageDir -Force | Out-Null

    try {
        Copy-Item -LiteralPath $bootBinary -Destination (Join-Path $stageDir "vexa-boot.exe")
        Copy-Item -LiteralPath $coreBinary -Destination (Join-Path $stageDir "vexa-core.exe")
        Copy-Item -LiteralPath (Join-Path $repoRoot "LICENSE") -Destination (Join-Path $stageDir "LICENSE")
        Copy-Item -LiteralPath (Join-Path $repoRoot "THIRD_PARTY_LICENSES.txt") -Destination (Join-Path $stageDir "THIRD_PARTY_LICENSES.txt")

        if (Test-Path -LiteralPath $archive) {
            Remove-Item -LiteralPath $archive -Force
        }
        # Use fast archive creation for release binaries.
        Add-Type -AssemblyName System.IO.Compression
        $archiveStream = [IO.File]::Open($archive, [IO.FileMode]::CreateNew, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
        $zip = $null
        try {
            $zip = [IO.Compression.ZipArchive]::new($archiveStream, [IO.Compression.ZipArchiveMode]::Create, $false)
            foreach ($file in Get-ChildItem -LiteralPath $stageRoot -File -Recurse) {
                $relativePath = $file.FullName.Substring($stageRoot.Length + 1).Replace('\', '/')
                $entry = $zip.CreateEntry($relativePath, [IO.Compression.CompressionLevel]::NoCompression)
                $entryStream = $entry.Open()
                $sourceStream = [IO.File]::OpenRead($file.FullName)
                try { $sourceStream.CopyTo($entryStream) }
                finally { $sourceStream.Dispose(); $entryStream.Dispose() }
            }
        }
        finally {
            if ($null -ne $zip) { $zip.Dispose() }
            $archiveStream.Dispose()
        }
    }
    finally {
        if (Test-Path -LiteralPath $stageRoot) {
            if ([IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($stageRoot)) -ne $outputDirAbs) {
                throw "Refusing to recursively remove staging path outside the output directory: '$stageRoot'."
            }
            Remove-Item -LiteralPath $stageRoot -Recurse -Force
        }
    }

    Write-Output $archive
}
finally {
    Pop-Location
}
