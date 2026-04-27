param(
    [Parameter(Mandatory = $true)]
    [string]$Config,

    [string]$Binary = ".\\target\\debug\\go-on.exe"
)

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RootDir = Resolve-Path (Join-Path $ScriptDir "..")

Write-Host "=== BLUE15 P3-1 quality gate: request benchmark + regression checks ==="
& (Join-Path $ScriptDir "run-request.ps1") -Config $Config -Template (Join-Path $RootDir "requests/quality-benchmark.ndjson") -Binary $Binary

Write-Host "=== Running benchmark scenario integration regression ==="
cargo test run_scenario_file_executes_quality_benchmark_requests -- --nocapture
cargo test ndjson_scenario_files_all_pass -- --nocapture

$cargoList = cargo --list
if ($cargoList -match "\baudit\b|\btarpaulin\b") {
    if ($cargoList -match "\btarpaulin\b") {
        Write-Host "=== Optional coverage gate (tarpaulin) ==="
        cargo tarpaulin --out Stdout --fail-under 70
    }
    else {
        Write-Host "cargo-tarpaulin not installed, skipping optional coverage gate"
    }
}
else {
    Write-Host "cargo-tarpaulin not installed, skipping optional coverage gate"
}

Write-Host "✅ BLUE15 P3-1 quality gate completed"
