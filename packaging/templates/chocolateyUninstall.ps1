# Rendered by packaging/render.py from the release's SHA256SUMS (github.com/dmytrobsp/ttyak); edit the template.
# Deletes ttyak's data (settings, sign-ins, drafts, caches) before Chocolatey removes the program. Chocolatey runs this
# on `choco uninstall` only (an upgrade runs chocolateyBeforeModify.ps1), elevated: the data is that of the user who
# ran choco.
$ErrorActionPreference = 'Stop'
$toolsDir = Split-Path -Parent $MyInvocation.MyCommand.Definition

& (Join-Path $toolsDir 'ttyak.exe') purge --yes
if ($LASTEXITCODE -ne 0) {
  Write-Warning "ttyak's data was not deleted: delete $env:APPDATA\ttyak and $env:LOCALAPPDATA\ttyak yourself."
}
