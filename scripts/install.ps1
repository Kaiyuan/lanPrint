# lanPrint desktop installer for Windows.
#
#   iwr -useb https://raw.githubusercontent.com/Kaiyuan/lanPrint/main/scripts/install.ps1 | iex
#
# Environment:
#   $env:LANPRINT_VERSION = "v2.0.2"  (default: latest)

$ErrorActionPreference = "Stop"
$repo = "Kaiyuan/lanPrint"
$arch = if ([Environment]::Is64BitOperatingSystem) { "x86_64" } else { "i686" }
$version = $env:LANPRINT_VERSION

if ([string]::IsNullOrWhiteSpace($version)) {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases/latest"
    $version = $release.tag_name
}
if (-not $version.StartsWith("v")) { $version = "v$version" }
$ver = $version.TrimStart("v")
$asset = "lanPrint_${ver}_Windows_${arch}.zip"
$url = "https://github.com/$repo/releases/download/$version/$asset"

$temp = Join-Path $env:TEMP "lanprint-install"
New-Item -ItemType Directory -Force -Path $temp | Out-Null
$zip = Join-Path $temp $asset
Write-Host "Downloading $url"
Invoke-WebRequest -Uri $url -OutFile $zip
Expand-Archive -Path $zip -DestinationPath $temp -Force

$target = Join-Path $env:LOCALAPPDATA "Programs\lanPrint"
New-Item -ItemType Directory -Force -Path $target | Out-Null
Copy-Item (Join-Path $temp "lanPrint.exe") (Join-Path $target "lanPrint.exe") -Force
Copy-Item (Join-Path $temp "README.md") $target -ErrorAction SilentlyContinue

$exe = Join-Path $target "lanPrint.exe"
$startup = [Environment]::GetFolderPath("Startup")
$shortcut = Join-Path $startup "lanPrint.lnk"
$shell = New-Object -ComObject WScript.Shell
$link = $shell.CreateShortcut($shortcut)
$link.TargetPath = $exe
$link.Arguments = "--tray"
$link.WorkingDirectory = $target
$link.Save()

# Allow inbound lanPrint traffic through Windows Firewall.
try {
    New-NetFirewallRule -DisplayName "lanPrint" -Direction Inbound -Program $exe -Action Allow -Profile Any | Out-Null
} catch {
    Write-Host "Could not add the firewall rule (run as administrator if clients cannot connect)."
}

Start-Process -FilePath $exe -ArgumentList "--tray"
Write-Host "lanPrint installed to $target and started in the system tray."
