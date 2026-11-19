use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use lanprint_core::{
    DuplexMode, LocalPrinter, PrinterCapabilities, PrinterOption, PrinterPreferences, ScanRequest,
};
use tracing::{debug, warn};

use super::ScannerInfo;
use crate::error::{AppError, Result};

fn run(cmd: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| AppError::Platform(format!("{cmd}: {e}")))?;
    if !output.status.success() {
        return Err(AppError::Platform(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn list_printers() -> Result<Vec<LocalPrinter>> {
    let names = match run("lpstat", &["-e"]) {
        Ok(out) => out,
        Err(err) => {
            warn!("cups is unavailable: {err}");
            return Ok(Vec::new());
        }
    };
    let default = run("lpstat", &["-d"])
        .ok()
        .and_then(|s| s.split(':').nth(1).map(|v| v.trim().to_string()))
        .unwrap_or_default();
    let scanners = detect_scanners().unwrap_or_default();
    let mut printers = Vec::new();
    for name in names.lines().map(str::trim).filter(|s| !s.is_empty()) {
        let detail = run("lpstat", &["-l", "-p", name]).unwrap_or_default();
        let mut location = String::new();
        let mut make_model = String::new();
        let mut description = String::new();
        for line in detail.lines() {
            let line = line.trim();
            if let Some(v) = line.strip_prefix("Location:") {
                location = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("Make and Model:") {
                make_model = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("Description:") {
                description = v.trim().to_string();
            }
        }
        let status = if detail.contains("is printing") || detail.contains("now printing") {
            "printing"
        } else if detail.contains("disabled") {
            "offline"
        } else if detail.contains("is idle") || detail.contains("idle") {
            "idle"
        } else {
            "unknown"
        };
        let ppd = ppd_path(name);
        let (mut caps, prefs) = caps_from_ppd(ppd.as_deref(), &make_model);
        caps.supports_scan = false;
        if let Some(scanner) = match_scanner(name, &scanners) {
            caps.supports_scan = true;
            caps.scanner_id = scanner.id.clone();
        }
        printers.push(LocalPrinter {
            name: name.to_string(),
            driver: if make_model.is_empty() {
                description
            } else {
                make_model
            },
            port: run("lpstat", &["-v", name])
                .ok()
                .and_then(|s| s.split(':').nth(1).map(|v| v.trim().to_string()))
                .unwrap_or_default(),
            is_default: default == name,
            status: status.into(),
            capabilities: caps,
            preferences: prefs,
            available: true,
        });
    }
    printers.sort_by(|a, b| b.is_default.cmp(&a.is_default).then(a.name.cmp(&b.name)));
    Ok(printers)
}

fn ppd_path(name: &str) -> Option<PathBuf> {
    let path = PathBuf::from("/etc/cups/ppd").join(format!("{name}.ppd"));
    path.exists().then_some(path)
}

fn caps_from_ppd(
    path: Option<&Path>,
    make_model: &str,
) -> (PrinterCapabilities, PrinterPreferences) {
    let mut caps = PrinterCapabilities {
        color: false,
        duplex: false,
        max_paper: "A4".into(),
        make_model: make_model.to_string(),
        resolutions: vec![300, 600],
        formats: vec!["pdf".into(), "ps".into(), "text".into(), "raw".into()],
        ..Default::default()
    };
    let mut prefs = PrinterPreferences::default();
    let Some(path) = path else {
        return (caps, prefs);
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return (caps, prefs);
    };
    for line in text.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("*ColorDevice:") {
            caps.color = v.trim().eq_ignore_ascii_case("true");
        } else if let Some(v) = line.strip_prefix("*DefaultColorSpace:") {
            prefs.color = !v.trim().contains("Gray");
        } else if let Some(v) = line.strip_prefix("*DefaultDuplex:") {
            prefs.duplex = match v.trim() {
                "DuplexNoTumble" => DuplexMode::LongEdge,
                "DuplexTumble" => DuplexMode::ShortEdge,
                _ => DuplexMode::Simplex,
            };
            caps.duplex = prefs.duplex != DuplexMode::Simplex;
        } else if let Some(v) = line.strip_prefix("*Duplex:") {
            caps.duplex = true;
        } else if let Some(v) = line.strip_prefix("*DefaultPageSize:") {
            prefs.paper = friendly_paper(v.trim());
        } else if let Some(v) = line.strip_prefix("*DefaultResolution:") {
            if let Ok(dpi) = v.trim().split('x').next().unwrap_or("").parse() {
                prefs.resolution = dpi;
            }
        } else if let Some(v) = line.strip_prefix("*OpenUI *PageSize:") {
            if v.contains("A3") {
                caps.max_paper = "A3".into();
            }
        }
        if line.starts_with("*PaperDimension") || line.starts_with("*PageSize ") {
            let paper = friendly_paper(line);
            if !paper.is_empty() && !caps.papers.iter().any(|p| p.eq_ignore_ascii_case(&paper)) {
                caps.papers.push(paper);
            }
        }
        if line.contains("A3") && caps.max_paper == "A4" {
            caps.max_paper = "A3".into();
        }
    }
    if caps.duplex {
        caps.duplex_modes = vec!["long_edge".into(), "short_edge".into()];
    }
    caps.papers.sort();
    (caps, prefs)
}

fn friendly_paper(ppd: &str) -> String {
    let upper = ppd.to_ascii_uppercase();
    for size in ["A3", "A4", "A5", "LETTER", "LEGAL"] {
        if upper.contains(size) {
            return size.to_string();
        }
    }
    ppd.to_string()
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

pub fn detect_scanners() -> Result<Vec<ScannerInfo>> {
    let output = match Command::new("scanimage").arg("-L").output() {
        Ok(output) => output,
        Err(err) => {
            debug!("scanimage unavailable: {err}");
            return Ok(Vec::new());
        }
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut scanners = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("device `") else {
            continue;
        };
        let Some((id, rest)) = rest.split_once('\'') else {
            continue;
        };
        let name = rest
            .split("is a")
            .nth(1)
            .map(|s| s.trim().trim_matches('\'').trim().to_string())
            .unwrap_or_else(|| id.to_string());
        scanners.push(ScannerInfo {
            id: id.to_string(),
            adf: name.to_lowercase().contains("adf")
                || name.to_lowercase().contains("document feeder"),
            name,
        });
    }
    Ok(scanners)
}

pub fn scan(request: &ScanRequest, scanner_id: &str) -> Result<Vec<u8>> {
    let mode = if request.color { "Color" } else { "Gray" };
    let mut cmd = Command::new("scanimage");
    cmd.args([
        "-d",
        scanner_id,
        "--format=png",
        "--resolution",
        &request.resolution.to_string(),
        "--mode",
        mode,
    ]);
    if request.source == "adf" {
        cmd.args(["--source", "ADF"]);
    }
    let output = cmd
        .output()
        .map_err(|e| AppError::Scan(format!("scanimage: {e}")))?;
    if !output.status.success() {
        return Err(AppError::Scan(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    if output.stdout.is_empty() {
        return Err(AppError::Scan("scanner produced no data".into()));
    }
    Ok(output.stdout)
}

pub fn print_document(
    printer: &str,
    data: &[u8],
    content_type: &str,
    filename: &str,
    prefs: &PrinterPreferences,
) -> Result<()> {
    let ext = filename
        .rsplit('.')
        .next()
        .filter(|e| e.len() <= 8 && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or("dat");
    let mut tmp = tempfile::Builder::new()
        .prefix("lanprint-print-")
        .suffix(&format!(".{ext}"))
        .tempfile()
        .map_err(|e| AppError::Print(e.to_string()))?;
    tmp.write_all(data)
        .map_err(|e| AppError::Print(e.to_string()))?;
    let mut cmd = Command::new("lp");
    cmd.arg("-d").arg(printer);
    cmd.arg("-t").arg(filename);
    if content_type.contains("octet-stream") && filename.ends_with(".raw") {
        cmd.args(["-o", "raw"]);
    }
    if !prefs.paper.is_empty() {
        cmd.args(["-o", &format!("media={}", prefs.paper)]);
    }
    if prefs.duplex != DuplexMode::Simplex {
        let side = if prefs.duplex == DuplexMode::LongEdge {
            "two-sided-long-edge"
        } else {
            "two-sided-short-edge"
        };
        cmd.args(["-o", &format!("sides={side}")]);
    }
    if !prefs.color {
        cmd.args(["-o", "ColorModel=Gray"]);
    }
    if prefs.resolution > 0 {
        cmd.args(["-o", &format!("Resolution={}dpi", prefs.resolution)]);
    }
    if !prefs.media_source.is_empty() {
        cmd.args(["-o", &format!("media-source={}", prefs.media_source)]);
    }
    if prefs.copies() > 1 {
        cmd.args(["-o", &format!("copies={}", prefs.copies())]);
    }
    for (key, value) in &prefs.options {
        cmd.args(["-o", &format!("{key}={value}")]);
    }
    cmd.arg(tmp.path());
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    let output = cmd
        .output()
        .map_err(|e| AppError::Print(format!("lp: {e}")))?;
    if !output.status.success() {
        return Err(AppError::Print(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    debug!("queued job for {printer} ({filename})");
    Ok(())
}

fn backend_dir() -> PathBuf {
    let mac = PathBuf::from("/usr/libexec/cups/backend");
    if mac.exists() {
        mac
    } else {
        PathBuf::from("/usr/lib/cups/backend")
    }
}

fn run_privileged(program: &str, args: &[String]) -> Result<std::process::Output> {
    #[allow(unused_mut)]
    let mut last_output = match Command::new(program).args(args).output() {
        Ok(output) if output.status.success() => return Ok(output),
        Ok(output) => Some(output),
        Err(_) => None,
    };
    #[cfg(target_os = "macos")]
    {
        let quoted = args
            .iter()
            .map(|a| format!("'{}'", a.replace('\'', "'\\''")))
            .collect::<Vec<_>>()
            .join(" ");
        let shell = format!("{} {}", program, quoted);
        let apple = format!(
            "do shell script \"{}\" with administrator privileges",
            shell.replace('\\', "\\\\").replace('"', "\\\"")
        );
        let output = Command::new("osascript")
            .args(["-e", &apple])
            .output()
            .map_err(|e| AppError::Platform(format!("osascript: {e}")))?;
        return Ok(output);
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for candidate in ["pkexec", "sudo"] {
            let mut cmd = Command::new(candidate);
            if candidate == "sudo" {
                cmd.arg("-n");
            }
            if let Ok(output) = cmd.arg(program).args(args).output() {
                if output.status.success() {
                    return Ok(output);
                }
                last_output = Some(output);
            }
        }
    }
    last_output.ok_or_else(|| {
        AppError::Platform(format!(
            "could not run {program} with administrator privileges"
        ))
    })
}

pub fn install_remote_printer(
    local_name: &str,
    display_name: &str,
    driver: &str,
    _tcp_port: u16,
    uri: &str,
    prefs: &PrinterPreferences,
) -> Result<()> {
    let exe = std::env::current_exe().map_err(|e| AppError::Platform(e.to_string()))?;
    let backend = backend_dir().join("lanprint");
    if !backend.exists() {
        let cmd = format!(
            "ln -sf '{}' '{}' && chmod 0700 '{}'",
            exe.display(),
            backend.display(),
            backend.display()
        );
        let output = run_privileged("sh", &["-c".into(), cmd.clone()])?;
        if !output.status.success() {
            return Err(AppError::Platform(format!(
                "CUPS backend is not installed. Run:\n  sudo sh -c \"{cmd}\""
            )));
        }
    }
    let mut args: Vec<String> = vec![
        "-p".into(),
        local_name.into(),
        "-E".into(),
        "-v".into(),
        uri.into(),
        "-m".into(),
        "raw".into(),
        "-D".into(),
        display_name.into(),
        "-L".into(),
        "lanPrint".into(),
    ];
    if !driver.is_empty() {
        args.push("-D".into());
        args.push(format!("{display_name} ({driver})"));
    }
    let output = run_privileged("lpadmin", &args)?;
    if !output.status.success() {
        return Err(AppError::Platform(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    // Apply the preferences that came from the host so that the local queue
    // uses the same defaults. Errors here are non fatal on drivers that do not
    // understand a particular option.
    let mut args: Vec<String> = vec!["-p".into(), local_name.into()];
    args.push("-o".into());
    args.push("printer-is-shared=false".into());
    if !prefs.paper.is_empty() {
        args.push("-o".into());
        args.push(format!("media={}", prefs.paper));
    }
    if prefs.duplex != DuplexMode::Simplex {
        args.push("-o".into());
        args.push(format!(
            "sides={}",
            if prefs.duplex == DuplexMode::LongEdge {
                "two-sided-long-edge"
            } else {
                "two-sided-short-edge"
            }
        ));
    }
    let _ = run_privileged("lpadmin", &args);
    Ok(())
}

pub fn apply_printer_preferences(local_name: &str, prefs: &PrinterPreferences) -> Result<()> {
    let sides = match prefs.duplex {
        DuplexMode::LongEdge => "two-sided-long-edge",
        DuplexMode::ShortEdge => "two-sided-short-edge",
        DuplexMode::Simplex => "one-sided",
    };
    let mut args: Vec<String> = vec![
        "-p".into(),
        local_name.into(),
        "-o".into(),
        format!("sides={sides}"),
    ];
    if !prefs.paper.is_empty() {
        args.push("-o".into());
        args.push(format!("media={}", prefs.paper));
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run("lpoptions", &refs).map(|_| ())
}

pub fn remove_remote_printer(local_name: &str) -> Result<()> {
    let output = run_privileged("lpadmin", &["-x".into(), local_name.into()])?;
    if !output.status.success() {
        return Err(AppError::Platform(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(())
}

pub fn launch_update(path: &Path) -> Result<()> {
    open::that(path).map_err(|e| AppError::Platform(format!("cannot open update: {e}")))
}

pub fn open_path(path: &Path) -> Result<()> {
    let cmd = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    Command::new(cmd)
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| AppError::Platform(format!("cannot open {}: {e}", path.display())))
}

pub fn log_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lanPrint")
        .join("logs")
}
