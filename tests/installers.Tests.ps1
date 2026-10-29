# Offline Windows acceptance. Persistent PATH tests require explicit opt-in on disposable GitHub Windows runners; state is restored in finally.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$Script = Join-Path (Split-Path $PSScriptRoot -Parent) 'scripts/install.ps1'
$Root = Join-Path ([IO.Path]::GetTempPath()) ('ssh-client-tests-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $Root | Out-Null
$global:SshClientInstallerTestFixtureRoot = Join-Path $Root 'fixture'
New-Item -ItemType Directory -Path $global:SshClientInstallerTestFixtureRoot | Out-Null
$global:SshClientInstallerTestDownloadFailure = $false
$global:SshClientInstallerTestAssertions = 0
$global:SshClientInstallerTestDeleteFailurePath = $null
function Assert-True([bool]$Value, [string]$Message) {
    $global:SshClientInstallerTestAssertions++
    if (-not $Value) { throw "Assertion failed: $Message" }
}
function Expect-Failure([scriptblock]$Action, [string]$Message) {
    $Failed = $false
    try { & $Action | Out-Null } catch { $Failed = $true }
    Assert-True $Failed $Message
}
# Fault injection for the partial-uninstall rollback path, runnable on any OS.
function Remove-Item {
    [CmdletBinding()]
    param([string[]]$LiteralPath, [switch]$Force, [switch]$Recurse)
    if ($global:SshClientInstallerTestDeleteFailurePath -and $LiteralPath -contains $global:SshClientInstallerTestDeleteFailurePath) {
        throw 'Simulated file deletion denied'
    }
    Microsoft.PowerShell.Management\Remove-Item @PSBoundParameters
}
# Function shadowing substitutes transport; installer URL validation stays real.
function Invoke-WebRequest {
    param($Uri, $OutFile, [switch]$UseBasicParsing)
    if ($global:SshClientInstallerTestDownloadFailure) { throw 'Simulated offline transport' }
    if ($Uri.EndsWith('/SHA256SUMS')) { Copy-Item (Join-Path $global:SshClientInstallerTestFixtureRoot 'SHA256SUMS') $OutFile }
    elseif ($Uri.EndsWith('.zip')) { Copy-Item (Join-Path $global:SshClientInstallerTestFixtureRoot 'package.zip') $OutFile }
    else { throw "Unexpected URL: $Uri" }
}
function Invoke-RestMethod { param($Uri); return @{ tag_name = 'v0.1.0' } }
function Build-Fixture([string[]]$Names = @('ssh.exe', 'ssh-client.exe'), [string]$Executable = '') {
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $Path = Join-Path $global:SshClientInstallerTestFixtureRoot 'package.zip'
    Remove-Item -ErrorAction SilentlyContinue -LiteralPath $Path
    $Zip = [IO.Compression.ZipFile]::Open($Path, [IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($Name in $Names) {
            $Entry = $Zip.CreateEntry($Name)
            if ($Executable) {
                $InputStream = [IO.File]::OpenRead($Executable)
                $OutputStream = $Entry.Open()
                try { $InputStream.CopyTo($OutputStream) } finally { $InputStream.Dispose(); $OutputStream.Dispose() }
            } else {
                $Writer = [IO.StreamWriter]::new($Entry.Open())
                try { $Writer.Write('fixture executable bytes') } finally { $Writer.Dispose() }
            }
        }
    } finally { $Zip.Dispose() }
    $Arch = if ([Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() -eq 'Arm64') { 'aarch64' } else { 'x86_64' }
    $Hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash
    "$Hash  ssh-client-v0.1.0-$Arch-pc-windows-msvc.zip" | Set-Content -LiteralPath (Join-Path $global:SshClientInstallerTestFixtureRoot 'SHA256SUMS')
}
$InitialPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$InitialProcessPath = $env:Path
$PersistentPathChanged = $false
function Invoke-FreshPathProcess([string]$Program, [string]$ExpectedBin) {
    $Start = [Diagnostics.ProcessStartInfo]::new()
    $Start.FileName = (Get-Process -Id $PID).Path
    $Start.Arguments = '-NoLogo -NoProfile -NonInteractive -EncodedCommand ' + [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($Program))
    $Start.UseShellExecute = $false
    $Start.RedirectStandardOutput = $true
    $Start.RedirectStandardError = $true
    # Model a freshly created Windows logon environment, not the installer's
    # inherited process PATH: machine entries then persisted user entries.
    $PersistedPath = @([Environment]::GetEnvironmentVariable('Path', 'Machine'), [Environment]::GetEnvironmentVariable('Path', 'User')) -join ';'
    $Start.EnvironmentVariables['PATH'] = [Environment]::ExpandEnvironmentVariables($PersistedPath)
    $Start.EnvironmentVariables['SSH_CLIENT_TEST_EXPECTED_BIN'] = $ExpectedBin
    $Process = [Diagnostics.Process]::Start($Start)
    try {
        $Output = $Process.StandardOutput.ReadToEnd()
        $Errors = $Process.StandardError.ReadToEnd()
        $Process.WaitForExit()
        if ($Process.ExitCode -ne 0) { throw "Fresh PATH process failed: $Errors" }
        return ($Output | ConvertFrom-Json)
    } finally { $Process.Dispose() }
}
try {
    Build-Fixture
    $Prefix = Join-Path $Root 'install with spaces'
    & $Script -Prefix $Prefix -Version v0.1.0 -NoPath
    Assert-True (Test-Path (Join-Path $Prefix 'bin/ssh.exe')) 'ssh installed'
    Assert-True (Test-Path (Join-Path $Prefix 'bin/ssh-client.exe')) 'ssh-client installed'
    & $Script -Prefix $Prefix -NoPath
    Assert-True (([Environment]::GetEnvironmentVariable('Path', 'User')) -eq $InitialPath) 'NoPath preserves registry PATH'
    Set-Content (Join-Path $Prefix 'keep.txt') 'keep'
    & $Script -Prefix $Prefix -Uninstall -NoPath
    Assert-True (-not (Test-Path (Join-Path $Prefix 'bin/ssh.exe'))) 'uninstall removes managed binary'
    Assert-True (Test-Path (Join-Path $Prefix 'keep.txt')) 'uninstall preserves foreign file'
    & $Script -Prefix $Prefix -Uninstall -NoPath

    $Foreign = Join-Path $Root 'foreign'
    New-Item -ItemType Directory (Join-Path $Foreign 'bin') | Out-Null
    Set-Content (Join-Path $Foreign 'bin/ssh.exe') 'foreign'
    Expect-Failure { & $Script -Prefix $Foreign -NoPath } 'foreign binary is not overwritten'
    Assert-True ((Get-Content (Join-Path $Foreign 'bin/ssh.exe')) -eq 'foreign') 'foreign contents preserved'

    & $Script -Prefix $Prefix -NoPath
    Set-Content (Join-Path $Prefix 'bin/ssh.exe') 'modified'
    Expect-Failure { & $Script -Prefix $Prefix -Uninstall -NoPath } 'modified binary is not deleted'
    Assert-True ((Get-Content (Join-Path $Prefix 'bin/ssh.exe')) -eq 'modified') 'modified contents preserved'

    if ([Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([Runtime.InteropServices.OSPlatform]::Windows)) {
        $LockedPrefix = Join-Path $Root 'locked-install'
        & $Script -Prefix $LockedPrefix -NoPath
        $Before = (Get-FileHash -Algorithm SHA256 (Join-Path $LockedPrefix 'bin/ssh-client.exe')).Hash
        $ReceiptBefore = Get-Content -Raw (Join-Path $LockedPrefix '.ssh-client-owned.json')
        $Held = [IO.File]::Open((Join-Path $LockedPrefix 'bin/ssh.exe'), [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
        try { Expect-Failure { & $Script -Prefix $LockedPrefix -NoPath } 'locked Windows executable rejects update' }
        finally { $Held.Dispose() }
        Assert-True ((Get-FileHash -Algorithm SHA256 (Join-Path $LockedPrefix 'bin/ssh-client.exe')).Hash -eq $Before) 'rollback preserves previous first binary'
        Assert-True ((Get-Content -Raw (Join-Path $LockedPrefix '.ssh-client-owned.json')) -eq $ReceiptBefore) 'rollback preserves receipt'
        # Hold the second deletion target so the first removal succeeds, then
        # confirm uninstall rolls it back without losing receipt or PATH.
        $PathBeforeUninstall = [Environment]::GetEnvironmentVariable('Path', 'User')
        $ProcessPathBeforeUninstall = $env:Path
        $Held = [IO.File]::Open((Join-Path $LockedPrefix 'bin/ssh-client.exe'), [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
        try { Expect-Failure { & $Script -Prefix $LockedPrefix -Uninstall -NoPath } 'locked second executable rejects uninstall' }
        finally { $Held.Dispose() }
        Assert-True (Test-Path (Join-Path $LockedPrefix 'bin/ssh.exe')) 'partial uninstall restores first binary'
        Assert-True (Test-Path (Join-Path $LockedPrefix 'bin/ssh-client.exe')) 'locked binary remains installed'
        Assert-True ((Get-Content -Raw (Join-Path $LockedPrefix '.ssh-client-owned.json')) -eq $ReceiptBefore) 'failed uninstall preserves receipt'
        Assert-True (([Environment]::GetEnvironmentVariable('Path', 'User')) -eq $PathBeforeUninstall) 'failed uninstall preserves persistent PATH'
        Assert-True ($env:Path -eq $ProcessPathBeforeUninstall) 'failed uninstall preserves process PATH'
        & $Script -Prefix $LockedPrefix -Uninstall -NoPath
        Assert-True (-not (Test-Path (Join-Path $LockedPrefix '.ssh-client-owned.json'))) 'uninstall retry removes receipt after unlocking'
    }

    $PartialPrefix = Join-Path $Root 'partial-uninstall'
    & $Script -Prefix $PartialPrefix -NoPath
    $PartialReceipt = Get-Content -Raw (Join-Path $PartialPrefix '.ssh-client-owned.json')
    $PartialPath = $env:Path
    $global:SshClientInstallerTestDeleteFailurePath = Join-Path $PartialPrefix 'bin/ssh-client.exe'
    try { Expect-Failure { & $Script -Prefix $PartialPrefix -Uninstall -NoPath } 'injected second deletion failure rejects uninstall' }
    finally { $global:SshClientInstallerTestDeleteFailurePath = $null }
    Assert-True (Test-Path (Join-Path $PartialPrefix 'bin/ssh.exe')) 'injected failure restores removed first binary'
    Assert-True ((Get-Content -Raw (Join-Path $PartialPrefix '.ssh-client-owned.json')) -eq $PartialReceipt) 'injected failure preserves ownership receipt'
    Assert-True ($env:Path -eq $PartialPath) 'injected failure preserves process PATH'
    & $Script -Prefix $PartialPrefix -Uninstall -NoPath
    Assert-True (-not (Test-Path (Join-Path $PartialPrefix '.ssh-client-owned.json'))) 'retry after injected failure completes'

    $Clean = Join-Path $Root 'clean'
    (('0' * 64) + '  ssh-client-v0.1.0-x86_64-pc-windows-msvc.zip') | Set-Content (Join-Path $global:SshClientInstallerTestFixtureRoot 'SHA256SUMS')
    Expect-Failure { & $Script -Prefix $Clean -NoPath } 'bad checksum rejected'
    Assert-True (-not (Test-Path $Clean)) 'bad checksum makes no installation'
    Build-Fixture @('ssh.exe', '../escape')
    Expect-Failure { & $Script -Prefix $Clean -NoPath } 'archive traversal rejected'
    Assert-True (-not (Test-Path (Join-Path $Root 'escape'))) 'archive cannot escape'
    Build-Fixture
    $global:SshClientInstallerTestDownloadFailure = $true
    Expect-Failure { & $Script -Prefix $Clean -NoPath } 'download error propagates'
    $global:SshClientInstallerTestDownloadFailure = $false
    Expect-Failure { & $Script -Prefix $Clean -Version '../../attack' -NoPath } 'invalid version rejected'
    Expect-Failure { & $Script -Prefix 'relative/path' -NoPath } 'relative path rejected'
    if ($env:SSH_CLIENT_TEST_PERSISTENT_PATH -eq '1') {
        if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows' -or -not [Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([Runtime.InteropServices.OSPlatform]::Windows)) {
            throw 'Persistent PATH tests are restricted to disposable GitHub Windows runners.'
        }
        $RepoRoot = Split-Path $PSScriptRoot -Parent
        $Candidates = @()
        if ($env:SSH_CLIENT_TEST_BINARY) { $Candidates += $env:SSH_CLIENT_TEST_BINARY }
        $Candidates += (Join-Path $RepoRoot 'target/debug/ssh-client.exe'), (Join-Path $RepoRoot 'target/release/ssh-client.exe')
        $Binary = @($Candidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1)
        if ($Binary.Count -ne 1) { throw 'Build a native ssh-client.exe first, or set SSH_CLIENT_TEST_BINARY.' }
        Build-Fixture -Executable $Binary[0]
        $PathPrefix = Join-Path $Root 'persistent-path-install'
        $PathBin = Join-Path $PathPrefix 'bin'
        $PersistentPathChanged = $true
        & $Script -Prefix $PathPrefix
        & $Script -Prefix $PathPrefix
        $UserEntries = @([Environment]::GetEnvironmentVariable('Path', 'User') -split ';' | Where-Object { $_.TrimEnd('\') -ieq $PathBin.TrimEnd('\') })
        Assert-True ($UserEntries.Count -eq 1) 'repeat install persists exactly one user PATH entry'
        Assert-True ((Get-Command ssh-client -CommandType Application | Select-Object -First 1).Source -ieq (Join-Path $PathBin 'ssh-client.exe')) 'current process resolves ssh-client to installation'
        Assert-True ((Get-Command ssh -CommandType Application | Select-Object -First 1).Source -ieq (Join-Path $PathBin 'ssh.exe')) 'current process resolves ssh alias to installation'
        $Installed = Invoke-FreshPathProcess -ExpectedBin $PathBin -Program @'
$ErrorActionPreference = 'Stop'
$Client = (Get-Command ssh-client -CommandType Application | Select-Object -First 1).Source
$Aliases = @(Get-Command ssh -All -CommandType Application | ForEach-Object { $_.Source })
$Version = & ssh-client --version
if ($LASTEXITCODE -ne 0) { throw 'ssh-client --version failed' }
$AliasVersion = & (Join-Path $env:SSH_CLIENT_TEST_EXPECTED_BIN 'ssh.exe') --version
if ($LASTEXITCODE -ne 0) { throw 'installed ssh --version failed' }
[pscustomobject]@{ client = $Client; aliases = $Aliases; version = ($Version -join "`n"); aliasVersion = ($AliasVersion -join "`n") } | ConvertTo-Json -Compress
'@
        Assert-True ($Installed.client -ieq (Join-Path $PathBin 'ssh-client.exe')) 'fresh process resolves persisted ssh-client PATH'
        Assert-True ((Join-Path $PathBin 'ssh.exe') -in @($Installed.aliases)) 'fresh process finds installed ssh alongside any system ssh'
        Assert-True ($Installed.version -match '^ssh-client ') 'fresh process executes real Rust client'
        Assert-True ($Installed.aliasVersion -eq $Installed.version) 'installed ssh alias executes same real client'
        Set-Content (Join-Path $PathPrefix 'foreign.txt') 'keep'
        $InstalledUserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        $Held = [IO.File]::Open((Join-Path $PathBin 'ssh-client.exe'), [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
        try { Expect-Failure { & $Script -Prefix $PathPrefix -Uninstall } 'locked uninstall cannot remove persisted PATH' }
        finally { $Held.Dispose() }
        Assert-True (([Environment]::GetEnvironmentVariable('Path', 'User')) -eq $InstalledUserPath) 'locked uninstall preserves actual registry PATH'
        Assert-True (Test-Path (Join-Path $PathPrefix '.ssh-client-owned.json')) 'locked PATH uninstall preserves receipt'
        Assert-True (Test-Path (Join-Path $PathBin 'ssh.exe')) 'locked PATH uninstall restores alias'
        & $Script -Prefix $PathPrefix -Uninstall
        Assert-True (Test-Path (Join-Path $PathPrefix 'foreign.txt')) 'PATH uninstall preserves unrelated files'
        Assert-True (-not (Test-Path (Join-Path $PathBin 'ssh-client.exe'))) 'PATH uninstall removes binary'
        $UserEntries = @([Environment]::GetEnvironmentVariable('Path', 'User') -split ';' | Where-Object { $_.TrimEnd('\') -ieq $PathBin.TrimEnd('\') })
        Assert-True ($UserEntries.Count -eq 0) 'uninstall removes persisted PATH entry'
        $Removed = Invoke-FreshPathProcess -ExpectedBin $PathBin -Program @'
$ClientPaths = @(Get-Command ssh-client -All -CommandType Application -ErrorAction SilentlyContinue | ForEach-Object { $_.Source })
$AliasPaths = @(Get-Command ssh -All -CommandType Application -ErrorAction SilentlyContinue | ForEach-Object { $_.Source })
[pscustomobject]@{ clients = $ClientPaths; aliases = $AliasPaths; path = $env:PATH } | ConvertTo-Json -Compress
'@
        Assert-True ((Join-Path $PathBin 'ssh-client.exe') -notin @($Removed.clients)) 'fresh process no longer resolves removed client'
        Assert-True ((Join-Path $PathBin 'ssh.exe') -notin @($Removed.aliases)) 'fresh process no longer resolves removed alias'
        Assert-True ($PathBin -notin @($Removed.path -split ';')) 'fresh process PATH no longer contains installation'
    }
    Write-Output "PASS: $global:SshClientInstallerTestAssertions PowerShell installer assertions"
} finally {
    try {
        if ($PersistentPathChanged) { [Environment]::SetEnvironmentVariable('Path', $InitialPath, 'User') }
    } finally {
        $env:Path = $InitialProcessPath
        $global:SshClientInstallerTestDeleteFailurePath = $null
        Remove-Item -Recurse -Force -LiteralPath $Root
        Remove-Variable -Scope Global -Name SshClientInstallerTestFixtureRoot, SshClientInstallerTestDownloadFailure, SshClientInstallerTestAssertions, SshClientInstallerTestDeleteFailurePath -ErrorAction SilentlyContinue
    }
}
