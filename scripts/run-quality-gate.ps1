param(
    # Kept for back-compat with earlier invocations; unused by the gate
    # itself (mirrors run-quality-gate.sh, which also accepts a config path
    # but only runs prompt validation + the lib test regression gate).
    [string]$Config,

    [string]$Binary = ".\\target\\debug\\go-on.exe"
)

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RootDir = Resolve-Path (Join-Path $ScriptDir "..")

Write-Host "=== BLUE15 P3-1 quality gate: prompt validation + regression checks ==="
Write-Host "=== Validating prompt templates ==="

# validate-prompts.sh is a bash script (python3 heredoc); run it when bash is
# available, otherwise skip with a warning so the gate degrades gracefully.
$bash = Get-Command bash -ErrorAction SilentlyContinue
if ($null -ne $bash) {
    & (Join-Path $ScriptDir "validate-prompts.sh")
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
else {
    Write-Host "[WARN] bash not found - skipping prompt template validation (run scripts/validate-prompts.sh on a POSIX shell)"
}

# Regression gate: the generated `requests/quality-benchmark.ndjson` scenario
# is not part of the repo, so run the lib test suite as the regression gate.
Write-Host "=== Running lib test suite (regression) ==="
cargo test --lib
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Write-Host "Test run completed"

Write-Host "✅ BLUE15 P3-1 quality gate completed"
