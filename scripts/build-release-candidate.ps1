param(
    [Parameter(Mandatory=$true)][string]$Snapshot,
    [Parameter(Mandatory=$true)][string]$OutputDirectory,
    [string]$UpdaterKeyPath,
    [string]$ReleaseKeyReceipt,
    [string]$DependencySourceDirectory,
    [switch]$TechnicalOnly
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$source = (Resolve-Path -LiteralPath $Snapshot).Path
$out = [IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $out) { throw 'Candidate output must be a new directory' }
$info = Get-Content -LiteralPath (Join-Path $source 'SOURCE.json') -Raw | ConvertFrom-Json
if ($info.identifier -ne 'com.dreamrugi.worldbuildtool') { throw 'Unexpected candidate identity' }
& python (Join-Path $source 'scripts/release-candidate.py') verify-source --folder $source
if ($LASTEXITCODE -ne 0) { throw 'Source snapshot verification failed' }
New-Item -ItemType Directory -Path $out | Out-Null
foreach ($name in @('SOURCE.json','LICENSE','README.md')) { Copy-Item -LiteralPath (Join-Path $source $name) -Destination (Join-Path $out $name) }
Copy-Item -LiteralPath "${source}-source.zip" -Destination (Join-Path $out 'corresponding-source.zip')
Copy-Item -LiteralPath (Join-Path $source 'packaging/release-notes.md') -Destination (Join-Path $out 'release-notes.md')
if ($DependencySourceDirectory) {
    $dependency = (Resolve-Path -LiteralPath $DependencySourceDirectory).Path
    $manifestPath = Join-Path $dependency 'third-party-source.manifest.json'
    $archivePath = Join-Path $dependency 'third-party-source.zip'
    $dependencyInfo = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    if ($dependencyInfo.base_head -ne $info.base_head -or
        $dependencyInfo.package_lock_sha256 -ne (Get-FileHash -LiteralPath (Join-Path $source 'package-lock.json')).Hash.ToLowerInvariant() -or
        $dependencyInfo.cargo_lock_sha256 -ne (Get-FileHash -LiteralPath (Join-Path $source 'src-tauri/Cargo.lock')).Hash.ToLowerInvariant() -or
        $dependencyInfo.zip_sha256 -ne (Get-FileHash -LiteralPath $archivePath).Hash.ToLowerInvariant()) {
        throw 'Reusable dependency source does not match this fixed candidate'
    }
    Copy-Item -LiteralPath $archivePath -Destination (Join-Path $out 'third-party-source.zip')
    Copy-Item -LiteralPath $manifestPath -Destination (Join-Path $out 'third-party-source.manifest.json')
}
Push-Location $source
try {
    & npm.cmd ci
    if ($LASTEXITCODE -ne 0) { throw 'npm ci failed' }
    $argsMap = @{ Channel='Github'; Mode='Release'; OutputDirectory=$out; UpdaterKeyPath=$UpdaterKeyPath; ReleaseKeyReceipt=$ReleaseKeyReceipt; UnsignedTechnical=$TechnicalOnly }
    & (Join-Path $source 'scripts/package-windows.ps1') @argsMap
    & python (Join-Path $source 'scripts/release-candidate.py') verify-source --folder $source
    if ($LASTEXITCODE -ne 0) { throw 'Build changed a fixed source input' }
    if (-not $DependencySourceDirectory) {
        & python (Join-Path $source 'scripts/export-third-party-source.py') (Join-Path $out 'third-party-source.zip')
        if ($LASTEXITCODE -ne 0) { throw 'Third-party source collection failed' }
    }
    # bundle still verifies every archived file; reuse only avoids recollection.
    $bundleArgs = @('bundle','--folder',$out,'--version',$info.version)
    if ($TechnicalOnly) { $bundleArgs += '--technical' } else { $bundleArgs += @('--public-key',"$UpdaterKeyPath.pub",'--receipt',$ReleaseKeyReceipt) }
    & python (Join-Path $source 'scripts/release-candidate.py') @bundleArgs
    if ($LASTEXITCODE -ne 0) { throw 'Release bundle verification failed' }
} finally {
    Pop-Location
    Remove-Item Env:\TAURI_SIGNING_PRIVATE_KEY,Env:\TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
}
