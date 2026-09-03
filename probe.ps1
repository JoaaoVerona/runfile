# Runfile Phase 0 - Windows shell discovery probe.
#   powershell -ExecutionPolicy Bypass -File probe.ps1
# Requires probe.sh alongside it. ASCII only, CRLF, no here-strings: PowerShell
# 5.1 reads .ps1 as ANSI without a BOM, and its here-string terminator is
# line-ending sensitive. v1 and v2 tripped on both.

Write-Host ""
Write-Host "=== candidates, in resolution order ==="
Write-Host ""

# mirrors runfile-shell/src/resolve.rs :: git_bash_known_paths()
$candidates = @(
  "$env:ProgramFiles\Git\bin\bash.exe",
  "${env:ProgramFiles(x86)}\Git\bin\bash.exe",
  "$env:LOCALAPPDATA\Programs\Git\bin\bash.exe",
  "C:\Git\bin\bash.exe"
)

$found = $null
foreach ($c in $candidates) {
  if (Test-Path -LiteralPath $c) {
    Write-Host "  FOUND  $c"
    if (-not $found) { $found = $c }
  } else {
    Write-Host "  -      $c"
  }
}

Write-Host ""
Write-Host "=== the WSL trap: every bash.exe on PATH ==="
$onPath = @(Get-Command bash.exe -All -ErrorAction SilentlyContinue)
if ($onPath.Count -eq 0) {
  Write-Host "  (none on PATH)"
}
foreach ($g in $onPath) {
  if ($g.Source -like "$env:SystemRoot\System32\*") {
    Write-Host "  TRAP   $($g.Source)"
    Write-Host "         ^ WSL launcher. resolve.rs correctly excludes this."
  } else {
    Write-Host "  ok     $($g.Source)"
  }
}

if (-not $found) {
  Write-Host ""
  Write-Host "No Git Bash found. This is the case the fallback has to cover."
  exit 1
}

Write-Host ""
Write-Host "=== running checks in: $found ==="
$script = Join-Path $PSScriptRoot "probe.sh"
if (-not (Test-Path -LiteralPath $script)) {
  Write-Host "  probe.sh not found next to this script"
  exit 1
}
& $found $script
