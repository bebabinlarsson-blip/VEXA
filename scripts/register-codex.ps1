$ErrorActionPreference = 'Stop'

$launcher = Get-Command vexa -ErrorAction SilentlyContinue
if (-not $launcher) {
    throw "The 'vexa' executable is not on PATH. No Codex configuration was changed."
}

& $launcher.Source hook codex-register
if ($LASTEXITCODE -ne 0) {
    throw "VEXA Codex hook registration failed with exit code $LASTEXITCODE."
}
