param(
  [switch]$LegacyWin7
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
Set-Location $root

New-Item -ItemType Directory -Force dist\matrix | Out-Null

if ($LegacyWin7) {
  $env:GOTOOLCHAIN = 'go1.20.14'
  Write-Host 'Using Go toolchain go1.20.14 for Windows 7-compatible build artifacts.'
}

function Build-One {
  param(
    [string]$GOOS,
    [string]$GOARCH,
    [string]$GOARM = ''
  )

  $ext = if ($GOOS -eq 'windows') { '.exe' } else { '' }
  $suffix = if ($GOARM) { "v$GOARM" } else { '' }
  $out = "dist/matrix/lanPrint-$GOOS-$GOARCH$suffix$ext"
  $ldflags = '-s -w'
  $cgo = '0'

  if ($GOOS -eq 'windows') {
    $ldflags = '-s -w -H=windowsgui -buildid='
    $cgo = '1'
  }

  Write-Host "Building $out"
  $env:GOOS = $GOOS
  $env:GOARCH = $GOARCH
  $env:CGO_ENABLED = $cgo

  if ($GOARM) {
    $env:GOARM = $GOARM
    go build -trimpath -ldflags $ldflags -o $out ./cmd/lanPrint
    Remove-Item Env:GOARM -ErrorAction SilentlyContinue
  } else {
    go build -trimpath -ldflags $ldflags -o $out ./cmd/lanPrint
  }
}

Build-One windows amd64
Build-One windows 386
Build-One darwin amd64
Build-One darwin arm64
Build-One linux amd64
Build-One linux 386
Build-One linux arm64
Build-One linux arm 6
Build-One linux arm 7

Remove-Item Env:GOOS -ErrorAction SilentlyContinue
Remove-Item Env:GOARCH -ErrorAction SilentlyContinue
Remove-Item Env:CGO_ENABLED -ErrorAction SilentlyContinue

Write-Host 'Build matrix done: dist/matrix'
