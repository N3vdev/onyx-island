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
    [switch]$SkipBuild,
    [switch]$MsixOnly
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
if (-not $MsixOnly -and -not $makensis) { throw "NSIS (makensis.exe) not found" }

# The Visual C++ runtime DLLs, shipped next to the exes (app-local).
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vs = & $vswhere -latest -products * -property installationPath
$crt = Get-ChildItem (Join-Path $vs "VC\Redist\MSVC") -Recurse -Directory -Filter "Microsoft.VC14*.CRT" |
    Where-Object { $_.FullName -match '\\x64\\' -and $_.FullName -notmatch 'onecore|debug' } |
    Sort-Object FullName | Select-Object -Last 1
if (-not $crt) { throw "Visual C++ redistributable DLLs not found" }

# --- Stage ---------------------------------------------------------------
# Verify the local cleanup payload before replacing any packaging workspace.
$cleanupFiles = @(
    @{ Name = 'onyx-cleanup-270m-r5-selected-q8_0.gguf'; Hash = '760e148a14c0057a086f47efbec76f738ce817901141e45a58d9566dee67cf8f' },
    @{ Name = 'llama-b11927-bin-win-cpu-x64.zip'; Hash = 'f70b7fe800e683f58fed3199fcf9c961932307f30916498a7758dce5f1d76ce9' }
)
foreach ($file in $cleanupFiles) {
    $source = Join-Path (Join-Path $release 'models') $file.Name
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw "Missing cleanup payload: $source" }
    if ((Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash -ne $file.Hash) {
        throw "Cleanup payload checksum mismatch: $($file.Name)"
    }
}
$resolvedWork = [IO.Path]::GetFullPath($work)
$targetRoot = [IO.Path]::GetFullPath((Join-Path $root 'target')) + [IO.Path]::DirectorySeparatorChar
if (-not $resolvedWork.StartsWith($targetRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Packaging workspace is outside target: $resolvedWork"
}
if ((Test-Path -LiteralPath $resolvedWork) -and
    ((Get-Item -LiteralPath $resolvedWork -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
    throw "Packaging workspace must not be a link: $resolvedWork"
}
Remove-Item -LiteralPath $resolvedWork -Recurse -Force -ErrorAction SilentlyContinue
$stage = Join-Path $work "stage"
New-Item -ItemType Directory -Force $stage, $dist | Out-Null
foreach ($exe in "islet.exe", "islet-settings.exe", "islet-asr-worker.exe") {
    Copy-Item (Join-Path $release $exe) $stage
}
foreach ($dll in "vcruntime140.dll", "vcruntime140_1.dll") {
    Copy-Item (Join-Path $crt.FullName $dll) $stage
}
$models = Join-Path $stage 'models'
$runtime = Join-Path $models 'ai-cleanup\runtime'
New-Item -ItemType Directory -Force $runtime | Out-Null
Copy-Item -LiteralPath (Join-Path (Join-Path $release 'models') $cleanupFiles[0].Name) -Destination $models
# Runtime code stays inside the package. The app runs this bundled server
# directly; only the model is copied to writable per-user storage.
Add-Type -AssemblyName System.IO.Compression.FileSystem
$runtimeZip = [IO.Compression.ZipFile]::OpenRead((Join-Path (Join-Path $release 'models') $cleanupFiles[1].Name))
try {
    foreach ($entry in $runtimeZip.Entries) {
        $name = [IO.Path]::GetFileName($entry.FullName)
        if ($name -and ($name.EndsWith('.dll') -or $name -eq 'llama-server.exe' -or $name.StartsWith('LICENSE'))) {
            [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, (Join-Path $runtime $name), $true)
        }
    }
} finally { $runtimeZip.Dispose() }
if (-not (Test-Path -LiteralPath (Join-Path $runtime 'llama-server.exe') -PathType Leaf)) {
    throw 'Pinned runtime archive did not contain llama-server.exe.'
}
foreach ($dll in 'vcruntime140.dll', 'vcruntime140_1.dll') {
    Copy-Item -LiteralPath (Join-Path $crt.FullName $dll) -Destination $runtime
}

# --- MSIX ----------------------------------------------------------------
$layout = Join-Path $work "msix"
New-Item -ItemType Directory -Force $layout | Out-Null
Copy-Item "$stage\*" $layout -Recurse
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
if (-not $MsixOnly) {
$setup = Join-Path $dist "Onyx-Setup-$version.exe"
& $makensis /V2 "/DVERSION=$version" "/DPUBLISHER=$PublisherDisplayName" "/DSTAGE=$stage" "/DICON=$(Join-Path $root 'assets\onyx.ico')" "/DOUTFILE=$setup" (Join-Path $here "onyx.nsi")
if ($LASTEXITCODE -ne 0) { throw "makensis failed" }
}

Write-Host ""
$outputs = @($msix)
if (-not $MsixOnly) { $outputs += $setup }
Get-Item $outputs | ForEach-Object { "{0,-32} {1,6:N1} MB" -f $_.Name, ($_.Length / 1MB) }
