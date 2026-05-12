param(
  [int]$ApiPort = 52333,
  [int]$IppPort = 631,
  [string]$PrinterName = ''
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
Set-Location $root

Write-Host '[1/4] Building lanPrint...'
go build -o dist\lanPrint.exe ./cmd/lanPrint

Write-Host '[2/4] Starting lanPrint service process...'
$proc = Start-Process -FilePath dist\lanPrint.exe -PassThru
Start-Sleep -Seconds 2

Write-Host '[3/4] Checking API health...'
Invoke-RestMethod -Uri "http://127.0.0.1:$ApiPort/api/v1/stats" -Method GET | Out-Null

if (-not $PrinterName) {
  $printers = Invoke-RestMethod -Uri "http://127.0.0.1:$ApiPort/api/v1/printers" -Method GET
  if ($printers.Count -gt 0) { $PrinterName = $printers[0].name }
}

if ($PrinterName) {
  Write-Host "[4/4] Sharing printer: $PrinterName"
  Invoke-RestMethod -Uri "http://127.0.0.1:$ApiPort/api/v1/printers/share" -Method POST -ContentType 'application/json' -Body (@{name=$PrinterName;shared=$true} | ConvertTo-Json) | Out-Null
} else {
  Write-Host '[4/4] No local printer found, skipping share step.'
}

Write-Host "Server verify done. PID=$($proc.Id). Keep this machine running for client test."
