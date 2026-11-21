# Install OpenLight for the current Windows user.
#
# Run from an extracted package:
#   powershell -ExecutionPolicy Bypass -File .\install.ps1
#
# No administrator rights are needed. The binaries go to
# %LOCALAPPDATA%\Programs\OpenLight and Start Menu shortcuts are created for
# the current user. The machine PATH is not changed.

[CmdletBinding()]
param(
    [string]$Source = $PSScriptRoot
)

$ErrorActionPreference = "Stop"
$destination = Join-Path $env:LOCALAPPDATA "Programs\OpenLight"
$startMenu = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs"
New-Item -ItemType Directory -Force -Path $destination | Out-Null
New-Item -ItemType Directory -Force -Path $startMenu | Out-Null

$console = Join-Path $Source "openlight.exe"
if (-not (Test-Path $console)) {
    throw "openlight.exe was not found beside install.ps1; extract the whole package first"
}
Copy-Item $console $destination

$desktop = Join-Path $Source "openlight-desktop.exe"
if (Test-Path $desktop) {
    Copy-Item $desktop $destination
}

$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut((Join-Path $startMenu "OpenLight.lnk"))
$shortcut.TargetPath = Join-Path $destination "openlight.exe"
$shortcut.WorkingDirectory = $destination
$shortcut.Description = "OpenLight console"
$shortcut.Save()

if (Test-Path $desktop) {
    $shortcut = $shell.CreateShortcut((Join-Path $startMenu "OpenLight Desktop.lnk"))
    $shortcut.TargetPath = Join-Path $destination "openlight-desktop.exe"
    $shortcut.WorkingDirectory = $destination
    $shortcut.Description = "OpenLight desktop"
    $shortcut.Save()
}

Write-Host "Installed OpenLight in $destination"
Write-Host "Launch it from the Start Menu, or run $(Join-Path $destination 'openlight.exe') --probe"
