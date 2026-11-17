# One-click release build for Windows.
#
#   powershell -ExecutionPolicy Bypass -File .\scripts\build.ps1
#
# Outputs to dist\. Requires Rust (MSVC toolchain).
$ErrorActionPreference = "Stop"

$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $root

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo not found. Install Rust from https://rustup.rs/ first."
}

$versionLine = Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
$version = if ($versionLine) { $versionLine.Matches[0].Groups[1].Value } else { "0.0.0" }
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root "target" }

Write-Host "==> Building lanPrint $version (release)"
cargo build --release --locked -p lanprint-desktop -p lanprint-bridge
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$out = Join-Path $root "dist"
New-Item -ItemType Directory -Force -Path $out | Out-Null
$dir = Join-Path $out "lanPrint_${version}_Windows_x86_64"
New-Item -ItemType Directory -Force -Path $dir | Out-Null

Copy-Item "$targetDir\release\lanPrint.exe" "$dir\lanPrint.exe" -Force
Copy-Item -Path README.md,README_EN.md,LICENSE -Destination $dir -Force
$zip = Join-Path $out "lanPrint_${version}_Windows_x86_64.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path "$dir\*" -DestinationPath $zip

Copy-Item "$targetDir\release\lanprint-bridge.exe" "$out\lanprint-bridge_${version}_Windows_x86_64.exe" -Force

$iscc = "C:\Program Files (x86)\Inno Setup 6\ISCC.exe"
if (Test-Path $iscc) {
    Write-Host "==> Building Windows installer"
    & $iscc "/DMyAppVersion=$version" "/DMySource=$targetDir\release\lanPrint.exe" "packaging\windows\lanPrint.iss"
    if ($LASTEXITCODE -eq 0 -and (Test-Path "dist\lanPrint-$version-setup.exe")) {
        Move-Item "dist\lanPrint-$version-setup.exe" "dist\lanPrint-${version}-setup.exe" -Force
    }
} else {
    Write-Host "Inno Setup not found; skipping installer. Install it to build the setup executable."
}

Write-Host ""
Write-Host "Build complete:"
Get-ChildItem $out -File | ForEach-Object { Write-Host ("  " + $_.Name) }
Write-Host ""
Write-Host "Desktop binary: $targetDir\release\lanPrint.exe"
Write-Host "Bridge binary:  $targetDir\release\lanprint-bridge.exe"
exit 0
