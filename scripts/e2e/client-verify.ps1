param(
  [Parameter(Mandatory=$true)][string]$ServerHost,
  [int]$ApiPort = 52333
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
Set-Location $root

Write-Host '[1/5] Building lanPrint client binary...'
go build -o dist\lanPrint.exe ./cmd/lanPrint

Write-Host '[2/5] Starting client process...'
$proc = Start-Process -FilePath dist\lanPrint.exe -PassThru
Start-Sleep -Seconds 2

Write-Host '[3/5] Registering remote server device...'
$addBody = @{name="remote-$ServerHost"; address=$ServerHost; port=$ApiPort} | ConvertTo-Json
Invoke-RestMethod -Uri "http://127.0.0.1:$ApiPort/api/v1/client/devices" -Method POST -ContentType 'application/json' -Body $addBody | Out-Null

Write-Host '[4/5] Fetching remote shared printers...'
$devices = Invoke-RestMethod -Uri "http://127.0.0.1:$ApiPort/api/v1/client/devices" -Method GET
$device = $devices | Where-Object { $_.address -eq $ServerHost } | Select-Object -First 1
if (-not $device) { throw 'Device was not added.' }
$remotePrinters = Invoke-RestMethod -Uri "http://127.0.0.1:$ApiPort/api/v1/client/devices/$($device.id)/printers" -Method GET
if (-not $remotePrinters -or $remotePrinters.Count -eq 0) { throw 'No shared printer found on server.' }

$printerName = $remotePrinters[0].name
Write-Host "[5/5] Connecting remote printer: $printerName"
$connectBody = @{device_id=[int64]$device.id; printer_name=$printerName} | ConvertTo-Json
Invoke-RestMethod -Uri "http://127.0.0.1:$ApiPort/api/v1/client/connect" -Method POST -ContentType 'application/json' -Body $connectBody | Out-Null

Write-Host "Client verify done. PID=$($proc.Id). Printer connected: $printerName"
