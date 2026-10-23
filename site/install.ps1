# Installs ttyak for the current user on Windows, or updates it in place:
#   irm https://ttyak.sh/install.ps1 | iex
#   & ([scriptblock]::Create((irm https://ttyak.sh/install.ps1))) -Yes -Version 0.10.0 -Dir C:\tools\ttyak
# The archive comes from the GitHub release and must match the release's SHA256SUMS. `-Help` lists the flags.
# Windows PowerShell 5.1 or PowerShell 7. All of it runs inside Install-Ttyak, called on the last line: a download
# cut short runs nothing, and `irm | iex` leaves none of its variables in the session.

function Write-Usage {
    Write-Say @'
Installs ttyak into %LOCALAPPDATA%\Programs\ttyak, or updates the copy there.

usage: install.ps1 [-Yes] [-Version X.Y.Z] [-Dir PATH] [-NoModifyPath]
       install.ps1 -Uninstall [-Yes] [-KeepData] [-Dir PATH]

  -Yes            answer yes to every question (needed with no terminal for -Uninstall)
  -Version X.Y.Z  install this version instead of the latest
  -Dir PATH       install into PATH instead of %LOCALAPPDATA%\Programs\ttyak
  -NoModifyPath   leave the user PATH alone; say what to add instead
  -Uninstall      remove ttyak and its data (runs `ttyak uninstall`)
  -KeepData       with -Uninstall: keep settings, sign-ins, drafts and caches
  -Help           this text

`irm https://ttyak.sh/install.ps1 | iex` passes no flags: the environment variables TTYAK_YES=1, TTYAK_VERSION,
TTYAK_DIR, TTYAK_NO_MODIFY_PATH=1, TTYAK_UNINSTALL=1 and TTYAK_KEEP_DATA=1 do the same as the flags (the flags
win), or pass the flags like this:
  & ([scriptblock]::Create((irm https://ttyak.sh/install.ps1))) -Yes -Version 0.10.0
'@
}

# The installer's own output, for the user to read (never part of a pipeline).
function Write-Say {
    [Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '', Justification = 'messages for the user')]
    param([string]$Text)
    Write-Host $Text
}

# A warning or the reason the installer stopped, on stderr.
function Write-Complaint([string]$Text) {
    $Host.UI.WriteErrorLine("ttyak installer: $Text")
}

# The message of an error, without the "Exception calling ..." wrapping of a .NET call.
function Get-Reason($ErrorRecord) {
    $e = $ErrorRecord.Exception
    while ($e.InnerException) { $e = $e.InnerException }
    return $e.Message
}

# Whether an environment variable's value turns a flag on.
function Test-On([string]$Value) {
    return $Value -in @('1', 'true', 'yes')
}

# A switch: the flag when given, else its environment variable.
function Resolve-Switch($Bound, [string]$Name, [string]$Variable) {
    if ($Bound.ContainsKey($Name)) { return [bool]$Bound[$Name] }
    return Test-On ([Environment]::GetEnvironmentVariable($Variable))
}

# A value: the flag when given, else its environment variable.
function Resolve-Value($Bound, [string]$Name, [string]$Variable) {
    if ($Bound.ContainsKey($Name)) { return [string]$Bound[$Name] }
    return [string][Environment]::GetEnvironmentVariable($Variable)
}

# Whether $Value is a release version: X.Y.Z, or X.Y.Z-pre.
function Test-Version([string]$Value) {
    return $Value -cmatch '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?\z'
}

# The release platform name of this machine, as the archives name it; $Arch stands in for the CPU when given.
function Get-Platform([string]$Arch) {
    if (-not $Arch) {
        try { $Arch = [string][Runtime.InteropServices.RuntimeInformation]::OSArchitecture } catch { $Arch = '' }
    }
    # A 32-bit PowerShell on 64-bit Windows says x86 in PROCESSOR_ARCHITECTURE.
    if (-not $Arch) { $Arch = $env:PROCESSOR_ARCHITEW6432 }
    if (-not $Arch) { $Arch = $env:PROCESSOR_ARCHITECTURE }
    switch -Regex ($Arch) {
        '^(amd64|x64|x86_64)$' { return 'x86_64-windows' }
        '^(arm|aarch64)' { throw 'there is no Windows on ARM build of ttyak yet, only one for x64 Windows' }
        '^(x86|i[3-6]86)$' { throw 'ttyak has no build for 32-bit Windows, only one for x64 Windows' }
    }
    if (-not $Arch) { $Arch = 'this CPU' }
    throw "ttyak has no build for $Arch Windows, only one for x64 Windows"
}

# Whether this PowerShell runs as Administrator.
function Test-Elevated {
    try {
        $me = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
        return $me.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    } catch {
        return $false
    }
}

# Whether there is a terminal to ask on: stdin is not redirected and PowerShell was not started -NonInteractive.
function Test-Terminal {
    if ([Console]::IsInputRedirected) { return $false }
    foreach ($arg in [Environment]::GetCommandLineArgs()) {
        if ($arg -match '^[-/]noni') { return $false }
    }
    return $true
}

# Downloads $Url into the file $File, giving up after $Seconds seconds when given (the archive gets no limit).
function Invoke-Download([string]$Url, [string]$File, [int]$Seconds) {
    $request = @{ Uri = $Url; OutFile = $File; UseBasicParsing = $true }
    if ($Seconds -gt 0) { $request.TimeoutSec = $Seconds }
    Invoke-WebRequest @request
}

# The "version" string in a JSON file (latest.json, install.json); nothing when there is none.
function Get-JsonVersion([string]$File) {
    try {
        $json = [IO.File]::ReadAllText($File) | ConvertFrom-Json
        $field = $json.PSObject.Properties['version']
        if ($field -and $field.Value -is [string]) { return $field.Value }
    } catch {
        return ''
    }
    return ''
}

# The latest version as latest.json names it; nothing when it cannot be read.
function Get-FeedVersion([string]$LatestUrl, [string]$Tmp) {
    $feed = [IO.Path]::Combine($Tmp, 'latest.json')
    try { Invoke-Download -Url $LatestUrl -File $feed -Seconds 30 } catch { return '' }
    return Get-JsonVersion $feed
}

# The latest version from where GitHub's /releases/latest redirects (its Location names the tag).
function Get-GitHubVersion([string]$DownloadUrl) {
    if ($DownloadUrl -cnotmatch '/download\z') { return '' }
    $releases = $DownloadUrl.Substring(0, $DownloadUrl.Length - '/download'.Length)
    try {
        $request = [Net.WebRequest]::Create("$releases/latest")
        $request.Method = 'HEAD'
        $request.AllowAutoRedirect = $false
        $request.Timeout = 30000
        $response = $request.GetResponse()
        try { $location = [string]$response.Headers['Location'] } finally { $response.Close() }
    } catch {
        return ''
    }
    if ($location -cmatch '/releases/tag/([^/?#]*)\z') { return $Matches[1] -creplace '^v', '' }
    return ''
}

# The version to install: the one asked for, else the latest.
function Resolve-Version([string]$Version, [string]$LatestUrl, [string]$DownloadUrl, [string]$Tmp) {
    if ($Version) {
        $Version = $Version -creplace '^v', ''
        if (-not (Test-Version $Version)) { throw "not a ttyak version: $Version (expected X.Y.Z, e.g. 0.10.0)" }
        return $Version
    }
    $Version = Get-FeedVersion $LatestUrl $Tmp
    if (Test-Version $Version) { return $Version }
    Write-Say "Could not read the latest version from $LatestUrl; asking GitHub instead"
    $Version = Get-GitHubVersion $DownloadUrl
    if (Test-Version $Version) { return $Version }
    throw "could not find out the latest ttyak version (from $LatestUrl or GitHub); pass -Version X.Y.Z (TTYAK_VERSION with irm | iex)"
}

# The install folder as an absolute path (not made yet): $Dir, else %LOCALAPPDATA%\Programs\ttyak.
function Resolve-Folder([string]$Dir) {
    if (-not $Dir) {
        $local = $env:LOCALAPPDATA
        if (-not $local) { $local = [Environment]::GetFolderPath('LocalApplicationData') }
        if (-not $local) { throw 'LOCALAPPDATA is not set: pick a folder with -Dir' }
        $Dir = [IO.Path]::Combine($local, 'Programs\ttyak')
    }
    # PATH separates its folders with ;
    if ($Dir -match '[\x00-\x1f;]') { throw "the folder's name has a ; or control characters in it: pick another with -Dir" }
    try {
        # Relative to PowerShell's current folder, ~ for the home folder.
        $full = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Dir)
        $full = [IO.Path]::GetFullPath($full)
    } catch {
        throw "not a folder name: $Dir (pick another with -Dir)"
    }
    if ($full.Length -gt 3) { $full = $full.TrimEnd('\') }
    return $full
}

# Removes the copies a running ttyak kept in use at the last update, where it can.
function Clear-OldCopy([string]$Dir) {
    if (-not [IO.Directory]::Exists($Dir)) { return }
    foreach ($file in [IO.Directory]::GetFiles($Dir, 'ttyak.old*.exe')) {
        if ([IO.Path]::GetFileName($file) -cmatch '^ttyak\.old(-[0-9a-f]{8})?\.exe\z') {
            try { [IO.File]::Delete($file) } catch { Write-Verbose "still in use: $file" }
        }
    }
}

# The SHA256 that the SHA256SUMS file gives for $Asset (its exact name); nothing when it names no such file.
function Find-Checksum([string]$File, [string]$Asset) {
    foreach ($line in [IO.File]::ReadAllLines($File)) {
        $fields = @($line.Trim() -split '\s+')
        if ($fields.Count -ge 2 -and ($fields[1] -ceq $Asset -or $fields[1] -ceq "*$Asset")) {
            return $fields[0].ToLowerInvariant()
        }
    }
    return ''
}

# Puts the staged executable in place of $Exe. A ttyak running from $Exe keeps its file, so the old copy is renamed
# aside first (put back when the new one cannot be moved in); returns the old copy's name when it is still in use.
function Install-Executable([string]$Staged, [string]$Exe, [string]$Dir) {
    if (-not [IO.File]::Exists($Exe)) {
        [IO.File]::Move($Staged, $Exe)
        return ''
    }
    $aside = [IO.Path]::Combine($Dir, 'ttyak.old.exe')
    if ([IO.File]::Exists($aside)) {
        $aside = [IO.Path]::Combine($Dir, 'ttyak.old-' + [Guid]::NewGuid().ToString('N').Substring(0, 8) + '.exe')
    }
    try { [IO.File]::Move($Exe, $aside) } catch { throw "could not replace ${Exe}: $(Get-Reason $_)" }
    try {
        [IO.File]::Move($Staged, $Exe)
    } catch {
        $reason = Get-Reason $_
        try { [IO.File]::Move($aside, $Exe) } catch { throw "could not put the new ttyak.exe in place ($reason); the old one is now $aside" }
        throw "could not put the new ttyak.exe in place ($reason); the old one stays"
    }
    try {
        [IO.File]::Delete($aside)
        return ''
    } catch {
        return $aside
    }
}

# Writes install.json next to the executable (UTF-8, no BOM), atomically.
function Write-Record([string]$Dir, [string]$Version, [hashtable]$State) {
    $escaped = $Dir.Replace('\', '\\').Replace('"', '\"')
    $record = [IO.Path]::Combine($Dir, 'install.json')
    $State.Staged = [IO.Path]::Combine($Dir, '.install.json.new')
    [IO.File]::Delete($State.Staged)
    $text = '{"method":"script","version":"' + $Version + '","dir":"' + $escaped + '"}' + "`n"
    [IO.File]::WriteAllText($State.Staged, $text, (New-Object Text.UTF8Encoding $false))
    if ([IO.File]::Exists($record)) {
        [IO.File]::Replace($State.Staged, $record, [NullString]::Value)
    } else {
        [IO.File]::Move($State.Staged, $record)
    }
    $State.Staged = ''
}

# Runs `ttyak.exe --version`; warns when it fails.
function Test-Installed([string]$Exe) {
    $ErrorActionPreference = 'Continue'
    try {
        $ran = (& $Exe --version 2>&1 | Out-String).Trim()
        if ($LASTEXITCODE -eq 0) { return }
    } catch {
        $ran = Get-Reason $_
    }
    Write-Complaint "installed, but ``$Exe --version`` failed: $ran"
}

# Whether the PATH list $List holds the folder $Dir (any case, a trailing \, quoted or as %VAR%).
function Test-OnPath([string]$List, [string]$Dir) {
    $want = $Dir.TrimEnd('\')
    foreach ($entry in ($List -split ';')) {
        $entry = [Environment]::ExpandEnvironmentVariables($entry.Trim().Trim('"')).TrimEnd('\')
        if ($entry -and [string]::Equals($entry, $want, [StringComparison]::OrdinalIgnoreCase)) { return $true }
    }
    return $false
}

# Tells running programs (Explorer, so new terminals) that the environment changed; false when it could not.
function Send-EnvironmentChange([string]$Tmp) {
    try {
        if (-not ('TtyakInstaller.Native' -as [type])) {
            # Add-Type leaves a folder in TEMP: in the installer's own, removed at the end.
            $temp = $env:TEMP, $env:TMP
            try {
                $env:TEMP = $env:TMP = $Tmp
                Add-Type -Namespace TtyakInstaller -Name Native -MemberDefinition @'
[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint msg, UIntPtr wParam, string lParam, uint flags, uint timeout, out UIntPtr result);
'@
            } finally {
                $env:TEMP, $env:TMP = $temp
            }
        }
        $result = [UIntPtr]::Zero
        # HWND_BROADCAST, WM_SETTINGCHANGE, SMTO_ABORTIFHUNG, 5 s
        $sent = [TtyakInstaller.Native]::SendMessageTimeout([IntPtr]0xffff, 0x1a, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$result)
        return $sent -ne [IntPtr]::Zero
    } catch {
        return $false
    }
}

# Puts $Dir on the user PATH when it is not on PATH already, once. The user PATH is read and written as the
# registry holds it, so %VAR% entries stay as they are.
function Add-ToPath([string]$Dir, [bool]$NoModifyPath, [string]$Tmp) {
    $exe = [IO.Path]::Combine($Dir, 'ttyak.exe')
    $manual = "add $Dir to your user PATH (Start, then ""Edit environment variables for your account"") to run ttyak by name"
    if (Test-OnPath $env:Path $Dir) {
        $found = @(Get-Command -Name 'ttyak.exe' -CommandType Application -ErrorAction SilentlyContinue)
        if ($found.Count -gt 0 -and -not [string]::Equals([IO.Path]::GetFullPath($found[0].Path), $exe, [StringComparison]::OrdinalIgnoreCase)) {
            Write-Complaint "note: $($found[0].Path) comes before $Dir on PATH, so ``ttyak`` runs that one"
        }
        Write-Say 'Run: ttyak'
        return
    }
    $raw = ''
    $kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment')
    if ($key) {
        try {
            $value = $key.GetValue('Path', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            if ($null -ne $value) {
                $raw = [string]$value
                $kind = $key.GetValueKind('Path')
            }
        } finally {
            $key.Close()
        }
    }
    if ((Test-OnPath $raw $Dir) -or (Test-OnPath ([Environment]::GetEnvironmentVariable('Path', 'Machine')) $Dir)) {
        Write-Say "$Dir is on your PATH already: open a new terminal to use ttyak"
        return
    }
    if ($NoModifyPath) {
        Write-Say "$Dir is not on PATH; as asked, PATH was not changed: $manual"
        return
    }
    if ($kind -ne [Microsoft.Win32.RegistryValueKind]::ExpandString -and $kind -ne [Microsoft.Win32.RegistryValueKind]::String) {
        Write-Say "$Dir is not on PATH; the user PATH is not a plain list, so it was left alone: $manual"
        return
    }
    $new = if ($raw) { "$Dir;$raw" } else { $Dir }
    try {
        $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
        try { $key.SetValue('Path', $new, $kind) } finally { $key.Close() }
    } catch {
        Write-Say "$Dir is not on PATH; could not change the user PATH ($(Get-Reason $_)): $manual"
        return
    }
    $env:Path = "$Dir;$env:Path"
    Write-Say "Added $Dir to your user PATH; ``ttyak uninstall`` removes it again."
    if (Send-EnvironmentChange $Tmp) {
        Write-Say 'Open a new terminal to use ttyak.'
    } else {
        Write-Say 'Open a new terminal to use ttyak (if it is not found there, sign out and in again).'
    }
}

function Invoke-Install([string]$Dir, [string]$Version, [bool]$NoModifyPath, [string]$Arch, [hashtable]$State) {
    $latestUrl = if ($env:TTYAK_LATEST_URL) { $env:TTYAK_LATEST_URL } else { 'https://ttyak.sh/latest.json' }
    $downloadUrl = if ($env:TTYAK_DOWNLOAD_URL) { $env:TTYAK_DOWNLOAD_URL } else { 'https://github.com/dmytrobsp/ttyak/releases/download' }
    $platform = Get-Platform $Arch
    $Dir = Resolve-Folder $Dir
    if (Test-Elevated) { Write-Complaint "running as Administrator is not needed: ttyak is installed for $env:USERNAME only, into $Dir" }
    Clear-OldCopy $Dir
    $tmp = [IO.Path]::Combine([IO.Path]::GetTempPath(), 'ttyak-install-' + [Guid]::NewGuid().ToString('N').Substring(0, 12))
    [void][IO.Directory]::CreateDirectory($tmp)
    $State.Tmp = $tmp
    $Version = Resolve-Version -Version $Version -LatestUrl $latestUrl -DownloadUrl $downloadUrl -Tmp $tmp
    $asset = "ttyak-$Version-$platform.zip"
    $base = "$downloadUrl/v$Version"
    Write-Say "Downloading ttyak $Version for $platform"
    $sums = [IO.Path]::Combine($tmp, 'SHA256SUMS')
    try { Invoke-Download -Url "$base/SHA256SUMS" -File $sums } catch { throw "could not download $base/SHA256SUMS: is $Version a ttyak release?" }
    $want = Find-Checksum $sums $asset
    if (-not $want) { throw "ttyak $Version has no build for $platform" }
    if ($want -cnotmatch '^[0-9a-f]{64}\z') { throw "SHA256SUMS has no proper checksum for $asset" }
    $zip = [IO.Path]::Combine($tmp, $asset)
    try { Invoke-Download -Url "$base/$asset" -File $zip } catch { throw "could not download $base/$asset" }
    $got = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($got -cne $want) { throw "checksum mismatch for ${asset}: SHA256SUMS says $want, the download is $got; nothing was installed" }
    $unpacked = [IO.Path]::Combine($tmp, 'unpacked')
    try { Expand-Archive -LiteralPath $zip -DestinationPath $unpacked } catch { throw "could not unpack $asset" }
    $new = [IO.Path]::Combine($unpacked, 'ttyak.exe')
    if (-not [IO.File]::Exists($new)) { throw "$asset has no ttyak.exe in it" }
    try { [void][IO.Directory]::CreateDirectory($Dir) } catch { throw "could not create $Dir" }
    $exe = [IO.Path]::Combine($Dir, 'ttyak.exe')
    if ([IO.Directory]::Exists($exe)) { throw "$exe is a folder" }
    $updating = [IO.File]::Exists($exe)
    $old = ''
    if ($updating) {
        $old = Get-JsonVersion ([IO.Path]::Combine($Dir, 'install.json'))
        if (-not (Test-Version $old)) { $old = '' }
    }
    $State.Staged = [IO.Path]::Combine($Dir, '.ttyak.exe.new')
    try {
        [IO.File]::Delete($State.Staged)
        [IO.File]::Copy($new, $State.Staged)
    } catch {
        throw "cannot write to ${Dir}: pick a folder you own with -Dir"
    }
    $running = Install-Executable -Staged $State.Staged -Exe $exe -Dir $Dir
    $State.Staged = ''
    Write-Record -Dir $Dir -Version $Version -State $State
    if (-not $updating) {
        Write-Say "Installed ttyak $Version as $exe"
    } elseif ($old -and $old -cne $Version) {
        Write-Say "Updated ttyak $old to $Version in $Dir"
    } else {
        Write-Say "Installed ttyak $Version over the copy in $Dir"
    }
    if ($running) {
        Write-Say "A ttyak still running goes on with the old copy, now $running (the next update removes it): restart it to use $Version"
    }
    Test-Installed $exe
    Add-ToPath -Dir $Dir -NoModifyPath $NoModifyPath -Tmp $tmp
    Write-Say 'To remove it later: ttyak uninstall'
}

# Hands over to `ttyak uninstall`, which asks whether to keep the data unless told; its exit code is in $LASTEXITCODE.
function Invoke-Uninstall([string]$Dir, [bool]$Yes, [bool]$KeepData) {
    $Dir = Resolve-Folder $Dir
    $exe = [IO.Path]::Combine($Dir, 'ttyak.exe')
    if (-not [IO.File]::Exists($exe)) { throw "no ttyak in ${Dir}: pass -Dir if it was installed somewhere else" }
    $flags = @('uninstall')
    if ($Yes) { $flags += '--yes' }
    if ($KeepData) { $flags += '--keep-data' }
    if (-not $Yes -and -not (Test-Terminal)) {
        throw 'no terminal to ask whether to keep your data: pass -Yes to remove ttyak and its data, or -Yes -KeepData to keep the data (TTYAK_YES=1 and TTYAK_KEEP_DATA=1 with irm | iex)'
    }
    # Not captured, so that ttyak asks on the terminal itself.
    & $exe @flags
}

function Install-Ttyak {
    [CmdletBinding()]
    param(
        [switch]$Yes,
        [string]$Version,
        [string]$Dir,
        [switch]$NoModifyPath,
        [switch]$Uninstall,
        [switch]$KeepData,
        [switch]$Help,
        # The CPU to install for instead of this machine's (for the tests).
        [Parameter(DontShow)][string]$Arch
    )
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    Set-StrictMode -Version 2.0
    $code = 0
    $state = @{ Tmp = ''; Staged = '' }
    $protocols = [Net.ServicePointManager]::SecurityProtocol
    try {
        if ($Help) {
            Write-Usage
        } elseif ($PSVersionTable.PSVersion.Major -ge 6 -and -not $IsWindows) {
            throw 'this script is for Windows; on Linux and macOS run: curl -fsSL https://ttyak.sh/install.sh | sh'
        } else {
            # Old .NET defaults leave out TLS 1.2, which GitHub needs; 0 is the system's choice, which has it.
            if ([int]$protocols -ne 0) { [Net.ServicePointManager]::SecurityProtocol = $protocols -bor [Net.SecurityProtocolType]::Tls12 }
            $b = $PSBoundParameters
            $folder = Resolve-Value -Bound $b -Name Dir -Variable TTYAK_DIR
            if (Resolve-Switch -Bound $b -Name Uninstall -Variable TTYAK_UNINSTALL) {
                $yesOn = Resolve-Switch -Bound $b -Name Yes -Variable TTYAK_YES
                $keep = Resolve-Switch -Bound $b -Name KeepData -Variable TTYAK_KEEP_DATA
                Invoke-Uninstall -Dir $folder -Yes $yesOn -KeepData $keep
                $code = $LASTEXITCODE
            } else {
                $wanted = Resolve-Value -Bound $b -Name Version -Variable TTYAK_VERSION
                $noPath = Resolve-Switch -Bound $b -Name NoModifyPath -Variable TTYAK_NO_MODIFY_PATH
                Invoke-Install -Dir $folder -Version $wanted -NoModifyPath $noPath -Arch $Arch -State $state
            }
        }
    } catch {
        Write-Complaint (Get-Reason $_)
        $code = 1
    } finally {
        if ($state.Staged) { try { [IO.File]::Delete($state.Staged) } catch { Write-Verbose "left: $($state.Staged)" } }
        if ($state.Tmp -and [IO.Directory]::Exists($state.Tmp)) { Remove-Item -LiteralPath $state.Tmp -Recurse -Force -ErrorAction SilentlyContinue }
        [Net.ServicePointManager]::SecurityProtocol = $protocols
    }
    # Run as a file: its exit code. Under iex: $LASTEXITCODE, and the session goes on.
    if ($PSCommandPath) { exit $code }
    $global:LASTEXITCODE = $code
}

# Last, and in braces, so that a download cut short runs nothing (not even the call without its flags).
if ($true) { Install-Ttyak @args }
