# Destructive only to a disposable Windows test VM/runner, never a user's PC.
param([switch]$IsolatedMachine)
$ErrorActionPreference = 'Stop'
if (-not $IsolatedMachine) { throw 'Use only on an isolated disposable Windows VM/CI runner, with no PSP attached.' }
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $admin) { throw 'This isolated VM test needs an administrator token.' }
$source = Get-Content (Join-Path $PSScriptRoot '../gateway/src/access.rs') -Raw
$marker = 'const DRIVER_PS1: &str = r#"'
$body = $source.Split(@($marker), [StringSplitOptions]::None)[1].Split(@('"#;'), [StringSplitOptions]::None)[0]
if (-not $body) { throw 'Embedded installer not found' }
$subject = 'CN=pspkit-usbnet driver (made on this computer)'
function TrustEntries {
    $entries = @()
    foreach ($s in 'Root','TrustedPublisher') {
        Get-ChildItem "Cert:\LocalMachine\$s" | Where-Object Subject -eq $subject | ForEach-Object { $entries += "$s/$($_.Thumbprint)" }
    }
    return $entries
}
$dir = Join-Path $env:TEMP ('usbnet-driver-check-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $dir | Out-Null
try {
    $before = @(TrustEntries)
    $good = Join-Path $dir 'driver.ps1'
    $body | Set-Content $good
    $null = [scriptblock]::Create($body)
    & powershell -NoProfile -ExecutionPolicy Bypass -File $good
    if ($LASTEXITCODE -ne 0) { Get-Content (Join-Path $dir 'driver.log'); throw 'actual installer failed' }
    $after = @(TrustEntries)
    foreach ($entry in $before) { if ($entry -notin $after) { throw 'existing trust entry removed' } }
    $bad = Join-Path $dir 'forced-failure.ps1'
    ("function New-FileCatalog { throw 'intentional catalog failure' }`n" + $body) | Set-Content $bad
    & powershell -NoProfile -ExecutionPolicy Bypass -File $bad
    if ($LASTEXITCODE -eq 0) { throw 'forced catalog failure was ignored' }
    $failed = @(TrustEntries)
    if (@(Compare-Object $after $failed).Count -ne 0) { throw 'failed attempt changed trust entries' }
    if (@(Get-ChildItem Cert:\LocalMachine\My | Where-Object Subject -eq $subject).Count -ne 0) { throw 'private signing key left behind' }
    Write-Output 'PASS: actual WinUSB installation; old trust entries retained; forced catalog failure rolls back new trust and private key'
} finally {
    Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
}

# The intentional failing child process must not become the CI step exit code.
$global:LASTEXITCODE = 0
