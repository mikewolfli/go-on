param(
    [Parameter(Mandatory = $true)]
    [string]$Config,

    [string]$Binary = ".\\target\\debug\\go-on.exe",

    [string]$OutputFile = ".\\RELEASE_GATE_OUTPUT.txt"
)

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RootDir = Resolve-Path (Join-Path $ScriptDir "..")
$Timestamp = Get-Date -Format "yyyy-MM-ddTHH:mm:ssZ"

# ── Secret resolution: prefer keyring, fall back to env var (documented policy) ──
# keyring://go-on/deepseek_api_key is the canonical reference.
# On machines where keyring is unavailable, DEEPSEEK_API_KEY env var is the
# authorised fallback for CI / developer workstations. This is intentional policy,
# not a workaround — pure-keyring enforcement is a deployment-time hardening step
# that requires a real secret to be written to the platform keychain.
if (-not (Test-Path env:DEEPSEEK_API_KEY)) {
    # If keyring CLI is present, attempt to read the secret directly.
    # Failure here is non-fatal; the binary itself will attempt keyring then env.
    $keyringVal = $null
    try {
        $keyringVal = & keyring get go-on deepseek_api_key 2>$null
    }
    catch {}
    if ($keyringVal) {
        $env:DEEPSEEK_API_KEY = $keyringVal.Trim()
        Write-Host "[gate] Secret resolved via keyring"
    }
    else {
        Write-Host "[gate] DEEPSEEK_API_KEY not set and keyring read returned empty — binary will attempt its own keyring->env fallback"
    }
}

$Results = @()
$OverallPass = $true

function Run-Gate-Step {
    param([string]$Label, [scriptblock]$Block)
    Write-Host ""
    Write-Host "=== $Label ==="
    $start = Get-Date
    try {
        & $Block
        $exitCode = $LASTEXITCODE
    }
    catch {
        $exitCode = 1
        Write-Host "ERROR: $_"
    }
    $elapsed = ((Get-Date) - $start).TotalSeconds
    $pass = ($exitCode -eq 0 -or $null -eq $exitCode)
    $script:Results += [PSCustomObject]@{ Label = $Label; Pass = $pass; ElapsedSec = [math]::Round($elapsed, 1) }
    if (-not $pass) { $script:OverallPass = $false }
    if ($pass) {
        Write-Host "[PASS] $Label (exit=$exitCode, ${elapsed}s)"
    }
    else {
        Write-Host "[FAIL] $Label (exit=$exitCode, ${elapsed}s)"
    }
}

Write-Host "=== BLUE26 Release Readiness Gate ==="
Write-Host "=== Config: $Config | Timestamp: $Timestamp ==="

# ── 1. Build check ────────────────────────────────────────────────────────────
Run-Gate-Step "cargo check --all-targets" { cargo check --all-targets }

# ── 2. BLUE26 integration assertions ─────────────────────────────────────────
Run-Gate-Step "integration: release.readiness benchmark" {
    cargo test --test acp_runtime_rpc_integration run_scenario_file_executes_release_readiness_benchmark_requests -- --nocapture
}
Run-Gate-Step "integration: governance benchmark" {
    cargo test --test acp_runtime_rpc_integration run_scenario_file_executes_governance_dynamic_rules_benchmark_requests -- --nocapture
}
Run-Gate-Step "integration: managed-service inference" {
    cargo test --test acp_runtime_rpc_integration managed_service_target_infers_multi_user_mode_on_main_chain -- --nocapture
}
Run-Gate-Step "integration: adversarial negative paths" {
    cargo test --test acp_runtime_rpc_integration adversarial_ -- --nocapture
}
Run-Gate-Step "integration: release readiness drill" {
    cargo test --test acp_runtime_rpc_integration run_scenario_file_executes_release_readiness_drill_requests -- --nocapture
}
Run-Gate-Step "integration: multi-user lifecycle drill" {
    cargo test --test acp_runtime_rpc_integration run_scenario_file_executes_multi_user_lifecycle_drill_requests -- --nocapture
}
Run-Gate-Step "integration: task.execute task-graph-checkpoint resume" {
    cargo test --test acp_runtime_rpc_integration task_execute_returns_task_graph_checkpoint -- --nocapture
}
Run-Gate-Step "integration: tool-loop safety-governance S12" {
    cargo test --test acp_runtime_rpc_integration task_execute_returns_tool_loop_safety_governance -- --nocapture
}
Run-Gate-Step "integration: role-collaboration conflict-resolution S13" {
    cargo test --test acp_runtime_rpc_integration task_execute_returns_role_handoff_conflict_resolution -- --nocapture
}
Run-Gate-Step "integration: shutdown inflight" {
    cargo test --test acp_runtime_rpc_integration rpc_shutdown_waits_for_inflight_chat_completion -- --nocapture
}
Run-Gate-Step "integration: ndjson all pass" {
    cargo test --test acp_runtime_rpc_integration ndjson_scenario_files_all_pass -- --nocapture
}

# ── 2b. BLUE26 closure consistency guard ─────────────────────────────────────
Run-Gate-Step "docs/contract: BLUE26 closure consistency" {
    & .\scripts\verify-blue26-closure.ps1
}

# ── 3. VS Code addon ──────────────────────────────────────────────────────────
Run-Gate-Step "addon: compile + lint" { npm --prefix vscode-addon run check }
Run-Gate-Step "addon: contract smoke" { node vscode-addon/scripts/contract-smoke.js }

# ── 4. GUI ────────────────────────────────────────────────────────────────────
Run-Gate-Step "GUI: cargo check" {
    Push-Location .\gui
    try {
        cargo check --all-targets
    }
    finally {
        Pop-Location
    }
}
Run-Gate-Step "GUI: cargo test" {
    Push-Location .\gui
    try {
        cargo test --all-targets
    }
    finally {
        Pop-Location
    }
}

# ── 5. Output artifact ────────────────────────────────────────────────────────
$PassCount = ($Results | Where-Object { $_.Pass }).Count
$FailCount = ($Results | Where-Object { -not $_.Pass }).Count
$Summary = $Results | Format-Table Label, Pass, ElapsedSec -AutoSize | Out-String

$OutputContent = @"
BLUE26 Release Gate Output
Generated: $Timestamp
Config:    $Config
Overall:   $(if ($OverallPass) { 'PASS' } else { 'FAIL' })
Pass:      $PassCount / $($Results.Count)
Fail:      $FailCount / $($Results.Count)

Secret policy: keyring://go-on/deepseek_api_key (canonical).
  Fallback: DEEPSEEK_API_KEY env var is authorised for CI/dev workstations.
  Pure-keyring enforcement requires a real secret written to the platform keychain.

Step Results:
$Summary
"@

$OutputContent | Set-Content -Path $OutputFile -Encoding UTF8
Write-Host ""
Write-Host "=== Gate output written to: $OutputFile ==="
if ($OverallPass) {
    Write-Host "[PASS] BLUE26 Release Gate: PASS"
}
else {
    Write-Host "[FAIL] BLUE26 Release Gate: FAIL"
}

if (-not $OverallPass) {
    exit 1
}
