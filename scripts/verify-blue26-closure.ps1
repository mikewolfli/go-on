Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$rootDir = Resolve-Path (Join-Path $scriptDir "..")

$contractPath = Join-Path $rootDir "contracts/editor-capability-matrix.json"
$blue26Path = Join-Path $rootDir "docs/blueprints/blue26.md"
$addonSmokePath = Join-Path $rootDir "vscode-addon/scripts/contract-smoke.js"
if (-not (Test-Path $blue26Path)) { throw "blue26.md not found" }
if (-not (Test-Path $contractPath)) { throw "editor-capability-matrix.json not found" }
if (-not (Test-Path $addonSmokePath)) { throw "vscode addon contract-smoke.js not found" }

$blue26 = Get-Content -Path $blue26Path -Raw -Encoding UTF8
$addonSmoke = Get-Content -Path $addonSmokePath -Raw -Encoding UTF8
$contract = Get-Content -Path $contractPath -Raw -Encoding UTF8 | ConvertFrom-Json

$errors = New-Object System.Collections.Generic.List[string]

if ($blue26 -notmatch '100%') {
    $errors.Add('blue26.md must declare 100% completion')
}

if ($blue26 -notmatch 'S0-S41') {
    $errors.Add('blue26.md must include S0-S41 completion scope')
}

if ($blue26 -match '\u2B1C\s*\|') {
    $errors.Add('blue26.md still contains unchecked box markers for pending items')
}

foreach ($sid in @('B26-S39','B26-S40','B26-S41')) {
    if ($blue26 -notmatch [Regex]::Escape("| $sid |")) {
        $errors.Add("blue26.md row for $sid is missing")
    }
}

foreach ($token in @('capability_consistency_mainchain', 'shared_learning_data_flow', 'self_evolution_flow')) {
    if ($blue26 -notmatch [Regex]::Escape($token)) {
        $errors.Add("blue26.md must include closure writeback token: $token")
    }
}

$requiredContractFlags = @(
    'blue26S39CapabilityConsistencyMainChainCheckedInMainChain',
    'blue26S40SharedLearningDataFlowCheckedInMainChain',
    'blue26S41SelfEvolutionFlowCheckedInMainChain'
)

foreach ($flag in $requiredContractFlags) {
    $value = $contract.protocol.$flag
    if ($null -eq $value -or $value -ne $true) {
        $errors.Add("contract.protocol.$flag must be true")
    }
}

# contract-smoke.js formats these assertions across multiple lines
# (e.g. `assert.equal(\n  contract.protocol.<flag>,\n  true,\n);`), so match
# with flexible whitespace instead of a single-line literal.
foreach ($flag in $requiredContractFlags) {
    $pattern = 'assert\.equal\(\s*contract\.protocol\.' + [Regex]::Escape($flag) + '\s*,\s*true\s*\)'
    if ($addonSmoke -notmatch $pattern) {
        $errors.Add("vscode addon smoke missing assertion: $flag")
    }
}

if ($errors.Count -gt 0) {
    Write-Host '[FAIL] BLUE26 closure consistency check failed:'
    foreach ($msg in $errors) {
        Write-Host " - $msg"
    }
    exit 1
}

Write-Host '[PASS] BLUE26 closure consistency check passed'
