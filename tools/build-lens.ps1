# Builds Onyx Lens (lens/, a Tauri app with its own Cargo workspace) and puts
# onyx-lens.exe next to islet.exe in target\release, where Onyx looks for it.
#
# Plain `cargo build` would leave the exe pointing at the Vite dev server, so
# this goes through `tauri build`, which bundles the web pages into the exe.
$ErrorActionPreference = 'Stop'
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$lens = Join-Path $repositoryRoot 'lens'

Get-Process onyx-lens -ErrorAction SilentlyContinue | Stop-Process -Force

Push-Location $lens
try {
    # Through cmd: Windows PowerShell turns progress on stderr into errors.
    cmd /c "npm ci --no-audit --no-fund 2>&1"
    if ($LASTEXITCODE -ne 0) { throw 'npm ci failed.' }
    cmd /c "npm run tauri build -- --no-bundle 2>&1"
    if ($LASTEXITCODE -ne 0) { throw 'Onyx Lens build failed.' }
} finally { Pop-Location }

$release = Join-Path $repositoryRoot 'target\release'
New-Item -ItemType Directory -Force $release | Out-Null
Copy-Item -LiteralPath (Join-Path $lens 'src-tauri\target\release\onyx-lens.exe') -Destination $release -Force
