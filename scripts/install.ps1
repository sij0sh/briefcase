#Requires -Version 5.1
# briefcase installer for Windows.
# Usage: powershell -ExecutionPolicy Bypass -File install.ps1 -Repo <owner/repo> [-Version latest|tag] [-InstallDir <path>]
# Piped: & ([scriptblock]::Create((irm <raw-url>/scripts/install.ps1))) -Version v0.1.1
param(
    [string]$Repo = $(if ($env:GITHUB_REPO) { $env:GITHUB_REPO } else { "sij0sh/briefcase" }),
    [string]$Version = "latest",
    [string]$InstallDir = "$env:LOCALAPPDATA\Programs\briefcase"
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# TLS 1.2 needed by Windows PowerShell 5.x on older stacks.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

if (-not $Repo) { throw "Set GITHUB_REPO or pass -Repo <owner/repo>" }

# Prefer the real OS architecture: x64-emulated PowerShell on Windows ARM64
# reports AMD64 via $env:PROCESSOR_ARCHITECTURE.
$arch = "$env:PROCESSOR_ARCHITECTURE"
try {
    $osArch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    if ($osArch) { $arch = "$osArch" }
} catch { }

switch -Regex ($arch) {
    '^(X64|AMD64)$'    { $target = 'x86_64-pc-windows-msvc' }
    '^(ARM64|AARCH64)$' { $target = 'aarch64-pc-windows-msvc' }
    default { throw "Unsupported architecture: $arch" }
}

if ($Version -eq 'latest') {
    $url = "https://github.com/$Repo/releases/latest/download/briefcase-$target.zip"
} else {
    $url = "https://github.com/$Repo/releases/download/$Version/briefcase-$target.zip"
}

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("briefcase-install-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
$zip = Join-Path $tmp "briefcase-$target.zip"
$extract = Join-Path $tmp 'extracted'

try {
    Write-Host "Downloading briefcase ($target) from $url"
    try {
        Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
    } catch {
        throw "Download failed: $url`n$($_.Exception.Message)"
    }
    if (-not (Test-Path $zip) -or (Get-Item $zip).Length -eq 0) {
        throw "Asset not found or empty: $url"
    }

    $checksumUrl = $url.Substring(0, $url.LastIndexOf('/')) + '/SHA256SUMS'
    $checksumFile = Join-Path $tmp 'SHA256SUMS'
    Invoke-WebRequest -Uri $checksumUrl -OutFile $checksumFile -UseBasicParsing
    $assetName = [IO.Path]::GetFileName($zip)
    $matches = @(Get-Content $checksumFile | Where-Object { $_ -match ('^[0-9a-f]{64}  ' + [regex]::Escape($assetName) + '$') })
    if ($matches.Count -ne 1) { throw 'Missing or ambiguous archive checksum' }
    $expected = $matches[0].Substring(0, 64)
    $actual = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLowerInvariant()
    if ($actual -ne $expected) { throw 'Archive checksum mismatch' }

    Expand-Archive -Path $zip -DestinationPath $extract
    $exe = Get-ChildItem -Path $extract -Recurse -Filter 'briefcase.exe' | Select-Object -First 1
    if (-not $exe) { throw "briefcase.exe not found in archive" }

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Move-Item -Path $exe.FullName -Destination (Join-Path $InstallDir 'briefcase.exe') -Force

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $entries = @()
    if ($userPath) { $entries = $userPath -split ';' | Where-Object { $_ } }
    if ($entries -notcontains $InstallDir) {
        $newPath = if ($userPath) { "$userPath;$InstallDir" } else { $InstallDir }
        [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
        Write-Host "Added $InstallDir to your user PATH. Open a new terminal for it to take effect."
    }
    if (($env:Path -split ';') -notcontains $InstallDir) {
        $env:Path = "$env:Path;$InstallDir"
    }

    $briefcase = Join-Path $InstallDir 'briefcase.exe'
    try {
        $v = (& $briefcase --version | Select-Object -First 1)
        Write-Host "Installed: $v"
    } catch {
        Write-Host "Installed to $briefcase (could not run --version locally)."
    }

    Write-Host ""
    Write-Host "Next steps:"
    Write-Host "  1. Open a new terminal."
    Write-Host "  2. Run: briefcase install --agent pi --dry-run, then without --dry-run"
    Write-Host "  3. In each repository, run pi (or: briefcase sync)"
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
