# Stage the locally trained, pinned model for this PC and local build.
param([Parameter(Mandatory = $true)][string]$ModelPath)
$ErrorActionPreference = 'Stop'
$expectedHash = '760E148A14C0057A086F47EFBEC76F738CE817901141E45A58D9566DEE67CF8F'
if ((Get-FileHash -LiteralPath $ModelPath -Algorithm SHA256).Hash -ne $expectedHash) {
    throw 'This is not the expected Onyx Cleanup Round 5 model.'
}
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$fileName = 'onyx-cleanup-270m-r5-selected-q8_0.gguf'
$destinations = @((Join-Path $repositoryRoot 'target\release\models'), (Join-Path $env:LOCALAPPDATA 'Islet\models\ai-cleanup'))
foreach ($destination in $destinations) {
    New-Item -ItemType Directory -Path $destination -Force | Out-Null
    $targetFile = Join-Path $destination $fileName
    # An identical installed model can still be memory-mapped by a runtime.
    # Leave that verified copy in place rather than try to overwrite it.
    $alreadyInstalled = (Test-Path -LiteralPath $targetFile -PathType Leaf) -and
        ((Get-FileHash -LiteralPath $targetFile -Algorithm SHA256).Hash -eq $expectedHash)
    if (-not $alreadyInstalled) {
        Copy-Item -LiteralPath $ModelPath -Destination $targetFile -Force
    }
    if ((Get-FileHash -LiteralPath $targetFile -Algorithm SHA256).Hash -ne $expectedHash) {
        throw 'Installed model checksum did not match.'
    }
}
Write-Output 'Onyx Cleanup model installed and staged. The CPU runtime is installed from Dictation settings if missing.'
