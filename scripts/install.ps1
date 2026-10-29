# Download from a trusted, pinned tag for reproducible installation.
[CmdletBinding()]
param(
    [string]$Version = 'latest',
    [string]$Prefix = (Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'ssh-client'),
    [switch]$NoPath,
    [switch]$Uninstall
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not [IO.Path]::IsPathRooted($Prefix)) { throw 'Prefix must be absolute.' }
$Prefix = [IO.Path]::GetFullPath($Prefix).TrimEnd([IO.Path]::DirectorySeparatorChar)
if (-not $Prefix -or $Prefix -eq [IO.Path]::GetPathRoot($Prefix).TrimEnd('\')) { throw 'Refusing root prefix.' }
if ($Prefix.IndexOfAny([char[]]"`r`n;`"") -ge 0) { throw 'Unsupported characters in prefix.' }
if ($Version -ne 'latest' -and $Version -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+(?:[-.][A-Za-z0-9.-]+)?$') { throw 'Version must be latest or vX.Y.Z.' }
$Bin = Join-Path $Prefix 'bin'
$Receipt = Join-Path $Prefix '.ssh-client-owned.json'
function Assert-NoLinks([string]$Path) {
    $Current = $Path
    while ($Current) {
        if (Test-Path -LiteralPath $Current) {
            $Item = Get-Item -Force -LiteralPath $Current
            if ($Item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Refusing reparse point: $Current" }
        }
        $Parent = Split-Path -Parent $Current
        if ($Parent -eq $Current) { break }
        $Current = $Parent
    }
}
function Assert-Owned {
    $Saved = $null
    if (Test-Path -LiteralPath $Receipt) { $Saved = Get-Content -Raw -LiteralPath $Receipt | ConvertFrom-Json }
    foreach ($Name in @('ssh.exe', 'ssh-client.exe')) {
        $Path = Join-Path $Bin $Name
        if (Test-Path -LiteralPath $Path) {
            Assert-NoLinks $Path
            if (-not $Saved -or -not ($Saved.PSObject.Properties.Name -contains $Name)) { throw "Refusing foreign file: $Path" }
            if ((Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash -ne $Saved.$Name) { throw "Refusing modified file: $Path" }
        }
    }
}
# Serialize installers in the current logon session without leaving stale lock files.
$Hasher = [Security.Cryptography.SHA256]::Create()
try { $LockHash = [BitConverter]::ToString($Hasher.ComputeHash([Text.Encoding]::UTF8.GetBytes($Prefix.ToLowerInvariant()))).Replace('-', '') }
finally { $Hasher.Dispose() }
$Guard = [Threading.Mutex]::new($false, ('ssh-client-installer-' + $LockHash))
$Acquired = $false
try { $Acquired = $Guard.WaitOne(0) } catch [Threading.AbandonedMutexException] { $Acquired = $true }
if (-not $Acquired) { $Guard.Dispose(); throw 'Another installer is running for this prefix.' }
try {
Assert-NoLinks $Bin
Assert-NoLinks $Receipt
Assert-Owned
if ($Uninstall) {
    if (Test-Path -LiteralPath $Receipt) {
        # Back up owned binaries before deletion. A running executable can deny
        # deletion on Windows; keep both the receipt and PATH until all removals
        # succeed, restoring any binary already removed when a later one fails.
        $UninstallStage = Join-Path $Prefix ('.uninstall-' + [Guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $UninstallStage | Out-Null
        $Removed = @()
        $UninstallRollbackFailed = $false
        try {
            foreach ($Name in @('ssh.exe', 'ssh-client.exe')) {
                $Path = Join-Path $Bin $Name
                if (Test-Path -LiteralPath $Path) { Copy-Item -LiteralPath $Path -Destination (Join-Path $UninstallStage $Name) }
            }
            try {
                foreach ($Name in @('ssh.exe', 'ssh-client.exe')) {
                    $Path = Join-Path $Bin $Name
                    if (Test-Path -LiteralPath $Path) {
                        Remove-Item -Force -ErrorAction Stop -LiteralPath $Path
                        $Removed += $Name
                    }
                }
            } catch {
                $RemovalError = $_
                foreach ($Name in $Removed) {
                    try { Copy-Item -LiteralPath (Join-Path $UninstallStage $Name) -Destination (Join-Path $Bin $Name) -ErrorAction Stop }
                    catch { $UninstallRollbackFailed = $true; Write-Warning "Could not restore $Name; backup retained at $UninstallStage." }
                }
                throw "Uninstall could not remove all binaries. Close running ssh / ssh-client processes and retry. Receipt and PATH were retained. Original error: $RemovalError"
            }
        } finally {
            if (-not $UninstallRollbackFailed) { Remove-Item -Recurse -Force -ErrorAction Stop -LiteralPath $UninstallStage }
        }
        Remove-Item -Force -LiteralPath $Receipt
        if (-not $NoPath) {
            $UserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
            $Entries = @($UserPath -split ';' | Where-Object { $_ -and $_.TrimEnd('\') -ine $Bin.TrimEnd('\') })
            [Environment]::SetEnvironmentVariable('Path', ($Entries -join ';'), 'User')
            $env:Path = (@($env:Path -split ';' | Where-Object { $_ -and $_.TrimEnd('\') -ine $Bin.TrimEnd('\') }) -join ';')
        }
        foreach ($Directory in @($Bin, $Prefix)) { if ((Test-Path -LiteralPath $Directory) -and -not @(Get-ChildItem -Force -LiteralPath $Directory).Count) { Remove-Item -LiteralPath $Directory } }
        Write-Output 'Uninstalled managed binaries; open a new terminal to refresh PATH.'
    } else { Write-Output 'Nothing installed (no ownership receipt).' }
    return
}
if ($env:OS -ne 'Windows_NT') { throw 'Use install.sh on Linux and macOS.' }
$Architecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
$Arch = switch ($Architecture) { 'X64' { 'x86_64' }; 'Arm64' { 'aarch64' }; default { throw "Unsupported architecture: $Architecture" } }
$Repo = 'neko233-com/ssh-client'
# TLS 1.2 remains enabled for Windows PowerShell 5.1 without disabling certificate checks.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
if ($Version -eq 'latest') {
    $Version = (Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest").tag_name
    if ($Version -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+(?:[-.][A-Za-z0-9.-]+)?$') { throw 'Unsafe release tag.' }
}
$Asset = "ssh-client-$Version-$Arch-pc-windows-msvc.zip"
$Base = "https://github.com/$Repo/releases/download/$Version"
$Work = Join-Path ([IO.Path]::GetTempPath()) ('ssh-client-' + [Guid]::NewGuid().ToString('N'))
$Stage = $null
$RollbackFailed = $false
New-Item -ItemType Directory -Path $Work | Out-Null
try {
    $Archive = Join-Path $Work 'package.zip'
    $Sums = Join-Path $Work 'SHA256SUMS'
    Invoke-WebRequest -UseBasicParsing -Uri "$Base/$Asset" -OutFile $Archive
    Invoke-WebRequest -UseBasicParsing -Uri "$Base/SHA256SUMS" -OutFile $Sums
    $MatchesForAsset = @(Get-Content -LiteralPath $Sums | Where-Object { $_ -match ('^[a-fA-F0-9]{64}\s+\*?' + [regex]::Escape($Asset) + '$') })
    if ($MatchesForAsset.Count -ne 1) { throw 'Missing or duplicate checksum.' }
    $Expected = ($MatchesForAsset[0] -split '\s+')[0]
    if ((Get-FileHash -Algorithm SHA256 -LiteralPath $Archive).Hash -ne $Expected) { throw 'SHA256 verification failed.' }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $Zip = [IO.Compression.ZipFile]::OpenRead($Archive)
    try {
        $Names = @($Zip.Entries | ForEach-Object { $_.FullName })
        if ($Names.Count -ne 2 -or @($Names | Select-Object -Unique).Count -ne 2 -or 'ssh.exe' -notin $Names -or 'ssh-client.exe' -notin $Names) { throw 'Invalid archive entries.' }
        foreach ($Entry in $Zip.Entries) {
            # Reject UNIX symlinks if an archive creator supplied UNIX mode metadata.
            if ((($Entry.ExternalAttributes -shr 16) -band 0xF000) -eq 0xA000) { throw 'Archive contains a symlink.' }
        }
    } finally { $Zip.Dispose() }
    Expand-Archive -LiteralPath $Archive -DestinationPath (Join-Path $Work 'extract')
    Assert-NoLinks $Bin
    Assert-NoLinks $Receipt
    Assert-Owned
    New-Item -ItemType Directory -Force -Path $Bin | Out-Null
    $Stage = Join-Path $Prefix ('.stage-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $Stage | Out-Null
    $Hashes = @{}
    foreach ($Name in @('ssh-client.exe', 'ssh.exe')) {
        Copy-Item -LiteralPath (Join-Path (Join-Path $Work 'extract') $Name) -Destination (Join-Path $Stage $Name)
        $Hashes[$Name] = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $Stage $Name)).Hash
    }
    $Replaced = @()
    try {
        foreach ($Name in @('ssh-client.exe', 'ssh.exe')) {
            $Destination = Join-Path $Bin $Name
            if (Test-Path -LiteralPath $Destination) { Copy-Item -LiteralPath $Destination -Destination (Join-Path $Stage ($Name + '.backup')) }
            Move-Item -Force -LiteralPath (Join-Path $Stage $Name) -Destination $Destination
            $Replaced += $Name
        }
        $ReceiptTemp = Join-Path $Stage 'receipt.new'
        $Hashes | ConvertTo-Json | Set-Content -Encoding UTF8 -LiteralPath $ReceiptTemp
        if (Test-Path -LiteralPath $Receipt) { [IO.File]::Replace($ReceiptTemp, $Receipt, (Join-Path $Stage 'receipt.backup')) }
        else { [IO.File]::Move($ReceiptTemp, $Receipt) }
    } catch {
        foreach ($Name in $Replaced) {
            $Backup = Join-Path $Stage ($Name + '.backup')
            try {
                if (Test-Path -LiteralPath $Backup) { Copy-Item -Force -LiteralPath $Backup -Destination (Join-Path $Bin $Name) }
                else { Remove-Item -Force -LiteralPath (Join-Path $Bin $Name) }
            } catch { $RollbackFailed = $true; Write-Warning "Rollback failed for $Name; backups retained at $Stage." }
        }
        throw
    }
    if (-not $NoPath) {
        $UserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        $Entries = @($UserPath -split ';' | Where-Object { $_ -and $_.TrimEnd('\') -ine $Bin.TrimEnd('\') })
        [Environment]::SetEnvironmentVariable('Path', ((@($Bin) + $Entries) -join ';'), 'User')
        $env:Path = ((@($Bin) + @($env:Path -split ';' | Where-Object { $_ -and $_.TrimEnd('\') -ine $Bin.TrimEnd('\') })) -join ';')
    }
    Write-Output "Installed $Version to $Bin. Both ssh-client.exe and ssh.exe are available."
    Write-Output 'Existing system ssh is untouched. Windows machine PATH entries can precede user PATH in new terminals; use ssh-client or the full path if ssh resolves to Windows OpenSSH.'
} finally {
    if (-not $RollbackFailed -and $Stage -and (Test-Path -LiteralPath $Stage)) { Remove-Item -Recurse -Force -LiteralPath $Stage }
    Remove-Item -Recurse -Force -LiteralPath $Work
}

} finally {
    $Guard.ReleaseMutex()
    $Guard.Dispose()
}
