param(
    [string]$BaseUrl = "http://127.0.0.1:8080"
)

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RootDir = Resolve-Path (Join-Path $ScriptDir "..")
$SettingsFile = Join-Path $RootDir ".zed/settings.json"
$DocEn = Join-Path $RootDir "docs/en/src/zed.md"
$DocZh = Join-Path $RootDir "docs/zh-CN/src/zed.md"

function Pass([string]$Message) { Write-Host "[PASS] $Message" }
function Fail([string]$Message) { Write-Host "[FAIL] $Message"; exit 1 }
function Step([string]$Message) { Write-Host "== $Message ==" }

Step "Zed integration file checks"
if (-not (Test-Path $SettingsFile)) { Fail "missing .zed/settings.json" }
if (-not (Test-Path $DocEn)) { Fail "missing DOC/en/src/zed.md" }
if (-not (Test-Path $DocZh)) { Fail "missing DOC/zh-CN/src/zed.md" }
Pass "required Zed files exist"

Step "Workspace settings schema checks"
$settingsText = Get-Content -Path $SettingsFile -Raw
if ($settingsText -notmatch '"agent_servers"') { Fail "agent_servers missing" }
if ($settingsText -notmatch '"language_models"') { Fail "language_models missing" }
if ($settingsText -notmatch '"openai_compatible"') { Fail "openai_compatible provider missing" }
if ($settingsText -notmatch '"available_models"') { Fail "available_models missing" }
if ($settingsText -notmatch '"gpt-5\.5"') { Fail "gpt-5.5 model entry missing" }
Pass "workspace settings structure is valid"

Step "Docs consistency checks"
$docEnText = Get-Content -Path $DocEn -Raw
$docZhText = Get-Content -Path $DocZh -Raw
if ($docEnText -notmatch 'openai_compatible') { Fail "EN doc does not mention openai_compatible" }
if ($docZhText -notmatch 'openai_compatible') { Fail "ZH doc does not mention openai_compatible" }
if ($docEnText -notmatch 'type:\s*custom|"type"\s*:\s*"custom"') { Fail "EN doc does not mention custom provider type" }
if ($docZhText -notmatch 'type:\s*custom|"type"\s*:\s*"custom"') { Fail "ZH doc does not mention custom provider type" }
Pass "docs include current provider guidance"

Step "Local endpoint smoke checks (optional)"
try {
    $response = Invoke-WebRequest -Uri "$BaseUrl/v1/models" -Method Get -TimeoutSec 3 -ErrorAction Stop
    if ($response.StatusCode -ge 200 -and $response.StatusCode -lt 400) {
        Pass "endpoint reachable: $BaseUrl/v1/models"
    }
    else {
        Write-Host "[WARN] endpoint returned status $($response.StatusCode)"
    }
}
catch {
    Write-Host "[WARN] endpoint not reachable at $BaseUrl/v1/models (start server to enable runtime verification)"
}

Step "Result"
Pass "Zed integration baseline verification completed"
