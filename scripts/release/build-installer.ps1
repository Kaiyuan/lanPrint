param(
  [string]$ISCC = "C:\Program Files (x86)\Inno Setup 6\ISCC.exe"
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
Set-Location $root

$env:CGO_ENABLED = "1"; go build -ldflags "-H=windowsgui" -o dist\\lanPrint.exe ./cmd/lanPrint

if (-not (Test-Path $ISCC)) {
  throw "Inno Setup compiler not found: $ISCC"
}

& $ISCC packaging\inno\lanPrint.iss
Write-Host 'Installer generated under dist\lanPrint-setup-win.exe'
