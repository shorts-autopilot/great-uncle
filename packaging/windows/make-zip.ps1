# Build a portable Windows package.
#
#   .\packaging\windows\make-zip.ps1
#   .\packaging\windows\make-zip.ps1 -NoGui
#   .\packaging\windows\make-zip.ps1 -Profile debug
#
# Run from a Developer PowerShell for Visual Studio (or any shell where cargo
# is on PATH). The package is written to dist\ and contains the two binaries,
# docs, examples and the per-user install scripts.

[CmdletBinding()]
param(
    [ValidateSet("debug", "release")]
    [string]$Profile = "release",
    [switch]$NoGui,
    [string]$Target = ""
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
Push-Location $root
try {
    $versionLine = Select-String -Path (Join-Path $root "Cargo.toml") -Pattern '^version = "(.*)"' |
        Select-Object -First 1
    if (-not $versionLine) {
        throw "Cargo.toml has no workspace version"
    }
    $version = $versionLine.Matches[0].Groups[1].Value
    $cargoProfile = if ($Profile -eq "debug") { "dev" } else { "release" }
    $profileDir = if ($Profile -eq "debug") { "debug" } else { "release" }
    $hostTarget = ((& rustc -vV | Select-String '^host:').Line -split ':', 2)[1].Trim()
    $target = if ($Target) { $Target } else { $hostTarget }
    $targetRoot = Join-Path (Join-Path $root "target") $target
    $binaryDir = Join-Path $targetRoot $profileDir
    $name = "openlight-$version-$target"
    $stage = Join-Path $root "dist\$name"
    $archive = Join-Path $root "dist\$name.zip"

    & cargo build -p openlight-console --profile $cargoProfile --target $target
    if ($LASTEXITCODE -ne 0) { throw "console build failed" }
    if (-not $NoGui) {
        & cargo build -p openlight-desktop --features gui --profile $cargoProfile --target $target
        if ($LASTEXITCODE -ne 0) { throw "desktop build failed" }
    }

    if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
    if (Test-Path $archive) { Remove-Item -Force $archive }
    New-Item -ItemType Directory -Force -Path $stage | Out-Null

    Copy-Item (Join-Path $binaryDir "openlight.exe") $stage
    if (-not $NoGui) {
        Copy-Item (Join-Path $binaryDir "openlight-desktop.exe") $stage
    }
    foreach ($file in @("README.md", "CHANGELOG.md", "SECURITY.md", "CONTRIBUTING.md", "LICENSE")) {
        Copy-Item (Join-Path $root $file) $stage
    }
    Copy-Item -Recurse (Join-Path $root "docs") $stage
    Copy-Item -Recurse (Join-Path $root "examples") $stage
    Copy-Item (Join-Path $PSScriptRoot "install.ps1") $stage
    Copy-Item (Join-Path $PSScriptRoot "uninstall.ps1") $stage

    Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $archive
    Write-Host $archive
} finally {
    Pop-Location
}
