# Self-contained, on-demand OrbitDrop providers: no .NET installation required.
param([string]$Configuration = 'Release')
$ErrorActionPreference = 'Stop'
$repositoryRoot = Split-Path -Parent $PSScriptRoot
& dotnet publish (Join-Path $repositoryRoot 'workers\Onyx.Converter\Onyx.Converter.csproj') `
    -c $Configuration -r win-x64 --self-contained true `
    -o (Join-Path $repositoryRoot 'target\release\converter') --nologo
if ($LASTEXITCODE -ne 0) { throw 'Converter publish failed.' }

# Publishing into an existing directory does not remove obsolete files.
# Remove only debug symbols and non-English satellite resources owned by this
# helper, including files left behind by an older publish configuration.
$converterOutput = [IO.Path]::GetFullPath((Join-Path $repositoryRoot 'target\release\converter'))
$converterPrefix = $converterOutput + [IO.Path]::DirectorySeparatorChar
if ((Get-Item -LiteralPath $converterOutput -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
    throw 'Converter output must not be a link.'
}
Get-ChildItem -LiteralPath $converterOutput -File -Recurse | Where-Object {
    $_.Extension -eq '.pdb' -or
    ($_.Name.EndsWith('.resources.dll') -and $_.Directory.Name -ne 'en')
} | ForEach-Object {
    if (-not $_.FullName.StartsWith($converterPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Converter cleanup target is outside its output directory.'
    }
    Remove-Item -LiteralPath $_.FullName -Force
}
