# dlm gates: run every check with one command.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File .\gates.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File .\gates.ps1 -SkipIntegration
#
# Exit code 0 = all green; 1 = at least one gate failed.
param(
    [switch]$SkipIntegration
)

$ErrorActionPreference = 'Continue'
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
Push-Location $Root
$script:Failed = 0
$script:Results = @()

function Invoke-Gate {
    param(
        [string]$Name,
        [string]$Note,
        [scriptblock]$Body
    )

    Write-Host ''
    Write-Host "=== $Name ===" -ForegroundColor Cyan
    $out = & $Body 2>&1 | ForEach-Object { $_ }
    $code = $LASTEXITCODE
    $out | ForEach-Object { Write-Host $_ }

    $passed = $null
    $sum = 0
    $seen = $false
    foreach ($line in $out) {
        if ($line -match 'test result: ok\. (\d+) passed') {
            $sum += [int]$Matches[1]
            $seen = $true
        }
    }
    if ($seen) { $passed = $sum }

    if ($code -eq 0) {
        $detail = if ($null -ne $passed) { "$passed passed" } else { $Note }
        $script:Results += [pscustomobject]@{ Gate = $Name; Result = 'PASS'; Detail = $detail }
    }
    else {
        $script:Failed++
        $script:Results += [pscustomobject]@{ Gate = $Name; Result = 'FAIL'; Detail = "exit $code" }
    }
}

Invoke-Gate -Name 'fmt' -Note 'cargo fmt --check' -Body { cargo fmt --check }
Invoke-Gate -Name 'lib' -Note 'cargo test --lib' -Body { cargo test --lib }
Invoke-Gate -Name 'bins' -Note 'cargo test --bins' -Body { cargo test --bins }

if ($SkipIntegration) {
    $script:Results += [pscustomobject]@{ Gate = 'integration'; Result = 'SKIP'; Detail = '-SkipIntegration' }
}
else {
    Invoke-Gate -Name 'integration' -Note 'cargo test --test integration' -Body { cargo test --test integration }
}

Invoke-Gate -Name 'clippy' -Note 'cargo clippy --all-targets -D warnings' -Body { cargo clippy --all-targets -- -D warnings }

$node = Get-Command node -ErrorAction SilentlyContinue
if ($node) {
    Invoke-Gate -Name 'extension' -Note 'check-extension-static.mjs' -Body { node browser/test/check-extension-static.mjs }
}
else {
    $script:Results += [pscustomobject]@{ Gate = 'extension'; Result = 'SKIP'; Detail = 'node not found' }
}

Write-Host ''
Write-Host '=== summary ===' -ForegroundColor Cyan
$script:Results | Format-Table -AutoSize | Out-String | Write-Host

if ($script:Failed -gt 0) {
    Write-Host "GATES FAILED: $script:Failed" -ForegroundColor Red
    Pop-Location
    exit 1
}
Write-Host 'GATES PASSED' -ForegroundColor Green
Pop-Location
exit 0
