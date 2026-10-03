param()

$ErrorActionPreference = 'Stop'

$benchmarksRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$failures = New-Object 'System.Collections.Generic.List[string]'

function Add-Failure {
    param([string]$Message)
    $script:failures.Add($Message)
}

function Read-BenchmarkJson {
    param([string]$Path)

    try {
        return Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
    }
    catch {
        Add-Failure "Invalid JSON: $Path ($($_.Exception.Message))"
        return $null
    }
}

function Resolve-BenchmarkPath {
    param([string]$RelativePath)
    return [System.IO.Path]::GetFullPath((Join-Path $benchmarksRoot $RelativePath))
}

$jsonFiles = Get-ChildItem -LiteralPath $benchmarksRoot -Recurse -File -Filter '*.json'
foreach ($jsonFile in $jsonFiles) {
    [void](Read-BenchmarkJson -Path $jsonFile.FullName)
}

$suitePath = Join-Path $benchmarksRoot 'benchmark-suite.v1.json'
$suite = Read-BenchmarkJson -Path $suitePath
if ($null -eq $suite) {
    throw 'Benchmark suite could not be parsed.'
}

if ($suite.schema_version -ne 1) {
    Add-Failure 'Suite schema_version must be 1.'
}
if ($suite.runner_contract.manifest_contains_measurements -ne $false) {
    Add-Failure 'Suite manifest must not contain benchmark measurements.'
}

$taskSchemaPath = Resolve-BenchmarkPath -RelativePath $suite.task_schema
if (-not (Test-Path -LiteralPath $taskSchemaPath -PathType Leaf)) {
    Add-Failure "Missing task schema: $($suite.task_schema)"
}

$knownCategories = @('retrieval', 'adversarial', 'freshness')
$knownLanguages = @('rust', 'python', 'typescript', 'javascript', 'java', 'c', 'cpp', 'csharp', 'go', 'mixed')
$knownScenarios = @('single_query', 'index_then_mutate', 'symlink_escape')
$taskIds = @{}

foreach ($taskRelativePath in $suite.tasks) {
    $taskPath = Resolve-BenchmarkPath -RelativePath $taskRelativePath
    if (-not (Test-Path -LiteralPath $taskPath -PathType Leaf)) {
        Add-Failure "Missing task file: $taskRelativePath"
        continue
    }

    $task = Read-BenchmarkJson -Path $taskPath
    if ($null -eq $task) {
        continue
    }

    if ($task.schema_version -ne 1) {
        Add-Failure "${taskRelativePath}: schema_version must be 1."
    }
    if ([string]::IsNullOrWhiteSpace([string]$task.id)) {
        Add-Failure "${taskRelativePath}: missing id."
    }
    elseif ($taskIds.ContainsKey([string]$task.id)) {
        Add-Failure "${taskRelativePath}: duplicate task id '$($task.id)'."
    }
    else {
        $taskIds[[string]$task.id] = $true
    }

    if ($knownCategories -notcontains [string]$task.category) {
        Add-Failure "${taskRelativePath}: unknown category '$($task.category)'."
    }
    if ($knownLanguages -notcontains [string]$task.language) {
        Add-Failure "${taskRelativePath}: unknown language '$($task.language)'."
    }
    if ($knownScenarios -notcontains [string]$task.scenario.kind) {
        Add-Failure "${taskRelativePath}: unknown scenario '$($task.scenario.kind)'."
    }
    if ([string]::IsNullOrWhiteSpace([string]$task.prompt)) {
        Add-Failure "${taskRelativePath}: prompt must be non-empty."
    }

    $fixtureRepo = Resolve-BenchmarkPath -RelativePath ([string]$task.fixture_repo)
    if (-not (Test-Path -LiteralPath $fixtureRepo -PathType Container)) {
        Add-Failure "${taskRelativePath}: missing fixture repo '$($task.fixture_repo)'."
        continue
    }

    $mutationSource = $null
    $mutationTarget = $null
    if ($task.scenario.kind -eq 'index_then_mutate') {
        $mutationTarget = [string]$task.scenario.mutation.target_path
        $mutationSource = Resolve-BenchmarkPath -RelativePath ([string]$task.scenario.mutation.source_path)
        if (-not (Test-Path -LiteralPath $mutationSource -PathType Leaf)) {
            Add-Failure "${taskRelativePath}: missing mutation source '$($task.scenario.mutation.source_path)'."
        }
        if ($task.scenario.mutation.after_step -ne 'index' -or $task.scenario.mutation.operation -ne 'replace_file') {
            Add-Failure "${taskRelativePath}: stale-index scenario must replace_file after index."
        }
    }

    if ($task.scenario.kind -eq 'symlink_escape') {
        $targetFromRepo = [System.IO.Path]::GetFullPath((Join-Path $fixtureRepo ([string]$task.scenario.symlink.target_path)))
        if (-not (Test-Path -LiteralPath $targetFromRepo -PathType Leaf)) {
            Add-Failure "${taskRelativePath}: missing symlink target '$($task.scenario.symlink.target_path)'."
        }
        if (@($task.scenario.symlink.skip_if) -notcontains 'windows_symlink_creation_unsupported') {
            Add-Failure "${taskRelativePath}: Windows symlink privilege/unsupported case must be skippable."
        }
    }

    $relevantFiles = @($task.expected.relevant_files)
    $relevantSymbols = @($task.expected.relevant_symbols)
    if ($relevantFiles.Count -eq 0) {
        Add-Failure "${taskRelativePath}: expected.relevant_files must not be empty."
    }
    if ($relevantSymbols.Count -eq 0) {
        Add-Failure "${taskRelativePath}: expected.relevant_symbols must not be empty."
    }

    foreach ($relativeFile in $relevantFiles) {
        $fixtureFile = [System.IO.Path]::GetFullPath((Join-Path $fixtureRepo ([string]$relativeFile)))
        if (-not (Test-Path -LiteralPath $fixtureFile -PathType Leaf)) {
            Add-Failure "${taskRelativePath}: expected file not found in fixture repo: $relativeFile"
        }
    }

    foreach ($symbolExpectation in $relevantSymbols) {
        $symbolPath = [string]$symbolExpectation.path
        $symbol = [string]$symbolExpectation.symbol
        if ($relevantFiles -notcontains $symbolPath) {
            Add-Failure "${taskRelativePath}: symbol '$symbol' points to '$symbolPath', which is not an expected relevant file."
        }

        $contentPath = [System.IO.Path]::GetFullPath((Join-Path $fixtureRepo $symbolPath))
        if ($task.scenario.kind -eq 'index_then_mutate' -and $symbolPath -eq $mutationTarget) {
            $contentPath = $mutationSource
        }
        if ($null -ne $contentPath -and (Test-Path -LiteralPath $contentPath -PathType Leaf)) {
            $content = Get-Content -LiteralPath $contentPath -Raw
            if ($content.IndexOf($symbol, [System.StringComparison]::Ordinal) -lt 0) {
                Add-Failure "${taskRelativePath}: expected symbol '$symbol' not found in source for '$symbolPath'."
            }
        }
    }
}

$giantLog = Join-Path $benchmarksRoot 'fixtures/adversarial/logs/build.log'
if (-not (Test-Path -LiteralPath $giantLog -PathType Leaf)) {
    Add-Failure 'Missing giant-log fixture.'
}
elseif ((Get-Item -LiteralPath $giantLog).Length -lt 131072) {
    Add-Failure 'Giant-log fixture must be at least 128 KiB so it remains meaningfully larger than source fixtures.'
}

if ($failures.Count -gt 0) {
    foreach ($failure in $failures) {
        Write-Error $failure
    }
    exit 1
}

Write-Host "Validated $($jsonFiles.Count) JSON files and $($suite.tasks.Count) benchmark tasks."
Write-Host 'No benchmark measurements were produced.'
