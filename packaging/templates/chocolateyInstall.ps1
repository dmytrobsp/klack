# Rendered by packaging/render.py from the release's SHA256SUMS (github.com/dmytrobsp/ttyak); edit the template.
# Unzips the release into this package's tools folder; Chocolatey shims the ttyak.exe it finds there.
$ErrorActionPreference = 'Stop'
$toolsDir = Split-Path -Parent $MyInvocation.MyCommand.Definition

$packageArgs = @{
  packageName    = $env:ChocolateyPackageName
  unzipLocation  = $toolsDir
  url64bit       = {{url_x86_64_windows}}
  checksum64     = {{sha256_x86_64_windows}}
  checksumType64 = 'sha256'
}
Install-ChocolateyZipPackage @packageArgs
