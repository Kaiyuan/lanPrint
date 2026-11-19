//! Minimal autostart integration without extra dependencies.
//!
//! - Windows: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
//! - macOS: `~/Library/LaunchAgents/com.kaiyuan.lanprint.plist`
//! - Linux/UOS: `~/.config/autostart/lanprint.desktop`

use std::process::Command;

#[cfg(unix)]
use std::path::PathBuf;

use tracing::warn;

use crate::error::{AppError, Result};

const APP_ID: &str = "lanPrint";

pub fn set_enabled(enabled: bool) -> Result<()> {
    let exe = std::env::current_exe().map_err(|e| AppError::Platform(e.to_string()))?;
    #[cfg(windows)]
    {
        if enabled {
            let value = format!("\"{}\" --tray", exe.display());
            let status = hide_window(Command::new("reg"))
                .args([
                    "add",
                    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                    "/v",
                    APP_ID,
                    "/t",
                    "REG_SZ",
                    "/d",
                    &value,
                    "/f",
                ])
                .status()
                .map_err(|e| AppError::Platform(format!("reg add: {e}")))?;
            if !status.success() {
                return Err(AppError::Platform("could not enable autostart".into()));
            }
        } else {
            let _ = hide_window(Command::new("reg"))
                .args([
                    "delete",
                    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                    "/v",
                    APP_ID,
                    "/f",
                ])
                .status();
        }
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        let path = plist_path();
        if enabled {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let plist = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.kaiyuan.lanprint</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>--tray</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><false/>
  <key>ProcessType</key><string>Interactive</string>
</dict>
</plist>
"#,
                exe = exe.display()
            );
            std::fs::write(&path, plist)?;
        } else {
            let _ = std::fs::remove_file(&path);
        }
        Ok(())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let path = desktop_path();
        if enabled {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let desktop = format!(
                "[Desktop Entry]\nType=Application\nName=lanPrint\nComment=Cross-platform printer sharing\nExec=\"{}\" --tray\nX-GNOME-Autostart-enabled=true\nTerminal=false\n",
                exe.display()
            );
            std::fs::write(&path, desktop)?;
            let _ = Command::new("chmod")
                .args(["+x", &path.display().to_string()])
                .status();
        } else {
            let _ = std::fs::remove_file(&path);
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn plist_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library/LaunchAgents/com.kaiyuan.lanprint.plist")
}

#[cfg(all(unix, not(target_os = "macos")))]
fn desktop_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("autostart/lanprint.desktop")
}

#[cfg(windows)]
fn hide_window(mut cmd: Command) -> Command {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000);
    cmd
}

pub fn apply_on_startup(enabled: bool) {
    if let Err(err) = set_enabled(enabled) {
        warn!("could not update autostart: {err}");
    }
}
