param(
    [Parameter(Mandatory = $true)]
    [string]$Config,

    [string]$Binary = ".\\target\\debug\\go-on.exe"
)

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RootDir = Resolve-Path (Join-Path $ScriptDir "..")

Write-Host "=== BLUE15 Stage C release readiness gate ==="
Write-Host "=== 1) Release readiness scenario replay ==="
& (Join-Path $ScriptDir "run-request.ps1") -Config $Config -Template (Join-Path $RootDir "requests/release-readiness-drill.ndjson") -Binary $Binary

Write-Host "=== 2) Integration assertions ==="
cargo test run_scenario_file_executes_release_readiness_drill_requests -- --nocapture
cargo test rpc_shutdown_waits_for_inflight_chat_completion -- --nocapture
cargo test ndjson_scenario_files_all_pass -- --nocapture

Write-Host "✅ BLUE15 Stage C release readiness gate completed"
