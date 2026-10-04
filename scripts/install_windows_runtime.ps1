# Explicit per-user named-pipe prototype. Requires Windows PowerShell 5.1+.
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][ValidateSet('install','enrol','revoke','uninstall')][string]$Action,
    [string]$Binary,
    [switch]$UsePackagedBinary,
    [switch]$NoAutoUpdate,
    [switch]$RulesOnly,
    [switch]$Offline,
    [switch]$ModelManagement,
    [string]$ModelSource,
    [string]$ModelWorker,
    [string]$ModelLibrary,
    [string]$InstallDirectory,
    [ValidatePattern('^[a-zA-Z0-9_-]{1,64}$')][string]$Principal
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
# A PowerShell 7 parent can pass its module path to Windows PowerShell 5.1.
# Load the security commands from this host, rather than an incompatible module.
Import-Module (Join-Path $PSHOME 'Modules/Microsoft.PowerShell.Security/Microsoft.PowerShell.Security.psd1') -ErrorAction Stop
$UserSid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
$SystemSid = New-Object System.Security.Principal.SecurityIdentifier('S-1-5-18')
$Root = if ($InstallDirectory) { [IO.Path]::GetFullPath($InstallDirectory) } else { Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'E2EM' }
$PipeName = '\\.\pipe\e2em-' + $UserSid.Value
$Executable = Join-Path $Root 'e2emd.exe'
$GrantsPath = Join-Path $Root 'grants.json'
$Marker = Join-Path $Root 'installation.json'

# Process paths use long names, while Python temporary paths can contain 8.3
# aliases. Compare their canonical spelling before removing any private state.
if (-not ('E2EMSetupPaths' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;
public static class E2EMSetupPaths {
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern uint GetLongPathName(string path, StringBuilder output, uint length);
    public static string Canonical(string path) {
        var output = new StringBuilder(32768);
        uint length = GetLongPathName(path, output, (uint)output.Capacity);
        if (length == 0 || length >= output.Capacity)
            throw new Win32Exception(Marshal.GetLastWin32Error());
        return output.ToString();
    }
}
'@
}

function Protect-Path([string]$Path, [bool]$Directory) {
    if ($Directory) {
        $Acl = New-Object System.Security.AccessControl.DirectorySecurity
        $Flags = [System.Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit'
    } else {
        $Acl = New-Object System.Security.AccessControl.FileSecurity
        $Flags = [System.Security.AccessControl.InheritanceFlags]::None
    }
    $Acl.SetOwner($UserSid)
    $Acl.SetAccessRuleProtection($true, $false)
    foreach ($Sid in @($UserSid, $SystemSid)) {
        $Rule = New-Object System.Security.AccessControl.FileSystemAccessRule($Sid, 'FullControl', $Flags, 'None', 'Allow')
        $Acl.AddAccessRule($Rule)
    }
    Set-Acl -LiteralPath $Path -AclObject $Acl
}
function Assert-PrivatePath([string]$Path, [bool]$Directory, [bool]$AllowInheritance = $false) {
    $Item = Get-Item -LiteralPath $Path -Force
    if ($Item.PSIsContainer -ne $Directory -or ($Item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Invalid private installation path' }
    $Acl = Get-Acl -LiteralPath $Path
    if ($Acl.GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $UserSid.Value) { throw 'Installation path belongs to another user' }
    if (-not $AllowInheritance -and -not $Acl.AreAccessRulesProtected) { throw 'Installation ACL must disable inheritance' }
    $OwnerAllowed = $false
    foreach ($Rule in $Acl.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier])) {
        if ($Rule.AccessControlType -eq 'Allow') {
            if ($Rule.IdentityReference.Value -notin @($UserSid.Value, $SystemSid.Value)) { throw 'Installation ACL is not private' }
            if ($Rule.IdentityReference.Value -eq $UserSid.Value -and
                ($Rule.FileSystemRights -band [System.Security.AccessControl.FileSystemRights]::FullControl) -eq [System.Security.AccessControl.FileSystemRights]::FullControl) { $OwnerAllowed = $true }
        }
    }
    if (-not $OwnerAllowed) { throw 'Installation ACL has no explicit owner access' }
}
function Save-Json([string]$Path, $Value) {
    # ReplaceFile preserves the destination DACL. Reject a broadened/reparse
    # destination before putting a fresh secret into it.
    if (Test-Path -LiteralPath $Path) { Assert-PrivatePath $Path $false }
    # Temporary files inherit the already private installation-directory ACL.
    $Temporary = Join-Path $Root ([Guid]::NewGuid().ToString('N') + '.tmp')
    try {
        [IO.File]::WriteAllText($Temporary, ($Value | ConvertTo-Json -Depth 12), (New-Object Text.UTF8Encoding($false)))
        Protect-Path $Temporary $false
        if (Test-Path -LiteralPath $Path) { [IO.File]::Replace($Temporary, $Path, [System.Management.Automation.Language.NullString]::Value) }
        else { [IO.File]::Move($Temporary, $Path) }
    } finally { if (Test-Path -LiteralPath $Temporary) { Remove-Item -LiteralPath $Temporary } }
}
function New-Secret {
    $Bytes = New-Object byte[] 32
    $Random = [Security.Cryptography.RandomNumberGenerator]::Create()
    try { $Random.GetBytes($Bytes) } finally { $Random.Dispose() }
    return (($Bytes | ForEach-Object { $_.ToString('x2') }) -join '')
}
if ($Action -eq 'install') {
    if (Test-Path -LiteralPath $Root) { throw 'Refusing to overwrite an existing installation' }
    if (-not $Binary -or -not (Test-Path -LiteralPath $Binary -PathType Leaf)) { throw 'Specify an existing runtime binary' }
    New-Item -ItemType Directory -Path $Root | Out-Null
    Protect-Path $Root $true
    if ($UsePackagedBinary) { $Executable = [IO.Path]::GetFullPath($Binary) }
    else {
        Copy-Item -LiteralPath $Binary -Destination $Executable
        Protect-Path $Executable $false
    }
    Save-Json $GrantsPath @{provider=('project-' + [Guid]::NewGuid().ToString('N')); grants=@()}
    $InstallMarker = @{version=1; platform='windows'}
    if ($UsePackagedBinary) { $InstallMarker.packaged_binary = $Executable }
    Save-Json $Marker $InstallMarker
    if (-not $RulesOnly) {
        $Backend = Split-Path -Parent ([IO.Path]::GetFullPath($Binary))
        if (-not $ModelWorker) { $ModelWorker = Join-Path $Backend 'e2em-inference.exe' }
        if (-not $ModelLibrary) { $ModelLibrary = Join-Path $Backend 'onnxruntime.dll' }
        $Bundled = Join-Path $Backend 'models/gandalf'
        if (-not $ModelSource) { $ModelSource = if (Test-Path -LiteralPath $Bundled -PathType Container) { $Bundled } else { 'gandalf' } }
        if ($Offline -and -not (Test-Path -LiteralPath $ModelSource -PathType Container)) { throw 'Offline setup requires a local model package' }
        $ModelArgs = @('--grants', $GrantsPath)
        if ($Offline) { $ModelArgs += '--offline' }
        & $Executable @ModelArgs --model-init --model-worker ([IO.Path]::GetFullPath($ModelWorker)) --model-library ([IO.Path]::GetFullPath($ModelLibrary))
        if ($LASTEXITCODE -ne 0) { throw 'Model store initialization failed' }
        $InstallArgs = @('--model-install', $ModelSource)
        if ($Offline -or $NoAutoUpdate) { $InstallArgs += '--model-no-update' }
        & $Executable @ModelArgs @InstallArgs
        if ($LASTEXITCODE -ne 0) { throw "Model provisioning failed. Resume with: & '$Executable' --grants '$GrantsPath' --model-install '$ModelSource'" }
    }
    $UpdateArgument = if ($NoAutoUpdate -or $Offline) { '' } else { ' --auto-update' }
    if ($Offline) { $UpdateArgument += ' --offline' }
    if ($RulesOnly) { $UpdateArgument += ' --rules-only' }
    Write-Output "Installed. Run in your user session: & '$Executable' --pipe '$PipeName' --grants '$GrantsPath'$UpdateArgument"
    exit
}
Assert-PrivatePath $Root $true
Assert-PrivatePath $Marker $false
$Installation = Get-Content -LiteralPath $Marker -Raw | ConvertFrom-Json
if ($Installation.version -ne 1 -or $Installation.platform -ne 'windows') { throw 'Unmanaged installation' }
$Packaged = $Installation.PSObject.Properties.Name -contains 'packaged_binary'
if ($Packaged) {
    if (-not [IO.Path]::IsPathRooted($Installation.packaged_binary)) { throw 'Invalid packaged executable path' }
    $Executable = $Installation.packaged_binary
}
if ($Action -eq 'uninstall') {
    # Never remove a running process's grant registry. Stop the foreground service
    # with Ctrl+C first. Inspect exact executable paths, not just process names.
    if (Test-Path -LiteralPath $Executable) {
        $ManagedExecutable = [E2EMSetupPaths]::Canonical($Executable)
        if (Get-Process -Name e2emd -ErrorAction SilentlyContinue | Where-Object {
            $_.Path -and [E2EMSetupPaths]::Canonical($_.Path) -eq $ManagedExecutable
        }) { throw 'Stop the installed runtime before uninstalling' }
    }
    foreach ($File in @(Get-ChildItem -LiteralPath $Root -Filter 'app-*.json')) { Remove-Item -LiteralPath $File.FullName }
    $Managed = @($GrantsPath, $Marker)
    foreach ($Name in @('onboarding.json','start-runtime.ps1')) {
        $Path = Join-Path $Root $Name
        if (Test-Path -LiteralPath $Path) { $Managed += $Path }
    }
    $StartupKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
    $StartupValue = Get-ItemProperty -LiteralPath $StartupKey -Name 'E2EM Runtime' -ErrorAction SilentlyContinue
    if ($StartupValue -and $StartupValue.'E2EM Runtime'.Contains((Join-Path $Root 'start-runtime.ps1'))) {
        Remove-ItemProperty -LiteralPath $StartupKey -Name 'E2EM Runtime'
    }
    if (-not $Packaged) { $Managed += $Executable }
    foreach ($Path in $Managed) { Remove-Item -LiteralPath $Path }
    $Setup = Join-Path $Root 'setup'
    if (Test-Path -LiteralPath $Setup) {
        Assert-PrivatePath $Setup $true $true
        foreach ($Name in @('session.json','session.lock')) {
            $Path = Join-Path $Setup $Name
            if (Test-Path -LiteralPath $Path) { Remove-Item -LiteralPath $Path }
        }
        if (-not (Get-ChildItem -LiteralPath $Setup -Force)) { Remove-Item -LiteralPath $Setup }
    }
    $Models = Join-Path $Root 'models'
    if (Test-Path -LiteralPath $Models) {
        Assert-PrivatePath $Models $true $true
        # Remove only the owned model store after rejecting reparse points.
        foreach ($Item in @(Get-ChildItem -LiteralPath $Models -Recurse -Force)) {
            if ($Item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Model store contains a reparse point' }
        }
        Remove-Item -LiteralPath $Models -Recurse
    }
    $ModelsConfig = Join-Path $Root 'models.json'
    if (Test-Path -LiteralPath $ModelsConfig) { Remove-Item -LiteralPath $ModelsConfig }
    $Updates = Join-Path $Root 'updates'
    if (Test-Path -LiteralPath $Updates) {
        Assert-PrivatePath $Updates $true $true
        foreach ($File in @(Get-ChildItem -LiteralPath $Updates -Force)) {
            if (-not $File.PSIsContainer -and ($File.Name.StartsWith('.e2em-update-') -or $File.Name -in @('state.json','status.json','supervisor.lock','check-request.json') -or
                $File.Name -match '^e2emd-\d+\.\d+\.\d+(-[a-zA-Z0-9.-]+)?\.exe$')) {
                Remove-Item -LiteralPath $File.FullName
            }
        }
        if (-not (Get-ChildItem -LiteralPath $Updates -Force)) { Remove-Item -LiteralPath $Updates }
    }
    if (-not (Get-ChildItem -LiteralPath $Root -Force)) { Remove-Item -LiteralPath $Root }
    Write-Output 'Removed managed prototype files.'
    exit
}
if (-not $Principal) { throw 'Specify an application principal' }
Assert-PrivatePath $GrantsPath $false
$Registry = Get-Content -LiteralPath $GrantsPath -Raw | ConvertFrom-Json
$Registry.grants = @($Registry.grants | Where-Object { $_.principal -ne $Principal })
$Credential = Join-Path $Root ('app-' + $Principal + '.json')
if ($Action -eq 'revoke') {
    Save-Json $GrantsPath $Registry
    if (Test-Path -LiteralPath $Credential) { Remove-Item -LiteralPath $Credential }
    exit
}
if ($Registry.grants.Count -ge 64) { throw 'Grant limit reached' }
$Secret = New-Secret
$Grant = @{principal=$Principal; sid=$UserSid.Value; secret=$Secret}
if ($ModelManagement) { $Grant.model_management = $true }
$Registry.grants += $Grant
Save-Json $GrantsPath $Registry
Save-Json $Credential @{kind='project'; socket_path=$PipeName; provider=$Registry.provider; principal=$Principal; secret=$Secret}
Write-Output "Private credentials: $Credential"
