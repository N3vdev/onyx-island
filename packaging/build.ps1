# Builds Onyx's release packages into dist\:
#   Onyx_<version>_x64.msix   for the Microsoft Store (unsigned; the Store signs it)
#   Onyx-Setup-<version>.exe  a per-user installer for downloading directly
#
# The Store identity defaults are Onyx Island's, from Partner Center > the app
# > Product management > Product identity (Store ID 9N7CWBB4HWS0). They must
# match it exactly or the upload is refused.

param(
    [string]$IdentityName = "Nevdev.OnyxIsland",
    [string]$Publisher = "CN=7BE60B1B-C205-4C0E-AB26-9C89190ED9A1",
    [string]$PublisherDisplayName = "Nevdev",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$here = $PSScriptRoot
$work = Join-Path $root "target\package"
$dist = Join-Path $root "dist"
$release = Join-Path $root "target\release"

$version = ([regex]'(?m)^version = "([^"]+)"').Match((Get-Content (Join-Path $root "Cargo.toml") -Raw)).Groups[1].Value
$msixVersion = "$version.0"
Write-Host "Onyx $version"

# --- Build ---------------------------------------------------------------
if (-not $SkipBuild) {
    Get-Process islet, islet-settings, islet-asr-worker -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Seconds 1
    Push-Location $root
    try {
        # Through cmd: Windows PowerShell turns cargo's progress on stderr into
        # errors whenever the output is redirected.
        cmd /c "cargo build --release -p islet-app -p islet-settings -p islet-asr-worker 2>&1"
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    } finally { Pop-Location }
}

# --- Tools ---------------------------------------------------------------
$kit = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin" -Directory |
    Where-Object { Test-Path (Join-Path $_.FullName "x64\makeappx.exe") } |
    Sort-Object { [version]$_.Name } | Select-Object -Last 1
if (-not $kit) { throw "Windows SDK (makeappx.exe) not found" }
$makeappx = Join-Path $kit.FullName "x64\makeappx.exe"
$makepri = Join-Path $kit.FullName "x64\makepri.exe"

$makensis = @("${env:ProgramFiles(x86)}\NSIS\makensis.exe", "$env:ProgramFiles\NSIS\makensis.exe") |
    Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $makensis) { throw "NSIS (makensis.exe) not found" }

# The Visual C++ runtime DLLs, shipped next to the exes (app-local).
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vs = & $vswhere -latest -products * -property installationPath
$crt = Get-ChildItem (Join-Path $vs "VC\Redist\MSVC") -Recurse -Directory -Filter "Microsoft.VC14*.CRT" |
    Where-Object { $_.FullName -match '\\x64\\' -and $_.FullName -notmatch 'onecore|debug' } |
    Sort-Object FullName | Select-Object -Last 1
if (-not $crt) { throw "Visual C++ redistributable DLLs not found" }

# --- Stage ---------------------------------------------------------------
Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
$stage = Join-Path $work "stage"
New-Item -ItemType Directory -Force $stage, $dist | Out-Null
foreach ($exe in "islet.exe", "islet-settings.exe", "islet-asr-worker.exe") {
    Copy-Item (Join-Path $release $exe) $stage
}
foreach ($dll in "vcruntime140.dll", "vcruntime140_1.dll") {
    Copy-Item (Join-Path $crt.FullName $dll) $stage
}

# --- MSIX ----------------------------------------------------------------
$layout = Join-Path $work "msix"
New-Item -ItemType Directory -Force $layout | Out-Null
Copy-Item "$stage\*" $layout
python -I (Join-Path $here "make_assets.py") (Join-Path $root "assets") (Join-Path $layout "Assets")
if ($LASTEXITCODE -ne 0) { throw "logo generation failed" }

$manifest = (Get-Content (Join-Path $here "AppxManifest.xml") -Raw).
    Replace("{{IDENTITY_NAME}}", $IdentityName).
    Replace("{{PUBLISHER}}", $Publisher).
    Replace("{{PUBLISHER_DISPLAY_NAME}}", $PublisherDisplayName).
    Replace("{{VERSION}}", $msixVersion)
[IO.File]::WriteAllText((Join-Path $layout "AppxManifest.xml"), $manifest, (New-Object Text.UTF8Encoding $false))

# resources.pri lets Windows pick each logo's scale and unplated variants.
$priConfig = Join-Path $work "priconfig.xml"
& $makepri createconfig /cf $priConfig /dq en-US /pv 10.0.0 /o | Out-Null
& $makepri new /pr $layout /cf $priConfig /mn (Join-Path $layout "AppxManifest.xml") /of (Join-Path $layout "resources.pri") /o | Out-Null
if ($LASTEXITCODE -ne 0) { throw "makepri failed" }

$msix = Join-Path $dist "Onyx_${msixVersion}_x64.msix"
& $makeappx pack /d $layout /p $msix /o | Out-Null
if ($LASTEXITCODE -ne 0) { throw "makeappx failed" }

# --- Setup .exe ----------------------------------------------------------
$setup = Join-Path $dist "Onyx-Setup-$version.exe"
& $makensis /V2 "/DVERSION=$version" "/DPUBLISHER=$PublisherDisplayName" "/DSTAGE=$stage" "/DICON=$(Join-Path $root 'assets\onyx.ico')" "/DOUTFILE=$setup" (Join-Path $here "onyx.nsi")
if ($LASTEXITCODE -ne 0) { throw "makensis failed" }

Write-Host ""
Get-Item $msix, $setup | ForEach-Object { "{0,-32} {1,6:N1} MB" -f $_.Name, ($_.Length / 1MB) }
