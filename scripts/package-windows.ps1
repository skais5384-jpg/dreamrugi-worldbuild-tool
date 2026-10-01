param(
    [Parameter(Mandatory = $true)][ValidateSet('Github', 'Store')][string]$Channel,
    [ValidateSet('Test', 'Release')][string]$Mode = 'Test',
    [string]$VersionOverride,
    [string]$OutputDirectory,
    [string]$UpdaterKeyPath,
    [string]$ReleaseKeyReceipt,
    [string]$MsixThumbprint,
    [string]$StoreIdentityPath,
    [switch]$PrepareOnly,
    [switch]$UnsignedTechnical
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$base = Get-Content -LiteralPath (Join-Path $repo 'src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json
$npm = Get-Content -LiteralPath (Join-Path $repo 'package.json') -Raw | ConvertFrom-Json
$lock = Get-Content -LiteralPath (Join-Path $repo 'package-lock.json') -Raw | ConvertFrom-Json -AsHashtable
$cargo = Get-Content -LiteralPath (Join-Path $repo 'src-tauri/Cargo.toml') -Raw
$cargoVersion = [regex]::Match($cargo, '(?m)^version = "([^"\r\n]+)"').Groups[1].Value
$lockVersion = $lock['packages']['']['version']
$cargoLock = Get-Content -LiteralPath (Join-Path $repo 'src-tauri/Cargo.lock') -Raw
$ownLockVersion = [regex]::Match($cargoLock, '(?m)^name = "worldbuild-tool"\r?\nversion = "([^"\r\n]+)"').Groups[1].Value
if ($ownLockVersion -ne $npm.version -or $lock['version'] -ne $npm.version) { throw 'App version in lockfiles must match' }
if ($npm.version -ne $cargoVersion -or $npm.version -ne $base.version -or $npm.version -ne $lockVersion) {
    throw 'package.json, package-lock.json, Cargo.toml and tauri.conf.json versions must match'
}
if ($npm.license -ne 'GPL-3.0-only' -or $base.bundle.license -ne 'GPL-3.0-only' -or $cargo -notmatch '(?m)^license = "GPL-3\.0-only"$') {
    throw 'App license metadata must agree on GPL-3.0-only'
}
if ($Mode -eq 'Release' -and $VersionOverride) { throw 'Release version override is not allowed' }
$storeIdentity = $null
if ($Channel -eq 'Store') {
    if ($UpdaterKeyPath -or $ReleaseKeyReceipt -or (Test-Path Env:\TAURI_SIGNING_PRIVATE_KEY) -or (Test-Path Env:\TAURI_SIGNING_PRIVATE_KEY_PATH) -or (Test-Path Env:\TAURI_SIGNING_PRIVATE_KEY_PASSWORD)) {
        throw 'Store packaging cannot receive GitHub updater signing secrets'
    }
    if ($Mode -eq 'Release') {
        if (-not $StoreIdentityPath) { throw 'Verified Partner Center StoreIdentityPath is required for Store/release' }
        $identityJson = & python (Join-Path $PSScriptRoot 'store-identity.py') --identity $StoreIdentityPath --version $npm.version
        if ($LASTEXITCODE -ne 0) { throw 'Store identity/history validation failed' }
        $storeIdentity = $identityJson | ConvertFrom-Json
    } elseif ($StoreIdentityPath) { throw 'Local Store test identity cannot use Partner Center submission values' }
} elseif ($StoreIdentityPath -or $MsixThumbprint) { throw 'MSIX identity/certificate cannot be supplied to GitHub packaging' }
if ($UnsignedTechnical -and -not (($Mode -eq 'Release' -and $Channel -eq 'Github') -or ($Mode -eq 'Test' -and $Channel -eq 'Store'))) { throw 'Unsigned technical builds require Github/Release or isolated Store/Test identity' }
if ($UnsignedTechnical -and ($UpdaterKeyPath -or $env:TAURI_SIGNING_PRIVATE_KEY -or $env:TAURI_SIGNING_PRIVATE_KEY_PATH -or $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD)) { throw 'Unsigned technical builds cannot receive signing secrets' }
$version = if ($VersionOverride) { $VersionOverride } else { $npm.version }
if ($version -notmatch '^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$') { throw 'Expected a three-part numeric version' }
$parts = @([int]$Matches[1], [int]$Matches[2], [int]$Matches[3])
if ($parts | Where-Object { $_ -gt 65534 }) { throw 'Version parts exceed MSIX range' }
$msixVersion = if ($storeIdentity) { $storeIdentity.packageVersion } else { '{0}.{1}.{2}.0' -f ($parts[0] + 1), $parts[1], $parts[2] }
$out = if ($OutputDirectory) { [IO.Path]::GetFullPath($OutputDirectory) } else {
    Join-Path $repo ("logs/M7-7-IMPLEMENTATION-001/packages/{0}-{1}-{2}" -f $Channel.ToLower(), $Mode.ToLower(), $version)
}
New-Item -ItemType Directory -Force -Path $out | Out-Null
$overlayPath = Join-Path $out 'tauri-package-override.json'
$test = $Mode -eq 'Test'
$displayName = if ($test) { 'Dreamrugi Worldbuild Tool E Local Test' } else { $base.productName }
$identifier = if ($test) { 'com.dreamrugi.worldbuildtool.e.localtest' } else { $base.identifier }
$overlay = [ordered]@{
    productName = $displayName
    version = $version
    identifier = $identifier
    bundle = [ordered]@{
        targets = @('nsis')
        createUpdaterArtifacts = $Channel -eq 'Github' -and -not $UnsignedTechnical
        windows = [ordered]@{
            allowDowngrades = $false
            webviewInstallMode = @{ type = 'downloadBootstrapper' }
            nsis = @{ installMode = 'currentUser' }
        }
    }
}
if ($Channel -eq 'Github' -and $UpdaterKeyPath) {
    $publicKeyPath = "$UpdaterKeyPath.pub"
    if (-not (Test-Path -LiteralPath $publicKeyPath -PathType Leaf)) { throw 'Matching updater public key is missing' }
    $publicKey = (Get-Content -LiteralPath $publicKeyPath -Raw).Trim()
    if (-not $publicKey) { throw 'Updater public key is empty' }
    $overlay.plugins = [ordered]@{ updater = [ordered]@{ pubkey = $publicKey; endpoints = @() } }
}
$overlay | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $overlayPath -Encoding utf8
if ($PrepareOnly) {
    [pscustomobject]@{ Channel=$Channel; Mode=$Mode; Version=$version; MsixVersion=$msixVersion; Identifier=$identifier; Config=$overlayPath }
    return
}
if ($Channel -eq 'Github') {
    if (-not $UnsignedTechnical) {
    if (-not $UpdaterKeyPath) { throw 'GitHub packaging requires an updater private key path' }
    $key = (Resolve-Path -LiteralPath $UpdaterKeyPath).Path
    if ($Mode -eq 'Release' -and $key.StartsWith($repo, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Release key cannot be inside the repository'
    }
    if ($Mode -eq 'Release' -and (Test-Path -LiteralPath (Join-Path (Split-Path -Parent $key) 'LOCAL_TEST_ONLY'))) {
        throw 'Local test signing keys cannot be used for release packages'
    }
    if ($Mode -eq 'Release') {
        if (-not $ReleaseKeyReceipt) { throw 'Deployment key portable-backup receipt is required' }
        $receipt = Get-Content -LiteralPath $ReleaseKeyReceipt -Raw | ConvertFrom-Json
        if ($receipt.role -ne 'Release' -or $receipt.portableBackupVerified -ne $true) { throw 'Deployment key/portable backup verification is pending' }
        $keyText = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String(([IO.File]::ReadAllText($key)).Trim()))
        $keyPacket = [Convert]::FromBase64String(($keyText -split '\r?\n')[1])
        if ($keyPacket.Length -ne 158 -or $keyPacket[2] -ne 83 -or $keyPacket[3] -ne 99) { throw 'Release signing requires a password-encrypted Tauri/minisign key' }
        $publicText = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String(([IO.File]::ReadAllText("$key.pub")).Trim()))
        $publicPacket = [Convert]::FromBase64String(($publicText -split '\r?\n')[1])
        $publicHash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($publicPacket)).ToLowerInvariant()
        if ($publicHash -ne $receipt.publicKeySha256) { throw 'Deployment public key/receipt mismatch' }
    }
    if ($Mode -eq 'Release' -and -not $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD) {
        throw 'Updater key password must be provided in the current process environment'
    }
    if ($Mode -eq 'Test' -and -not (Test-Path Env:\TAURI_SIGNING_PRIVATE_KEY_PASSWORD)) {
        # CI-generated local test keys have no Tauri passphrase; an explicit
        # empty value avoids Tauri waiting for an interactive password prompt.
        $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ''
    }
    }
} else {
    if ($env:TAURI_SIGNING_PRIVATE_KEY -or $env:TAURI_SIGNING_PRIVATE_KEY_PATH -or $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD) {
        throw 'Store packaging cannot use the GitHub updater signing key'
    }
    if ((($test -and -not $UnsignedTechnical) -or $MsixThumbprint) -and $MsixThumbprint -notmatch '^[0-9A-Fa-f]{40}$') { throw 'A valid local MSIX test certificate thumbprint is required' }
}
$env:WORLDBUILD_BUILD_CHANNEL = $Channel.ToLower()
$env:WORLDBUILD_PACKAGE_MODE = $Mode.ToLower()
if ($storeIdentity) { $env:WORLDBUILD_STORE_IDENTITY = $identityJson }
$tauri = Join-Path $repo 'node_modules/.bin/tauri.cmd'
if (-not (Test-Path -LiteralPath $tauri)) { throw 'Installed Tauri CLI is missing; run npm ci' }
Push-Location $repo
try {
    if ($UpdaterKeyPath) { $env:TAURI_SIGNING_PRIVATE_KEY = $key }
    $begin = (Get-Date).ToUniversalTime()
    if ($Channel -eq 'Github') {
        & $tauri build --bundles nsis --config $overlayPath -- --locked
    } else {
        & $tauri build --no-bundle --config $overlayPath -- --locked
    }
    if ($LASTEXITCODE -ne 0) { throw "Tauri $Channel build failed: $LASTEXITCODE" }
    if ($Channel -eq 'Github') {
        $targetRoot = if ($env:CARGO_TARGET_DIR) { [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR) } else { Join-Path $repo 'src-tauri/target' }
        $bundle = Join-Path $targetRoot 'release/bundle/nsis'
        $files = @(Get-ChildItem -LiteralPath $bundle -File | Where-Object {
            $_.LastWriteTimeUtc -ge $begin -and ($_.Name -like '*-setup.exe' -or $_.Name -like '*-setup.exe.sig')
        })
        $expectedSignatures = if ($UnsignedTechnical) { 0 } else { 1 }
        if (@($files | Where-Object Extension -eq '.exe').Count -ne 1 -or @($files | Where-Object Extension -eq '.sig').Count -ne $expectedSignatures) {
            throw 'Expected exactly one new NSIS installer and its updater signature'
        }
        foreach ($file in $files) { Copy-Item -LiteralPath $file.FullName -Destination (Join-Path $out $file.Name) }
    } else {
        $targetRoot = if ($env:CARGO_TARGET_DIR) { [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR) } else { Join-Path $repo 'src-tauri/target' }
        $exe = Join-Path $targetRoot 'release/worldbuild-tool.exe'
        if (-not (Test-Path -LiteralPath $exe)) { throw 'Store app EXE was not built' }
        # A fresh stage prevents a removed resource from leaking into a later
        # package when the same output directory is used again.
        $stage = Join-Path $out ('msix-stage-' + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Force -Path $stage | Out-Null
        Copy-Item -LiteralPath $exe -Destination (Join-Path $stage 'worldbuild-tool.exe')
        foreach ($entry in $base.bundle.resources.PSObject.Properties) {
            $source = [IO.Path]::GetFullPath((Join-Path $repo ('src-tauri/' + $entry.Name)))
            if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw "Missing bundled resource: $($entry.Name)" }
            $target = Join-Path $stage $entry.Value
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $target) | Out-Null
            Copy-Item -LiteralPath $source -Destination $target
        }
        $assets = Join-Path $stage 'Assets'
        New-Item -ItemType Directory -Force -Path $assets | Out-Null
        foreach ($logo in @('StoreLogo.png','Square150x150Logo.png','Square44x44Logo.png')) {
            Copy-Item -LiteralPath (Join-Path $repo ('packaging/msix/Assets/' + $logo)) -Destination (Join-Path $assets $logo)
        }
        $packageName = if ($storeIdentity) { $storeIdentity.name } else { 'Dreamrugi.WorldbuildTool.ELocalTest' }
        $publisher = if ($storeIdentity) { $storeIdentity.publisher } else { 'CN=Dreamrugi Worldbuild Tool E Local Test' }
        $publisherDisplayName = if ($storeIdentity) { $storeIdentity.publisherDisplayName } else { 'Dreamrugi Local Test' }
        if ($MsixThumbprint) {
            $certificate = Get-ChildItem Cert:\CurrentUser\My | Where-Object Thumbprint -eq $MsixThumbprint | Select-Object -First 1
            if (-not $certificate -or -not $certificate.HasPrivateKey -or $certificate.Subject -cne $publisher) { throw 'MSIX local signing certificate must match the exact package Publisher' }
        }
        # Submission remains unsigned for Store re-signing. Local signing makes
        # a separate copy, with the exact same manifest/resource/executable payload.
        function XmlValue([string]$Value) { [Security.SecurityElement]::Escape($Value) }
        $template = Get-Content -LiteralPath (Join-Path $repo 'packaging/msix/AppxManifest.xml.in') -Raw
        $manifest = $template.Replace('{{IDENTITY}}',(XmlValue $packageName)).Replace('{{PUBLISHER}}',(XmlValue $publisher)).Replace('{{VERSION}}',$msixVersion).Replace('{{DISPLAY_NAME}}',(XmlValue $displayName)).Replace('{{PUBLISHER_DISPLAY_NAME}}',(XmlValue $publisherDisplayName))
        [IO.File]::WriteAllText((Join-Path $stage 'AppxManifest.xml'),$manifest,[Text.UTF8Encoding]::new($false))
        $sdk = 'C:\Program Files (x86)\Windows Kits\10\bin\10.0.19041.0\x64'
        $makeAppx = Join-Path $sdk 'MakeAppx.exe'
        $signTool = Join-Path $sdk 'SignTool.exe'
        if (-not (Test-Path -LiteralPath $makeAppx) -or -not (Test-Path -LiteralPath $signTool)) { throw 'Windows SDK MakeAppx/SignTool are missing' }
        $package = Join-Path $out ("{0}_{1}_x64.msix" -f $packageName,$msixVersion)
        & $makeAppx pack /d $stage /p $package /o
        if ($LASTEXITCODE -ne 0) { throw 'MakeAppx pack failed' }
        if ($MsixThumbprint) {
            $localPackage = Join-Path $out ("{0}_{1}_x64-local-signed.msix" -f $packageName,$msixVersion)
            Copy-Item -LiteralPath $package -Destination $localPackage
            & $signTool sign /fd SHA256 /sha1 $MsixThumbprint /s My $localPackage
            if ($LASTEXITCODE -ne 0) { throw 'MSIX SignTool sign failed' }
            & $signTool verify /pa /v $localPackage
            if ($LASTEXITCODE -ne 0) { throw 'MSIX local signature verification failed; certificate trust was not changed' }
        }
        $payload = @(Get-ChildItem -LiteralPath $stage -Recurse -File | ForEach-Object {
            [ordered]@{ path=[IO.Path]::GetRelativePath($stage,$_.FullName).Replace('\','/'); bytes=$_.Length; sha256=(Get-FileHash -LiteralPath $_.FullName).Hash.ToLowerInvariant() }
        })
        [ordered]@{ packageName=$packageName; publisher=$publisher; publisherDisplayName=$publisherDisplayName; storeIdentity=$storeIdentity; localSigned=[bool]$MsixThumbprint; payload=$payload } |
            ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $out 'msix-payload.json') -Encoding utf8
        $resolvedOut = [IO.Path]::GetFullPath($out).TrimEnd([IO.Path]::DirectorySeparatorChar)
        $resolvedStage = [IO.Path]::GetFullPath($stage)
        if (-not $resolvedStage.StartsWith($resolvedOut + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
            throw 'MSIX stage escaped the output directory'
        }
        Remove-Item -LiteralPath $resolvedStage -Recurse -Force
    }
    $packages = @(Get-ChildItem -LiteralPath $out -File | Where-Object { $_.Extension -in '.exe','.msix','.sig' })
    $fingerprints = foreach ($item in $packages) {
        [ordered]@{ name=$item.Name; bytes=$item.Length; sha256=(Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash.ToLowerInvariant() }
    }
    $sourceInfo = if (Test-Path -LiteralPath (Join-Path $repo 'SOURCE.json')) { Get-Content -LiteralPath (Join-Path $repo 'SOURCE.json') -Raw | ConvertFrom-Json } else { $null }
    $sourceHead = if ($sourceInfo) { $sourceInfo.base_head } else { git -C $repo rev-parse HEAD }
    $candidateId = if ($sourceInfo) { $sourceInfo.candidate_id } else { $null }
    [ordered]@{ channel=$Channel; mode=$Mode; version=$version; msixVersion=$msixVersion; identifier=$identifier; sourceHead=$sourceHead; candidateId=$candidateId; technicalOnly=[bool]$UnsignedTechnical; artifacts=@($fingerprints) } |
        ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $out 'package-metadata.json') -Encoding utf8
    [pscustomobject]@{ Channel=$Channel; Mode=$Mode; Version=$version; MsixVersion=$msixVersion; Output=$out; Artifacts=$packages.Count }
} finally {
    Pop-Location
    Remove-Item Env:\WORLDBUILD_BUILD_CHANNEL,Env:\WORLDBUILD_PACKAGE_MODE,Env:\TAURI_SIGNING_PRIVATE_KEY,Env:\TAURI_SIGNING_PRIVATE_KEY_PASSWORD,Env:\WORLDBUILD_STORE_IDENTITY -ErrorAction SilentlyContinue
}
