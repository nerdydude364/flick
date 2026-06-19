#!/usr/bin/env pwsh
<#
Builds Flick_{version}_{arch}-setup.exe for Windows via cargo-packager (NSIS).

Bundles libmpv-2.dll and its dependency closure next to flick.exe inside the
installer — same idea as dylibbundler on macOS, but Windows needs every DLL
copied explicitly (see packaging/windows-dlls staging below).
#>

$ErrorActionPreference = 'Stop'

# Change to repository root (script is in scripts/)
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location -Path (Resolve-Path (Join-Path $scriptDir '..'))

# MPV arch default
if (-not $env:MPV_ARCH -or $env:MPV_ARCH -eq '') { $env:MPV_ARCH = 'amd64' }
$MPV_ARCH = $env:MPV_ARCH

# MPV_DEV_DIR default
if (-not $env:MPV_DEV_DIR -or $env:MPV_DEV_DIR -eq '') {
    $env:MPV_DEV_DIR = Join-Path (Get-Location) "third_party\mpv-windows-$MPV_ARCH"
}
$MPV_DEV_DIR = $env:MPV_DEV_DIR

Write-Host "==> Setting up libmpv for Windows ($MPV_ARCH)"

# Try to run the existing shell setup script via bash/sh if available
$setupScript = Join-Path $scriptDir 'mpv-windows-setup.sh'
if (Test-Path $setupScript) {
    if (Get-Command bash -ErrorAction SilentlyContinue) {
        & bash $setupScript
    } elseif (Get-Command sh -ErrorAction SilentlyContinue) {
        & sh $setupScript
    } else {
        throw "Required shell (bash/sh) not found. Please run '$setupScript' manually or install Git for Windows / WSL."
    }
} else {
    Write-Host "Note: setup script not found at $setupScript — continuing."
}

Write-Host "==> Staging runtime DLLs for the NSIS bundle"
$dest = Join-Path (Get-Location) 'packaging\windows-dlls'
New-Item -ItemType Directory -Path $dest -Force | Out-Null

# Remove existing DLLs if any
Get-ChildItem -Path $dest -Filter '*.dll' -File -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue

$srcBin = Join-Path $MPV_DEV_DIR 'bin'
if (-not (Test-Path $srcBin)) { throw "MPV bin directory not found: $srcBin" }
Get-ChildItem -Path $srcBin -Filter '*.dll' -File | ForEach-Object {
    Copy-Item -Path $_.FullName -Destination $dest -Force
}

Write-Host "==> Checking for cargo packager, installing if missing"
$packagerInstalled = $false
try {
    $out = & cargo --list 2>$null
    if ($out -match 'packager') { $packagerInstalled = $true }
} catch {
    # cargo not available or error occurred; installation below will report the error
}

if (-not $packagerInstalled) {
    Write-Host "cargo-packager not found — installing..."
    & cargo install --locked cargo-packager
} else {
    Write-Host "cargo-packager already installed."
}

Write-Host "==> Building NSIS installer"
& cargo packager -c Packager.toml --formats nsis

Write-Host "==> Done — see dist/"
Write-Host ""
Write-Host "Verification checklist:"
Write-Host "  - Install the .exe on a clean Windows VM without mpv installed separately"
Write-Host "  - Confirm video playback and file associations work"
Write-Host "  - Re-run the format-diversity fixture set against the packaged build"
