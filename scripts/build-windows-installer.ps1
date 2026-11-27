# Builds EcConn release binaries, stages a single-click Portable bundle,
# and compiles EcConn-Setup.exe if Inno Setup 6 (ISCC.exe) is installed.
$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent $PSScriptRoot
Push-Location $Root
try {
    Write-Host "Building EcConn Host/Receiver (app-host) and server..."
    cargo build --release -p app-host -p server
    if ($LASTEXITCODE -ne 0) { throw "Cargo build failed" }

    $StageDir = Join-Path $Root "dist\EcConn-Windows-Portable"
    New-Item -ItemType Directory -Force -Path $StageDir | Out-Null

    Copy-Item -Force (Join-Path $Root "target\release\app-host.exe") (Join-Path $StageDir "EcConn.exe")
    Copy-Item -Force (Join-Path $Root "target\release\server.exe") (Join-Path $StageDir "server.exe")

    $ffmpeg = Get-Command ffmpeg -ErrorAction SilentlyContinue
    if ($ffmpeg) {
        Copy-Item -Force $ffmpeg.Source (Join-Path $StageDir "ffmpeg.exe")
        Write-Host "Bundled ffmpeg.exe from $($ffmpeg.Source)"
    } else {
        Write-Warning "ffmpeg.exe not found in PATH; install ffmpeg or copy it into $StageDir"
    }

    Write-Host "Staged portable bundle at: $StageDir"

    $isccCandidates = @(
        "C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
        "C:\Program Files\Inno Setup 6\ISCC.exe"
    )
    $iscc = $isccCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1
    if ($iscc) {
        Write-Host "Compiling EcConn-Setup.exe with $iscc..."
        & $iscc (Join-Path $Root "packaging\windows\EcConn-Setup.iss")
        if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }
        Write-Host "Created installer: $(Join-Path $Root 'dist\EcConn-Setup.exe')"
    } else {
        Write-Host "Inno Setup 6 not found. Install via: winget install JRSoftware.InnoSetup"
        Write-Host "Then re-run this script to generate dist\EcConn-Setup.exe"
    }
} finally {
    Pop-Location
}
