# lanPrint One-Click Installer for Windows
# Usage: iwr -useb https://raw.githubusercontent.com/kaiyuan/lanPrint/main/scripts/install.ps1 | iex

$repo = "kaiyuan/lanPrint"
$latest = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases/latest"
$tag = $latest.tag_name

$arch = if ($env:PROCESSOR_ARCHITECTURE -eq "AMD64") { "amd64" } else { "arm64" }
$filename = "lanPrint_Windows_$($arch).zip"
$url = "https://github.com/$repo/releases/download/$tag/$filename"

$tmp = [System.IO.Path]::GetTempFileName()
Remove-Item $tmp
New-Item -ItemType Directory -Path $tmp | Out-Null

Write-Host "Downloading lanPrint $tag..."
Invoke-WebRequest -Uri $url -OutFile "$tmp/lanprint.zip"

Write-Host "Extracting..."
Expand-Archive -Path "$tmp/lanprint.zip" -DestinationPath $tmp -Force

$installDir = "$env:ProgramFiles/lanPrint"
if (!(Test-Path $installDir)) {
    New-Item -ItemType Directory -Path $installDir -Force | Out-Null
}

Write-Host "Installing to $installDir..."
Move-Item -Path "$tmp/lanPrint.exe" -Destination "$installDir/lanPrint.exe" -Force

# Add to PATH (User)
$path = [Environment]::GetEnvironmentVariable("Path", "User")
if ($path -notlike "*$installDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$path;$installDir", "User")
}

Write-Host "lanPrint installed successfully!"
Write-Host "Please restart your terminal and run 'lanPrint' with administrator privileges."

Remove-Item -Path $tmp -Recurse -Force
