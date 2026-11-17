param(
    [Parameter(Mandatory = $true)][string]$DeviceId,
    [Parameter(Mandatory = $true)][string]$OutFile,
    [int]$Resolution = 300,
    [ValidateSet('Color', 'Grayscale', 'BlackWhite')][string]$Mode = 'Color',
    [ValidateSet('Flatbed', 'ADF')][string]$Source = 'Flatbed'
)

$ErrorActionPreference = 'Stop'

$FORMAT_PNG = '{B96B3CAE-0728-11D3-9D7B-0000F81EF32E}'

$manager = New-Object -ComObject WIA.DeviceManager
$info = $null
foreach ($candidate in $manager.DeviceInfos) {
    if ($candidate.DeviceID -eq $DeviceId) {
        $info = $candidate
        break
    }
}
if ($null -eq $info) {
    # Fall back to the first scanner if the id changed after a driver update.
    foreach ($candidate in $manager.DeviceInfos) {
        if ($candidate.Type -eq 1) { $info = $candidate; break }
    }
}
if ($null -eq $info) {
    throw 'No WIA scanner found'
}

$device = $info.Connect()
$item = $device.Items.Item(1)
if ($Source -eq 'ADF') {
    try { $item.Properties.Item(3088).Value = 1 } catch { }
}

function Set-WiaProperty([int]$id, [object]$value) {
    try { $item.Properties.Item($id).Value = $value } catch { }
}

# 6146: current intent (1 color, 2 grayscale, 4 black/white)
$intent = 1
if ($Mode -eq 'Grayscale') { $intent = 2 }
if ($Mode -eq 'BlackWhite') { $intent = 4 }
Set-WiaProperty 6146 $intent
# 6147/6148: horizontal and vertical resolution
Set-WiaProperty 6147 $Resolution
Set-WiaProperty 6148 $Resolution

$image = $item.Transfer($FORMAT_PNG)
$image.SaveFile($OutFile)
