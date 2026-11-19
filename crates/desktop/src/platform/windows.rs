use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use lanprint_core::{LocalPrinter, PrinterCapabilities, PrinterPreferences, ScanRequest};
use tracing::{debug, warn};

use super::ScannerInfo;
use crate::error::{AppError, Result};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn powershell(script: &str) -> Result<String> {
    let mut cmd = Command::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        script,
    ])
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = cmd
        .output()
        .map_err(|e| AppError::Platform(format!("cannot start PowerShell: {e}")))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Platform(err.trim().to_string()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn ps_quote(value: &str) -> String {
    value.replace('\'', "''")
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WmiPrinter {
    #[serde(default)]
    name: String,
    #[serde(default)]
    driver_name: String,
    #[serde(default)]
    port_name: String,
    #[serde(default)]
    default: Option<bool>,
    #[serde(default)]
    work_offline: Option<bool>,
    #[serde(default)]
    printer_status: Option<u32>,
    #[serde(default)]
    color: Option<bool>,
    #[serde(default)]
    duplex: Option<bool>,
    #[serde(default)]
    horizontal_resolution: Option<u32>,
    #[serde(default)]
    local: Option<bool>,
}

/// Enumerate installed printers through WMI. This works on Windows 7 (via
/// `Get-WmiObject`) and on current Windows (via `Get-CimInstance`).
pub fn list_printers() -> Result<Vec<LocalPrinter>> {
    let script = r#"
$ErrorActionPreference = 'SilentlyContinue'
$p = @(Get-WmiObject -Class Win32_Printer)
if ($p.Count -eq 0) { $p = @(Get-CimInstance -ClassName Win32_Printer) }
$p | Select-Object Name,DriverName,PortName,Default,WorkOffline,PrinterStatus,Color,Duplex,HorizontalResolution,PaperSizesSupported,Local | ConvertTo-Json -Compress
"#;
    let text = powershell(script)?;
    let text = text.trim();
    if text.is_empty() || text == "null" {
        return Ok(Vec::new());
    }
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| AppError::Platform(format!("invalid printer list from WMI: {e}")))?;
    let entries: Vec<WmiPrinter> = match value {
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect(),
        other => serde_json::from_value(other).into_iter().collect(),
    };
    let scanners = cached_scanners();
    let mut printers = Vec::new();
    for p in entries {
        if p.name.is_empty() {
            continue;
        }
        let status = if p.work_offline.unwrap_or(false) {
            "offline"
        } else {
            match p.printer_status.unwrap_or(0) {
                3 => "idle",
                4 | 5 | 6 => "printing",
                7 => "offline",
                _ => "unknown",
            }
        };
        let driver = super::windows_pdf::driver_caps(&p.name, &p.port_name);
        let max_paper = if driver.papers.iter().any(|p| p == "A3") {
            "A3"
        } else {
            "A4"
        };
        let mut caps = PrinterCapabilities {
            color: driver.color.unwrap_or(p.color.unwrap_or(false)),
            duplex: driver.duplex.unwrap_or(p.duplex.unwrap_or(false)),
            max_paper: max_paper.into(),
            make_model: p.driver_name.clone(),
            supports_scan: false,
            scanner_id: String::new(),
            resolutions: if driver.resolutions.is_empty() {
                match p.horizontal_resolution {
                    Some(dpi) if dpi > 0 => vec![dpi],
                    _ => vec![300, 600],
                }
            } else {
                driver.resolutions.clone()
            },
            formats: vec!["pdf".into(), "raw".into(), "ps".into(), "text".into()],
            options: Vec::new(),
            papers: driver.papers.clone(),
            sources: driver.sources.clone(),
            duplex_modes: if driver.duplex.unwrap_or(p.duplex.unwrap_or(false)) {
                vec!["long_edge".into(), "short_edge".into()]
            } else {
                Vec::new()
            },
            booklet: driver.duplex.unwrap_or(p.duplex.unwrap_or(false)),
            max_copies: driver.max_copies,
        };
        if let Some(scanner) = match_scanner(&p.name, &scanners) {
            caps.supports_scan = true;
            caps.scanner_id = scanner.id.clone();
        }
        printers.push(LocalPrinter {
            name: p.name.clone(),
            driver: p.driver_name,
            port: p.port_name,
            is_default: p.default.unwrap_or(false),
            status: status.into(),
            capabilities: caps,
            preferences: driver.preferences.clone(),
            available: p.local.unwrap_or(true),
        });
    }
    printers.sort_by(|a, b| b.is_default.cmp(&a.is_default).then(a.name.cmp(&b.name)));
    Ok(printers)
}

fn match_scanner<'a>(printer: &str, scanners: &'a [ScannerInfo]) -> Option<&'a ScannerInfo> {
    if scanners.is_empty() {
        return None;
    }
    let p = printer.to_lowercase();
    for s in scanners {
        let n = s.name.to_lowercase();
        if n.contains(&p) || p.contains(&n) {
            return Some(s);
        }
    }
    // AIO model names commonly differ between the print and scan drivers.
    let tokens: Vec<&str> = p
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 3)
        .collect();
    for s in scanners {
        let n = s.name.to_lowercase();
        if tokens.iter().any(|t| n.contains(t)) {
            return Some(s);
        }
    }
    None
}

fn cached_scanners() -> Vec<ScannerInfo> {
    static CACHE: OnceLock<Mutex<Option<(Instant, Vec<ScannerInfo>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().unwrap();
    if let Some((at, scanners)) = guard.as_ref() {
        if at.elapsed() < Duration::from_secs(300) {
            return scanners.clone();
        }
    }
    let scanners = detect_scanners().unwrap_or_default();
    *guard = Some((Instant::now(), scanners.clone()));
    scanners
}

pub fn detect_scanners() -> Result<Vec<ScannerInfo>> {
    let script = r#"
$ErrorActionPreference = 'SilentlyContinue'
try {
  $dm = New-Object -ComObject WIA.DeviceManager
  $out = @()
  foreach ($info in $dm.DeviceInfos) {
    $name = ''
    try { $name = $info.Properties.Item('Name').Value } catch {}
    if ($info.Type -eq 1 -and $name) {
      $out += [pscustomobject]@{ id = $info.DeviceID; name = $name }
    }
  }
  $out | ConvertTo-Json -Compress
} catch { }
"#;
    let text = powershell(script).unwrap_or_default();
    let text = text.trim();
    if text.is_empty() || text == "null" {
        return Ok(Vec::new());
    }
    let value: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
    let entries = match value {
        serde_json::Value::Array(items) => items,
        other => vec![other],
    };
    let mut out = Vec::new();
    for v in entries {
        let id = v.get("id").and_then(|v| v.as_str()).unwrap_or_default();
        let name = v.get("name").and_then(|v| v.as_str()).unwrap_or_default();
        if !id.is_empty() {
            out.push(ScannerInfo {
                id: id.to_string(),
                name: name.to_string(),
                adf: name.to_lowercase().contains("adf")
                    || name.to_lowercase().contains("mfp")
                    || name.to_lowercase().contains("multifunction"),
            });
        }
    }
    Ok(out)
}

pub fn scan(request: &ScanRequest, scanner_id: &str) -> Result<Vec<u8>> {
    let out = tempfile::Builder::new()
        .prefix("lanprint-scan-")
        .suffix(".png")
        .tempfile()
        .map_err(|e| AppError::Platform(e.to_string()))?;
    let path = out.path().to_path_buf();
    let script = include_str!("../../scripts/windows_scan.ps1");
    let script_path = tempfile::Builder::new()
        .prefix("lanprint-scan-")
        .suffix(".ps1")
        .tempfile()
        .map_err(|e| AppError::Platform(e.to_string()))?;
    std::fs::write(script_path.path(), script)
        .map_err(|e| AppError::Platform(format!("cannot write scanner helper: {e}")))?;
    let mode = if request.color { "Color" } else { "Grayscale" };
    let source = if request.source == "adf" {
        "ADF"
    } else {
        "Flatbed"
    };
    let mut cmd = Command::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-STA",
        "-File",
    ])
    .arg(script_path.path())
    .arg("-DeviceId")
    .arg(scanner_id)
    .arg("-OutFile")
    .arg(&path)
    .arg("-Resolution")
    .arg(request.resolution.to_string())
    .arg("-Mode")
    .arg(mode)
    .arg("-Source")
    .arg(source)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = cmd
        .output()
        .map_err(|e| AppError::Platform(format!("cannot start scanner: {e}")))?;
    if !output.status.success() {
        return Err(AppError::Scan(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    let data = std::fs::read(&path)
        .map_err(|e| AppError::Scan(format!("scanner produced no output: {e}")))?;
    if data.is_empty() {
        return Err(AppError::Scan("scanner produced an empty file".into()));
    }
    Ok(data)
}

/// Send a document to a local Windows printer.
pub fn print_document(
    printer: &str,
    data: &[u8],
    content_type: &str,
    _filename: &str,
    prefs: &PrinterPreferences,
) -> Result<()> {
    let lower = content_type.to_ascii_lowercase();
    if lower.contains("pdf") || data.starts_with(b"%PDF") {
        match super::windows_pdf::print_pdf(printer, data, prefs) {
            Ok(()) => return Ok(()),
            Err(err) => warn!("PDF pipeline failed for {printer}: {err}; trying PrintTo"),
        }
    }
    if lower.contains("postscript") || data.starts_with(b"%!PS") {
        // A client queue that uses the host's PostScript driver spools raw
        // PostScript. Send it straight to the printer before trying a handler.
        match print_raw(printer, data) {
            Ok(()) => return Ok(()),
            Err(err) => warn!("raw PostScript failed for {printer}: {err}; trying PrintTo"),
        }
    }
    if lower.contains("pdf") || lower.contains("postscript") {
        // Fallback for machines without the Windows PDF engine (Windows 7) or
        // documents the engine rejects: use the installed PDF/PS handler.
        let suffix = if lower.contains("pdf") { ".pdf" } else { ".ps" };
        let mut tmp = tempfile::Builder::new()
            .prefix("lanprint-print-")
            .suffix(suffix)
            .tempfile()
            .map_err(|e| AppError::Print(e.to_string()))?;
        tmp.write_all(data)
            .map_err(|e| AppError::Print(e.to_string()))?;
        let path = tmp.path().to_path_buf();
        let script = format!(
            "$ErrorActionPreference='Stop'; Start-Process -FilePath '{}' -Verb PrintTo -ArgumentList '{}' -Wait",
            ps_quote(&path.display().to_string()),
            ps_quote(printer)
        );
        if powershell(&script).is_ok() {
            return Ok(());
        }
        warn!("PrintTo handler failed for {printer}; falling back to raw spool data");
    }
    print_raw(printer, data)
}

#[cfg(windows)]
fn print_raw(printer: &str, data: &[u8]) -> Result<()> {
    use std::ptr::null_mut;
    use winapi::shared::minwindef::DWORD;
    use winapi::um::winspool::{
        ClosePrinter, EndDocPrinter, EndPagePrinter, OpenPrinterW, StartDocPrinterW,
        StartPagePrinter, WritePrinter, DOC_INFO_1W,
    };

    let wide_name: Vec<u16> = printer.encode_utf16().chain(std::iter::once(0)).collect();
    let mut handle = null_mut();
    let ok = unsafe { OpenPrinterW(wide_name.as_ptr() as *mut _, &mut handle, null_mut()) };
    if ok == 0 {
        return Err(AppError::Print(format!(
            "OpenPrinter failed for {printer}: {}",
            std::io::Error::last_os_error()
        )));
    }
    let mut doc_name: Vec<u16> = "lanPrint job"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut datatype: Vec<u16> = "RAW".encode_utf16().chain(std::iter::once(0)).collect();
    let mut info = DOC_INFO_1W {
        pDocName: doc_name.as_mut_ptr(),
        pOutputFile: null_mut(),
        pDatatype: datatype.as_mut_ptr(),
    };
    let result = unsafe {
        if StartDocPrinterW(handle, 1, &mut info as *mut _ as *mut _) == 0 {
            Err(AppError::Print("StartDocPrinter failed".into()))
        } else if StartPagePrinter(handle) == 0 {
            let _ = EndDocPrinter(handle);
            Err(AppError::Print("StartPagePrinter failed".into()))
        } else {
            let mut written: DWORD = 0;
            let ok = WritePrinter(
                handle,
                data.as_ptr() as *mut _,
                data.len() as DWORD,
                &mut written,
            );
            let _ = EndPagePrinter(handle);
            let _ = EndDocPrinter(handle);
            if ok == 0 || written as usize != data.len() {
                Err(AppError::Print(format!(
                    "WritePrinter wrote {written}/{} bytes",
                    data.len()
                )))
            } else {
                Ok(())
            }
        }
    };
    unsafe {
        ClosePrinter(handle);
    }
    result
}

/// Install a local virtual queue backed by a TCP port that the desktop app
/// listens on and forwards to the remote host.
///
/// When the host's driver is installed on this machine the queue uses it, so
/// the Windows print dialog exposes the same duplex, colour, tray and finishing
/// options as the host printer and the spooler emits native driver data. When
/// the driver is missing the queue falls back to *Microsoft Print To PDF*, and
/// the host renders the PDF with lanPrint's own pipeline.
pub fn install_remote_printer(
    local_name: &str,
    display_name: &str,
    driver: &str,
    tcp_port: u16,
    prefs: &PrinterPreferences,
) -> Result<()> {
    let port_name = format!("lanPrint-{tcp_port}");
    let script = r#"
$ErrorActionPreference = 'Stop'
$portName = '__PORT_NAME__'
$port = __PORT__
$localName = '__LOCAL__'
$driver = '__DRIVER__'
$paper = '__PAPER__'
$color = __COLOR__
$duplex = '__DUPLEX__'
try {
  Add-PrinterPort -Name $portName -PrinterHostAddress '127.0.0.1' -PortNumber $port -ErrorAction Stop
} catch {
  $vbs = Get-ChildItem "$env:WINDIR\System32\Printing_Admin_Scripts\*\prnport.vbs" | Select-Object -First 1
  if ($vbs) { cscript //nologo $vbs.FullName -a -r $portName -h 127.0.0.1 -o raw -n $port }
}
$installed = @()
try { $installed = @(Get-PrinterDriver | Select-Object -ExpandProperty Name) } catch {}
if ($driver -and -not ($installed -contains $driver)) {
  try { Add-PrinterDriver -Name $driver -ErrorAction Stop; $installed += $driver } catch {}
}
try { Remove-Printer -Name $localName -ErrorAction SilentlyContinue } catch {}
$candidates = @()
if ($driver) { $candidates += $driver }
$candidates += 'Microsoft Print To PDF'
$candidates += 'Generic / Text Only'
$added = $false
foreach ($candidate in $candidates) {
  if (-not ($installed -contains $candidate)) { continue }
  try {
    Add-Printer -Name $localName -DriverName $candidate -PortName $portName -ErrorAction Stop
    $added = $true
    break
  } catch {}
}
if (-not $added) {
  rundll32 printui.dll,PrintUIEntry /if /b "$localName" /f "$env:WINDIR\inf\ntprint.inf" /r "$portName" /m "Generic / Text Only"
}
try {
  Set-PrintConfiguration -PrinterName $localName -PaperSize $paper -Color:$color -Duplexing $duplex -ErrorAction Stop
} catch {}
"#
    .replace("__PORT_NAME__", &ps_quote(&port_name))
    .replace("__PORT__", &tcp_port.to_string())
    .replace("__LOCAL__", &ps_quote(local_name))
    .replace("__DRIVER__", &ps_quote(driver))
    .replace("__PAPER__", &ps_quote(&prefs.paper))
    .replace("__COLOR__", if prefs.color { "$true" } else { "$false" })
    .replace("__DUPLEX__", duplex_setting(prefs));
    powershell(&script)?;
    debug!("installed Windows virtual printer {local_name} on {port_name} ({display_name})");
    Ok(())
}

fn duplex_setting(prefs: &PrinterPreferences) -> &'static str {
    match prefs.duplex {
        lanprint_core::DuplexMode::LongEdge => "TwoSidedLongEdge",
        lanprint_core::DuplexMode::ShortEdge => "TwoSidedShortEdge",
        lanprint_core::DuplexMode::Simplex => "OneSided",
    }
}

/// Apply print settings to the local queue defaults so applications that use
/// the Windows print dialog start from the values chosen in lanPrint.
pub fn apply_printer_preferences(local_name: &str, prefs: &PrinterPreferences) -> Result<()> {
    let mut args = vec![format!("-PrinterName '{}'", ps_quote(local_name))];
    if !prefs.paper.is_empty() {
        args.push(format!("-PaperSize '{}'", ps_quote(&prefs.paper)));
    }
    args.push(format!(
        "-Color:{}",
        if prefs.color { "$true" } else { "$false" }
    ));
    args.push(format!("-Duplexing {}", duplex_setting(prefs)));
    let script = format!(
        "$ErrorActionPreference='Stop'; Set-PrintConfiguration {} | Out-Null",
        args.join(" ")
    );
    powershell(&script).map(|_| ())
}

/// Switch an existing queue to the host printer's driver when it is installed
/// on this machine, so the Windows print dialog exposes the same features.
pub fn sync_printer_driver(
    local_name: &str,
    driver: &str,
    prefs: &PrinterPreferences,
) -> Result<()> {
    if driver.is_empty() {
        return Ok(());
    }
    let paper = if prefs.paper.is_empty() {
        String::new()
    } else {
        format!("-PaperSize '{}'", ps_quote(&prefs.paper))
    };
    let script = r#"
$ErrorActionPreference = 'Stop'
$name = '__NAME__'
$driver = '__DRIVER__'
try { $current = (Get-Printer -Name $name -ErrorAction Stop).DriverName } catch { exit 0 }
if ($current -eq $driver) { exit 0 }
$installed = @()
try { $installed = @(Get-PrinterDriver | Select-Object -ExpandProperty Name) } catch {}
if (-not ($installed -contains $driver)) {
  try { Add-PrinterDriver -Name $driver -ErrorAction Stop; $installed += $driver } catch {}
}
if (-not ($installed -contains $driver)) { exit 0 }
try { Set-Printer -Name $name -DriverName $driver -ErrorAction Stop } catch { exit 0 }
try {
  Set-PrintConfiguration -PrinterName $name __PAPER__ -Color:__COLOR__ -Duplexing __DUPLEX__ -ErrorAction Stop
} catch {}
"#
    .replace("__NAME__", &ps_quote(local_name))
    .replace("__DRIVER__", &ps_quote(driver))
    .replace("__PAPER__", &paper)
    .replace("__COLOR__", if prefs.color { "$true" } else { "$false" })
    .replace("__DUPLEX__", duplex_setting(prefs));
    powershell(&script).map(|_| ())
}

pub fn remove_remote_printer(local_name: &str) -> Result<()> {
    let script = format!(
        "$ErrorActionPreference='SilentlyContinue'; Remove-Printer -Name '{}'; if ($?) {{ exit 0 }}; rundll32 printui.dll,PrintUIEntry /dl /n \"{}\" /q",
        ps_quote(local_name),
        ps_quote(local_name)
    );
    powershell(&script)?;
    Ok(())
}

pub fn open_path(path: &Path) -> Result<()> {
    open::that(path).map_err(|e| AppError::Platform(format!("cannot open {}: {e}", path.display())))
}

pub fn log_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lanPrint")
        .join("logs")
}
