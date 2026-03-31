param(
    [Parameter(Mandatory = $true)]
    [string]$Config,

    [Parameter(Mandatory = $true)]
    [string]$Template,

    [string]$Binary = ".\\target\\debug\\go-on.exe"
)

if (-not (Test-Path $Binary)) {
    throw "Binary not found: $Binary"
}
if (-not (Test-Path $Config)) {
    throw "Config not found: $Config"
}
if (-not (Test-Path $Template)) {
    throw "Template not found: $Template"
}

Get-Content $Template | & $Binary --config $Config

