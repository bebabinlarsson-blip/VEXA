$ErrorActionPreference = 'Stop'

$launcher = Get-Command vexa -ErrorAction SilentlyContinue
if (-not $launcher) {
    throw "The 'vexa' executable is not on PATH. No Codex configuration was changed."
}

& $launcher.Source hook codex-unregister
if ($LASTEXITCODE -ne 0) {
    throw "VEXA Codex hook removal failed with exit code $LASTEXITCODE."
}
