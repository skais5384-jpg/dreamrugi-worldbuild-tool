param(
    [Parameter(Mandatory=$true)][ValidateSet('Generate','BackupAndVerify','Build')][string]$Action,
    [Parameter(Mandatory=$true)][string]$Directory,
    [string]$BackupDirectory,
    [string]$Snapshot,
    [string]$OutputDirectory,
    [string]$DependencySourceDirectory
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$directoryPath = [IO.Path]::GetFullPath($Directory)
if ($directoryPath.StartsWith($repo + [IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)) { throw 'Deployment keys must be outside the repository' }
$key = Join-Path $directoryPath 'updater.key'
$tauri = Join-Path $repo 'node_modules/.bin/tauri.cmd'
function Protect-Directory([string]$Target) {
    New-Item -ItemType Directory -Force -Path $Target | Out-Null
    $acl = Get-Acl -LiteralPath $Target
    $acl.SetAccessRuleProtection($true,$false)
    $userSid = [Security.Principal.WindowsIdentity]::GetCurrent().User
    $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($userSid,'FullControl','ContainerInherit,ObjectInherit','None','Allow'))
    $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new('SYSTEM','FullControl','ContainerInherit,ObjectInherit','None','Allow'))
    Set-Acl -LiteralPath $Target -AclObject $acl
}
if ($Action -eq 'Generate') {
    if (Test-Path -LiteralPath $directoryPath) { throw 'Key directory already exists; no overwrite' }
    Protect-Directory $directoryPath
    # User types the passphrase at Tauri's secure interactive prompt. No -p or --ci.
    & $tauri signer generate --write-keys $key
    if ($LASTEXITCODE -ne 0) { throw 'Key generation failed' }
    [IO.File]::WriteAllText((Join-Path $directoryPath 'DEPLOYMENT_KEY'),'Password-encrypted deployment key. Keep passphrase separately; never share it in chat.')
    Write-Host 'Next: BackupAndVerify with a user-managed separate backup location.'
    return
}
if ($Action -eq 'Build') {
    if (-not $Snapshot -or -not $OutputDirectory) { throw 'Snapshot and new output directory are required' }
    $password = Read-Host '배포 키 암호 (현재 빌드 프로세스에만 전달)' -AsSecureString
    $pointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($password)
    try {
        $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($pointer)
        & (Join-Path $PSScriptRoot 'build-release-candidate.ps1') -Snapshot $Snapshot -OutputDirectory $OutputDirectory -UpdaterKeyPath $key -ReleaseKeyReceipt (Join-Path $directoryPath 'release-key-receipt.json') -DependencySourceDirectory $DependencySourceDirectory
    } finally {
        Remove-Item Env:\TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
        [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($pointer)
        $password.Dispose()
    }
    return
}
if (-not $BackupDirectory) { throw 'Independent user-managed backup directory is required' }
$backup = [IO.Path]::GetFullPath($BackupDirectory)
if (Test-Path -LiteralPath $backup) { throw 'Backup destination already exists; no overwrite' }
if ($backup.StartsWith($directoryPath,[StringComparison]::OrdinalIgnoreCase) -or $backup.StartsWith($repo,[StringComparison]::OrdinalIgnoreCase)) { throw 'Backup must be separately managed, outside key directory and repository' }
$keyText = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String(([IO.File]::ReadAllText($key)).Trim()))
$packet = [Convert]::FromBase64String(($keyText -split '\r?\n')[1])
if ($packet.Length -ne 158 -or $packet[2] -ne 83 -or $packet[3] -ne 99) { throw 'Key must be password encrypted; empty-password/Test keys are rejected' }
New-Item -ItemType Directory -Path $backup | Out-Null
Copy-Item -LiteralPath $key,"$key.pub" -Destination $backup
$restore = Join-Path $directoryPath ('restore-verification-' + [guid]::NewGuid().ToString('N'))
Protect-Directory $restore
$restoredKey = Join-Path $restore 'updater.key'
Copy-Item -LiteralPath (Join-Path $backup 'updater.key') -Destination $restoredKey
Copy-Item -LiteralPath (Join-Path $backup 'updater.key.pub') -Destination "$restoredKey.pub"
$sample = Join-Path $restore 'restore-sample.txt'
[IO.File]::WriteAllText($sample,'Worldbuild deployment key portable encrypted-backup restore verification.')
$password = Read-Host '복원된 키의 암호 (화면/채팅/로그에 기록하지 않습니다)' -AsSecureString
$pointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($password)
try {
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($pointer)
    & $tauri signer sign --private-key-path $restoredKey $sample
    if ($LASTEXITCODE -ne 0) { throw 'Restored deployment key signing failed' }
    $resultText = & node (Join-Path $PSScriptRoot 'verify-updater-signature.mjs') $sample "$restoredKey.pub"
    if ($LASTEXITCODE -ne 0) { throw 'Restored key/public key verification failed' }
    $result = $resultText | ConvertFrom-Json
    $receipt = [ordered]@{ role='Release'; portableBackupVerified=$true; newDeviceTested=$false; publicKeySha256=$result.publicKeySha256; sampleSha256=(Get-FileHash -LiteralPath $sample).Hash.ToLowerInvariant(); encryptedBackupSha256=(Get-FileHash -LiteralPath (Join-Path $backup 'updater.key')).Hash.ToLowerInvariant(); verifiedUtc=[DateTime]::UtcNow.ToString('o') }
    $receipt | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $directoryPath 'release-key-receipt.json') -Encoding utf8
    Copy-Item -LiteralPath (Join-Path $directoryPath 'release-key-receipt.json') -Destination $backup
    Write-Host "Portable encrypted backup restored and verified. Receipt: $(Join-Path $directoryPath 'release-key-receipt.json')"
} finally {
    Remove-Item Env:\TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
    [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($pointer)
    $password.Dispose()
    # The restored private key remains encrypted. Preserve original and backup.
}
