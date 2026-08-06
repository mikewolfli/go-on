param(
    [string]$BaseUrl = "http://127.0.0.1:8090"
)

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RootDir = Resolve-Path (Join-Path $ScriptDir "..")
$SettingsFile = Join-Path $RootDir ".zed/settings.json"
$DocEn = Join-Path $RootDir "cookbook/src/en/zed.md"
$DocZh = Join-Path $RootDir "cookbook/src/zh-CN/zed.md"

function Pass([string]$Message) { Write-Host "[PASS] $Message" }
function Fail([string]$Message) { Write-Host "[FAIL] $Message"; exit 1 }
function Step([string]$Message) { Write-Host "== $Message ==" }

Step "Zed integration file checks"
if (-not (Test-Path $SettingsFile)) { Fail "missing .zed/settings.json" }
if (-not (Test-Path $DocEn)) { Fail "missing cookbook/src/en/zed.md" }
if (-not (Test-Path $DocZh)) { Fail "missing cookbook/src/zh-CN/zed.md" }
Pass "required Zed files exist"

Step "Workspace settings schema checks"
$settingsText = Get-Content -Path $SettingsFile -Raw
if ($settingsText -notmatch '"agent_servers"') { Fail "agent_servers missing" }
if ($settingsText -notmatch '"go-on"') { Fail "go-on agent server entry missing" }
if ($settingsText -notmatch '"auto_approve_tools"') { Fail "auto_approve_tools missing" }
Pass "workspace settings define the go-on agent server with auto_approve_tools"

Step "Docs consistency checks"
$docEnText = Get-Content -Path $DocEn -Raw
$docZhText = Get-Content -Path $DocZh -Raw
if ($docEnText -notmatch 'openai_compatible') { Fail "EN doc does not mention openai_compatible" }
if ($docZhText -notmatch 'openai_compatible') { Fail "ZH doc does not mention openai_compatible" }
if ($docEnText -notmatch '"type"\s*:\s*"custom"') { Fail "EN doc does not mention custom agent server type" }
if ($docZhText -notmatch '"type"\s*:\s*"custom"') { Fail "ZH doc does not mention custom agent server type" }
Pass "docs include current agent-server and provider guidance"

Step "Local endpoint smoke checks (optional)"
try {
    $response = Invoke-WebRequest -Uri "$BaseUrl/health" -Method Get -TimeoutSec 3 -ErrorAction Stop
    if ($response.StatusCode -ge 200 -and $response.StatusCode -lt 400) {
        Pass "endpoint reachable: $BaseUrl/health"
    }
    else {
        Write-Host "[WARN] endpoint returned status $($response.StatusCode)"
    }
}
catch {
    Write-Host "[WARN] endpoint not reachable at $BaseUrl/health (start server to enable runtime verification)"
}

Step "Result"
Pass "Zed integration baseline verification completed"
