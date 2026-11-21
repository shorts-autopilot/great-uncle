# Remove the current user's OpenLight installation.
#
#   powershell -ExecutionPolicy Bypass -File .\uninstall.ps1
#
# This removes only the install directory and the two Start Menu shortcuts;
# show files and desk files live elsewhere and are not touched.

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$destination = Join-Path $env:LOCALAPPDATA "Programs\OpenLight"
$startMenu = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs"

foreach ($name in @("OpenLight.lnk", "OpenLight Desktop.lnk")) {
    $shortcut = Join-Path $startMenu $name
    if (Test-Path $shortcut) {
        Remove-Item -Force $shortcut
    }
}
if (Test-Path $destination) {
    Remove-Item -Recurse -Force $destination
}
Write-Host "Removed OpenLight from $destination"
