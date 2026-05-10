# PowerShell sibling of test-all.sh — same behaviour, native on Windows.
#
# Usage:
#   .\scripts\test-all.ps1                 # everything
#   .\scripts\test-all.ps1 shamir-server   # one crate
#   .\scripts\test-all.ps1 -- --nocapture  # forward flags to cargo test
#   .\scripts\test-all.ps1 shamir-engine -- --test-threads=1
#
# Output mirrors test-all.sh: summary block + exit code 0 iff all green.

$ErrorActionPreference = 'Stop'

# Locate repo root relative to this script.
Set-Location (Join-Path $PSScriptRoot '..')

$logDir = 'target'
if (-not (Test-Path $logDir)) { New-Item -ItemType Directory -Path $logDir | Out-Null }
$log = Join-Path $logDir 'test-all.log'
'' | Out-File -FilePath $log -Encoding utf8

# Split args at `--`.
$crates = @()
$forward = @()
$seenDashDash = $false
foreach ($arg in $args) {
    if ($seenDashDash) {
        $forward += $arg
    } elseif ($arg -eq '--') {
        $seenDashDash = $true
    } else {
        $crates += $arg
    }
}

# Build cargo args.
if ($crates.Count -eq 0) {
    $cargoArgs = @('test', '--workspace', '--tests')
    $targetLabel = 'workspace'
} else {
    $cargoArgs = @('test', '--tests')
    foreach ($c in $crates) { $cargoArgs += @('-p', $c) }
    $targetLabel = ($crates -join ' ')
}
if ($forward.Count -gt 0) {
    $cargoArgs += '--'
    $cargoArgs += $forward
}

Write-Host "== test-all: $targetLabel ==" -ForegroundColor White
Write-Host "cargo $($cargoArgs -join ' ')`n"

$startTime = Get-Date

# Run cargo, tee output to log.
$tempErr = [System.IO.Path]::GetTempFileName()
try {
    & cargo @cargoArgs 2>$tempErr | Tee-Object -FilePath $log -Append
    $exitCode = $LASTEXITCODE
    Get-Content $tempErr | Tee-Object -FilePath $log -Append
} finally {
    Remove-Item $tempErr -ErrorAction SilentlyContinue
}

$elapsed = [int]((Get-Date) - $startTime).TotalSeconds

# Parse the transcript.
$passed = 0
$failed = 0
$ignored = 0
foreach ($line in Get-Content $log) {
    if ($line -match '^test result:.*?(\d+)\s+passed;\s+(\d+)\s+failed;\s+(\d+)\s+ignored') {
        $passed  += [int]$matches[1]
        $failed  += [int]$matches[2]
        $ignored += [int]$matches[3]
    }
}

Write-Host ""
Write-Host "── summary ──" -ForegroundColor White
Write-Host ("   target:   {0}" -f $targetLabel)
Write-Host ("   elapsed:  {0}s" -f $elapsed)
Write-Host ("   passed:   {0}" -f $passed)
Write-Host ("   failed:   {0}" -f $failed)
Write-Host ("   ignored:  {0}" -f $ignored)
Write-Host ("   log:      {0}" -f $log)

if ($exitCode -ne 0 -and $passed -eq 0 -and $failed -eq 0) {
    Write-Host "`ncargo failed before any tests could run (exit $exitCode)." -ForegroundColor Red
    Write-Host "Last 20 lines of log:"
    Get-Content $log -Tail 20
    exit $exitCode
}

if ($failed -gt 0) {
    Write-Host "`n$failed test(s) failed. See $log for details." -ForegroundColor Red
    exit 1
}

if ($exitCode -ne 0) {
    Write-Host "`ncargo exit code $exitCode but no test failures parsed." -ForegroundColor Yellow
    exit $exitCode
}

Write-Host "`nall green" -ForegroundColor Green
