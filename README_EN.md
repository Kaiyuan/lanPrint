# lanPrint

**lanPrint** is a lightweight, powerful cross-platform printing gateway designed to convert any local printer into a smart printer accessible via network. It completely bypasses traditional, complex protocols like IPP/WSD, implementing a custom high-efficiency HTTP-based transmission and virtual printer hijacking mechanism. It supports Windows, macOS, and Linux.

## 🌟 Key Features

- **🚀 Zero-Configuration Start**: One-click to run, automatically discovers other lanPrint instances in the local network.
- **💻 Full Platform Support**:
  - **Windows (7/8/10/11)**: Based on dynamic TCP port hijacking, automatic driver matching, and supports native PDF/XPS fallback printing.
  - **macOS (10.15+)**: Integrated CUPS Backend, supports system native print queues.
  - **Linux (Ubuntu, CentOS, etc.)**: Supports CUPS architecture, command-line friendly.
- **🔒 Secure & Reliable**: Supports password-protected printer sharing with SHA256 encryption.
- **⚡️ Zero-Dependency Design**: Built-in logic for all features. On Windows, it can perform headless printing using the built-in Edge browser even if no PDF reader is installed.
- **📊 Job Monitoring**: Real-time monitoring of print job status, history, and printer capabilities (Color, Duplex, A3, etc.).

## 🛠 How It Works

lanPrint uses a custom "Local Hijack -> HTTP Forward -> Remote Execution" architecture:
1. **Local Hijack**: Installs a virtual printer on the client. Windows intercepts data by listening on local TCP ports; Unix intercepts data via a custom CUPS Backend.
2. **HTTP Forward**: Sends the intercepted raw print stream (RAW, PDF, XPS, or PS) to the server via an encrypted HTTP POST request.
3. **Remote Execution**: The server receives the data and automatically chooses the best local distribution method (WinAPI `WritePrinter` or `lp` command) to send it to the physical printer.

## 📥 Installation & Usage

### Quick Start
Download the binary for your platform from the [Releases](https://github.com/kaiyuan/lanPrint/releases) page.

**Windows:**
1. Right-click `lanPrint.exe` and run as administrator.
2. Select "Open Settings" from the system tray menu.
3. In the "Printers" tab, select the printer to share and click "Share".

**macOS/Linux:**
1. Grant execution permission: `chmod +x lanPrint`
2. Recommended to run in service mode: `./lanPrint -service install && ./lanPrint -service start`

## 📖 Developer Guide

### Local Build

#### Requirements
- Go 1.22+
- **Linux Required**: `libappindicator3-dev` and `libgtk-3-dev` (for tray icon support)

#### Build Commands

**Windows:**
```powershell
# Build as a windowless GUI application
go build -ldflags "-s -w -X main.version=v1.0.0 -H=windowsgui" -o lanPrint.exe ./cmd/lanPrint
```

**Linux (Ubuntu/UOS/Deepin):**
```bash
# Install dependencies
sudo apt-get install -y libappindicator3-dev libgtk-3-dev

# Enable CGO for tray support
CGO_ENABLED=1 go build -ldflags "-s -w -X main.version=v1.0.0" -o lanPrint ./cmd/lanPrint
```

**macOS:**
```bash
go build -ldflags "-s -w -X main.version=v1.0.0" -o lanPrint ./cmd/lanPrint
```

### Cross-Platform Release (GoReleaser)
The project uses GoReleaser to handle multi-platform distribution:
```bash
# Local snapshot build
goreleaser build --snapshot --clean
```

## 📄 License
[MIT License](LICENSE)
