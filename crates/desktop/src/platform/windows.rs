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

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

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
        let duplex = driver.duplex.unwrap_or(false) || p.duplex.unwrap_or(false);
        let color = driver.color.unwrap_or(false) || p.color.unwrap_or(false);
        let mut caps = PrinterCapabilities {
            color,
            duplex,
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
            duplex_modes: if duplex {
                vec!["long_edge".into(), "short_edge".into()]
            } else {
                Vec::new()
            },
            booklet: duplex,
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

/// One printer operation handed to an elevated helper instance of lanPrint.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PrinterJob {
    pub local_name: String,
    pub display_name: String,
    pub driver: String,
    pub id: u32,
    pub tcp_port: u16,
    pub preferences: PrinterPreferences,
    #[serde(default)]
    pub upgrade: bool,
    /// Host features injected into the generated queue files after an IPP
    /// queue has been created.
    #[serde(default)]
    pub capabilities: Option<super::windows_caps::QueueCaps>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct PrinterJobResult {
    ok: bool,
    #[serde(default)]
    method: String,
    #[serde(default)]
    error: String,
}

fn is_elevated() -> bool {
    use winapi::um::handleapi::CloseHandle;
    use winapi::um::processthreadsapi::{GetCurrentProcess, OpenProcessToken};
    use winapi::um::securitybaseapi::GetTokenInformation;
    use winapi::um::winnt::{TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut size = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut _,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

/// Entry point for the elevated helper instance. Reads the job descriptions,
/// applies them in order and writes the outcomes next to the job file.
pub fn run_printer_job(job_path: Option<&String>, out_path: Option<&String>) -> i32 {
    let (Some(job_path), Some(out_path)) = (job_path, out_path) else {
        return 2;
    };
    let results = std::fs::read_to_string(job_path)
        .map_err(|e| AppError::Platform(format!("cannot read printer job: {e}")))
        .and_then(|raw| {
            let raw = raw.trim_start_matches('\u{feff}');
            serde_json::from_str::<Vec<PrinterJob>>(raw)
                .map_err(|e| AppError::Platform(format!("invalid printer job: {e}")))
        })
        .map(|jobs| {
            jobs.iter()
                .map(|job| match run_job(job) {
                    Ok(method) => PrinterJobResult {
                        ok: true,
                        method,
                        error: String::new(),
                    },
                    Err(err) => PrinterJobResult {
                        ok: false,
                        method: String::new(),
                        error: err.to_string(),
                    },
                })
                .collect::<Vec<_>>()
        });
    match results {
        Ok(payload) => {
            let code = if payload.iter().all(|result| result.ok) {
                0
            } else {
                2
            };
            if let Ok(json) = serde_json::to_string(&payload) {
                let _ = std::fs::write(out_path, json);
            }
            code
        }
        Err(err) => {
            let payload = vec![PrinterJobResult {
                ok: false,
                method: String::new(),
                error: err.to_string(),
            }];
            if let Ok(json) = serde_json::to_string(&payload) {
                let _ = std::fs::write(out_path, json);
            }
            2
        }
    }
}

fn apply_jobs(jobs: &[PrinterJob]) -> Vec<PrinterJobResult> {
    if is_elevated() {
        jobs.iter().map(apply_in_process).collect()
    } else {
        run_elevated_jobs(jobs)
    }
}

fn apply_in_process(job: &PrinterJob) -> PrinterJobResult {
    match run_job(job) {
        Ok(method) => PrinterJobResult {
            ok: true,
            method,
            error: String::new(),
        },
        Err(err) => PrinterJobResult {
            ok: false,
            method: String::new(),
            error: err.to_string(),
        },
    }
}

fn run_job(job: &PrinterJob) -> Result<String> {
    let method = match install_with_ipp(job) {
        Ok(method) => method,
        Err(err) if job.upgrade => return Err(err),
        Err(err) => {
            debug!(
                "IPP install for {} failed: {err}; using a TCP queue",
                job.local_name
            );
            return install_with_tcp(job).map(|driver| format!("TCP/{driver}"));
        }
    };
    if method == "IPP" {
        if let Some(capabilities) = job.capabilities.as_ref() {
            match super::windows_caps::apply(
                &job.local_name,
                capabilities,
                &job.preferences.paper,
            ) {
                Ok(()) => {
                    // The queue now knows the host's media sizes, so the
                    // preferred paper, colour mode and duplex mode can be
                    // stored as queue defaults.
                    if let Err(err) = apply_printer_preferences(&job.local_name, &job.preferences)
                    {
                        debug!("could not apply queue defaults for {}: {err}", job.local_name);
                    }
                }
                Err(err) => {
                    debug!(
                        "capability injection for {} skipped: {err}",
                        job.local_name
                    );
                }
            }
        }
    }
    Ok(method)
}

/// Create the queue through Windows' native IPP stack. The spooler asks lanPrint
/// for the host printer's real capabilities, so the print dialog exposes the
/// same duplex, colour, tray and media options as the shared printer.
fn install_with_ipp(job: &PrinterJob) -> Result<String> {
    if !crate::ipp::available() {
        return Err(AppError::Platform(
            "lanPrint IPP endpoint is not ready".into(),
        ));
    }
    let ipp_url = crate::ipp::http_uri(job.id);
    let script = r#"
$ErrorActionPreference = 'Stop'
$name = '__LOCAL__'
$ipp = '__IPP__'
$paper = '__PAPER__'
$color = __COLOR__
$duplex = '__DUPLEX__'
$upgrade = __UPGRADE__
$oldPort = ''
$oldDriver = ''
if ($upgrade) {
  $existing = Get-Printer -Name $name -ErrorAction SilentlyContinue
  if ($existing -and $existing.PortName -like 'WSD-*') { 'IPP'; exit 0 }
  if ($existing) { $oldPort = $existing.PortName; $oldDriver = $existing.DriverName }
}
if (Get-Printer -Name $name -ErrorAction SilentlyContinue) {
  Remove-Printer -Name $name -ErrorAction SilentlyContinue
  Start-Sleep -Milliseconds 400
}
try {
  Add-Printer -Name $name -IppURL $ipp -ErrorAction Stop
} catch {
  if ($upgrade -and $oldPort -and $oldDriver) {
    try { Add-Printer -Name $name -DriverName $oldDriver -PortName $oldPort -ErrorAction Stop } catch {}
  }
  throw
}
try {
  Set-PrintConfiguration -PrinterName $name -PaperSize $paper -Color:$color -Duplexing $duplex -ErrorAction Stop
} catch {}
'IPP'
"#
    .replace("__LOCAL__", &ps_quote(&job.local_name))
    .replace("__IPP__", &ps_quote(&ipp_url))
    .replace("__PAPER__", &ps_quote(&job.preferences.paper))
    .replace("__COLOR__", if job.preferences.color { "$true" } else { "$false" })
    .replace("__DUPLEX__", duplex_setting(&job.preferences))
    .replace("__UPGRADE__", if job.upgrade { "$true" } else { "$false" });
    let output = powershell(&script)?;
    if output.contains("IPP") {
        Ok("IPP".into())
    } else {
        Err(AppError::Platform(
            "Windows did not create an IPP queue".into(),
        ))
    }
}

/// Fallback queue: a raw TCP port served by lanPrint, using the host driver when
/// it is installed or Microsoft Print To PDF otherwise.
fn install_with_tcp(job: &PrinterJob) -> Result<String> {
    let port_name = format!("lanPrint-{}", job.tcp_port);
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
try { Remove-Printer -Name $localName -ErrorAction SilentlyContinue } catch {}
if ($driver -and -not ($installed -contains $driver)) {
  try { Add-PrinterDriver -Name $driver -ErrorAction Stop; $installed += $driver } catch {}
}
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
try { (Get-Printer -Name $localName -ErrorAction Stop).DriverName } catch { '' }
"#
    .replace("__PORT_NAME__", &ps_quote(&port_name))
    .replace("__PORT__", &job.tcp_port.to_string())
    .replace("__LOCAL__", &ps_quote(&job.local_name))
    .replace("__DRIVER__", &ps_quote(&job.driver))
    .replace("__PAPER__", &ps_quote(&job.preferences.paper))
    .replace("__COLOR__", if job.preferences.color { "$true" } else { "$false" })
    .replace("__DUPLEX__", duplex_setting(&job.preferences));
    let output = powershell(&script)?;
    let chosen = output.lines().last().unwrap_or_default().trim().to_string();
    if chosen.is_empty() {
        Err(AppError::Platform("Windows did not create a queue".into()))
    } else {
        Ok(chosen)
    }
}

fn run_elevated_jobs(jobs: &[PrinterJob]) -> Vec<PrinterJobResult> {
    use winapi::um::shellapi::ShellExecuteW;
    use winapi::um::winuser::SW_HIDE;

    let failed = |error: String| -> Vec<PrinterJobResult> {
        jobs.iter()
            .map(|_| PrinterJobResult {
                ok: false,
                method: String::new(),
                error: error.clone(),
            })
            .collect()
    };
    let dir = std::env::temp_dir();
    let stem = format!("lanprint-printer-{}", uuid::Uuid::new_v4());
    let job_path = dir.join(format!("{stem}.json"));
    let out_path = dir.join(format!("{stem}.out.json"));
    let encoded = match serde_json::to_vec(jobs) {
        Ok(encoded) => encoded,
        Err(err) => return failed(err.to_string()),
    };
    if let Err(err) = std::fs::write(&job_path, encoded) {
        return failed(format!("cannot prepare printer install: {err}"));
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            let _ = std::fs::remove_file(&job_path);
            return failed(err.to_string());
        }
    };
    let params = format!(
        "--apply-printer \"{}\" \"{}\"",
        job_path.display(),
        out_path.display()
    );
    let verb = wide("runas");
    let file = wide(&exe.display().to_string());
    let params_w = wide(&params);
    let launched = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            params_w.as_ptr(),
            std::ptr::null(),
            SW_HIDE,
        )
    } as isize;
    if launched <= 32 {
        let _ = std::fs::remove_file(&job_path);
        return failed("administrator rights are required to add the printer".into());
    }
    let seconds = (60 * jobs.len().max(1) as u64).clamp(120, 600);
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        if let Ok(raw) = std::fs::read_to_string(&out_path) {
            let _ = std::fs::remove_file(&out_path);
            let _ = std::fs::remove_file(&job_path);
            return match serde_json::from_str::<Vec<PrinterJobResult>>(&raw) {
                Ok(results) if results.len() == jobs.len() => results,
                Ok(results) => {
                    let mut results = results;
                    results.resize_with(jobs.len(), || PrinterJobResult {
                        ok: false,
                        method: String::new(),
                        error: "printer helper returned an incomplete result".into(),
                    });
                    results
                }
                Err(err) => failed(format!("invalid printer helper output: {err}")),
            };
        }
        if Instant::now() > deadline {
            let _ = std::fs::remove_file(&job_path);
            return failed("printer helper did not finish in time".into());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Install a client queue. Uses the native IPP stack (administrator rights are
/// requested once) so the Windows print dialog shows the host printer's duplex,
/// colour and finishing options; falls back to a TCP queue only when IPP is
/// unavailable.
pub fn install_remote_printer(
    local_name: &str,
    display_name: &str,
    driver: &str,
    id: u32,
    tcp_port: u16,
    prefs: &PrinterPreferences,
    capabilities: &PrinterCapabilities,
) -> Result<()> {
    let job = PrinterJob {
        local_name: local_name.to_string(),
        display_name: display_name.to_string(),
        driver: driver.to_string(),
        id,
        tcp_port,
        preferences: prefs.clone(),
        upgrade: false,
        capabilities: Some(super::windows_caps::QueueCaps::from_capabilities(
            capabilities,
        )),
    };
    let result = apply_jobs(std::slice::from_ref(&job))
        .pop()
        .unwrap_or_else(|| PrinterJobResult {
            ok: false,
            method: String::new(),
            error: "printer helper returned no result".into(),
        });
    if result.ok {
        debug!(
            "installed Windows queue {local_name} ({display_name}) via {}",
            result.method
        );
        return Ok(());
    }
    // Without administrator approval the IPP queue cannot be created. Keep the
    // printer usable with the basic TCP path instead of failing the whole add.
    if !is_elevated() {
        debug!(
            "IPP install for {local_name} failed ({}); using a TCP queue",
            result.error
        );
        if let Ok(method) = run_job(&job) {
            debug!("installed Windows queue {local_name} via {method}");
            return Ok(());
        }
    }
    Err(AppError::Platform(if result.error.is_empty() {
        "printer install failed".into()
    } else {
        result.error
    }))
}

/// Whether the queue still uses an older TCP port instead of the native IPP
/// stack, or has not had the host's current features injected yet. IPP queues
/// are exposed through a WSD port by the IPP class driver.
pub fn queue_needs_upgrade(local_name: &str, capabilities: &PrinterCapabilities) -> bool {
    let script = format!(
        "$p = Get-Printer -Name '{}' -ErrorAction SilentlyContinue; if ($p -and $p.PortName -notlike 'WSD-*') {{ 'yes' }} else {{ 'no' }}",
        ps_quote(local_name)
    );
    let is_tcp = powershell(&script)
        .map(|output| output.contains("yes"))
        .unwrap_or(false);
    if is_tcp {
        return true;
    }
    if !super::windows_caps::can_inject(local_name) {
        // A queue without generated class-driver files (a different driver, or
        // files already cleaned up by Windows) cannot carry injected features.
        return false;
    }
    let expected = super::windows_caps::QueueCaps::from_capabilities(capabilities).signature();
    super::windows_caps::installed_signature(local_name).as_deref() != Some(expected.as_str())
}

/// Move existing queues to the native IPP stack and refresh their injected
/// capabilities with a single elevation prompt. A failed upgrade leaves the
/// previous queue in place.
pub fn upgrade_ipp_queues(
    printers: &[(
        String,
        u32,
        PrinterPreferences,
        PrinterCapabilities,
    )],
) -> Result<()> {
    if printers.is_empty() || !crate::ipp::available() {
        return Ok(());
    }
    let jobs: Vec<PrinterJob> = printers
        .iter()
        .map(|(local_name, id, prefs, capabilities)| PrinterJob {
            local_name: local_name.clone(),
            display_name: local_name.clone(),
            driver: String::new(),
            id: *id,
            tcp_port: 19_100u16.saturating_add(*id as u16 % 800),
            preferences: prefs.clone(),
            upgrade: true,
            capabilities: Some(super::windows_caps::QueueCaps::from_capabilities(
                capabilities,
            )),
        })
        .collect();
    let results = apply_jobs(&jobs);
    for (job, result) in jobs.iter().zip(results) {
        if result.ok {
            debug!("queue {} upgraded via {}", job.local_name, result.method);
            continue;
        }
        return Err(AppError::Platform(if result.error.is_empty() {
            format!("could not repair queue {}", job.local_name)
        } else {
            result.error
        }));
    }
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

pub fn remove_remote_printer(local_name: &str) -> Result<()> {
    let script = format!(
        "$ErrorActionPreference='SilentlyContinue'; Remove-Printer -Name '{}'; if ($?) {{ exit 0 }}; rundll32 printui.dll,PrintUIEntry /dl /n \"{}\" /q",
        ps_quote(local_name),
        ps_quote(local_name)
    );
    powershell(&script)?;
    Ok(())
}

pub fn launch_update(path: &Path) -> Result<()> {
    std::process::Command::new(path)
        .args(["/SILENT", "/CLOSEAPPLICATIONS", "/RESTARTAPPLICATIONS"])
        .spawn()
        .map_err(|e| AppError::Platform(format!("cannot start update: {e}")))?;
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
