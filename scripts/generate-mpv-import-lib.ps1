# Generates an MSVC import library (mpv.lib) from libmpv-2.dll.
# Invoked by mpv-windows-setup.sh on Windows CI/dev machines.
$ErrorActionPreference = "Stop"

$libDir = $env:MPV_LIB_DIR
$dllPath = $env:MPV_DLL
$arch = if ($env:MPV_ARCH -eq "arm64") { "ARM64" } else { "X64" }
$hostArch = if ($env:MPV_ARCH -eq "arm64") { "arm64" } else { "x64" }

if (-not $libDir -or -not $dllPath) {
    throw "MPV_LIB_DIR and MPV_DLL must be set"
}
if (-not (Test-Path $dllPath)) {
    throw "libmpv-2.dll not found at $dllPath"
}

$outLib = Join-Path $libDir "mpv.lib"
$defFile = Join-Path $libDir "mpv.def"

$vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path $vswhere)) {
    throw "vswhere.exe not found - install Visual Studio Build Tools with C++ workload"
}

$vsPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vsPath) {
    throw "Visual Studio C++ tools not found"
}

$libExe = Get-ChildItem (Join-Path $vsPath "VC\Tools\MSVC\*\bin\Host$hostArch\$hostArch\lib.exe") -ErrorAction SilentlyContinue |
    Sort-Object FullName -Descending |
    Select-Object -First 1
if (-not $libExe) {
    throw "lib.exe not found under $vsPath for Host$hostArch\$hostArch"
}

# Locate dumpbin for generating a .def if needed
$dumpbin = Get-ChildItem (Join-Path $vsPath "VC\Tools\MSVC\*\bin\Host$hostArch\$hostArch\dumpbin.exe") -ErrorAction SilentlyContinue |
    Sort-Object FullName -Descending |
    Select-Object -First 1

Push-Location $libDir
try {
    if (Test-Path $defFile) {
        & $libExe.FullName "/def:mpv.def" "/name:libmpv-2.dll" "/out:mpv.lib" "/MACHINE:$arch"
    } else {
        if (-not $dumpbin) {
            throw "dumpbin.exe not found under $vsPath — cannot generate .def from DLL"
        }
        Write-Host "Generating mpv.def from $dllPath via dumpbin"
        $dumpOutput = & $dumpbin.FullName "/exports" $dllPath 2>&1 | Out-String
        # Save full dumpbin output to a file for CI artifact inspection
        $logFile = Join-Path $libDir "dumpbin-exports.txt"
        Set-Content -Path $logFile -Value $dumpOutput -Encoding UTF8
        Write-Host "Wrote dumpbin output to $logFile"
        # Emit the first 200 lines to the action log for quick inspection
        $dumpLines = $dumpOutput -split "\r?\n"
        $max = [Math]::Min($dumpLines.Count - 1, 199)
        if ($max -ge 0) { $dumpLines[0..$max] | ForEach-Object { Write-Host $_ } }
        $exportNames = @()
        foreach ($line in $dumpOutput -split "\r?\n") {
            if ($line -match '^\s*\d+\s+[0-9A-Fa-f]+\s+[0-9A-Fa-f]+\s+(\S+)') {
                $exportNames += $matches[1]
            }
        }
        if ($exportNames.Count -eq 0) {
            throw "No exported symbols found in $dllPath; cannot generate mpv.def"
        }
        $defContent = @()
        $defContent += "LIBRARY $(Split-Path -Leaf $dllPath)"
        $defContent += "EXPORTS"
        $defContent += $exportNames
        Set-Content -Path $defFile -Value $defContent -Encoding ASCII
        Write-Host "Generated $defFile with $($exportNames.Count) exports"
        & $libExe.FullName "/def:mpv.def" "/name:libmpv-2.dll" "/out:mpv.lib" "/MACHINE:$arch"
    }
} finally {
    Pop-Location
}

if (-not (Test-Path $outLib)) {
    throw "Failed to generate $outLib"
}

Write-Host "Generated $outLib"
