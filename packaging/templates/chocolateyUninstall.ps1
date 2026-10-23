# Rendered by packaging/render.py from the release's SHA256SUMS (github.com/dmytrobsp/ttyak); edit the template.
# Deletes ttyak's data (settings, sign-ins, drafts, caches) before Chocolatey removes the program. Chocolatey runs this
# on `choco uninstall` only (an upgrade runs chocolateyBeforeModify.ps1), elevated: the data is that of the user who
# ran choco.
# A purge that fails (ttyak still running, say) or writes to stderr never fails the uninstall: it warns.
$ErrorActionPreference = 'Continue'
$toolsDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$exe = Join-Path $toolsDir 'ttyak.exe'
$left = "ttyak's data was not deleted: run 'ttyak purge' if you install it again, or delete $env:APPDATA\ttyak and $env:LOCALAPPDATA\ttyak yourself."

try {
  $output = & $exe purge --yes 2>&1
  $code = $LASTEXITCODE
  $output | ForEach-Object { Write-Host $_ }
  if ($code -ne 0) {
    Write-Warning "ttyak purge exited with $code. $left"
  }
} catch {
  Write-Warning "ttyak purge could not run ($_). $left"
}
$global:LASTEXITCODE = 0
