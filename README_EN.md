# lanPrint

![lanPrint](crates/desktop/ui/assets/lanprint-icon.png)

**lanPrint** is a cross-platform printer-sharing tool built with **Rust + Slint**. It does not rely on Windows file sharing, IPP, WSD or Samba. A small, self-designed HTTP protocol shares any installed printer securely over a LAN or the internet.

Every installation can act as a **host** and as a **client**:

- Host: pick local printers, set a host access code and optional per-printer codes.
- Client: connect by device name or IP, enter the code, list shared printers and add them with one click.
- Cloud: deploy the small bridge server, enable cloud sharing on the host, then connect from any network with an 8-character share code.

Supported systems: **Windows 7+, macOS 10.15+, Ubuntu / Debian / UOS / Deepin**. The UI is available in **Simplified Chinese, Traditional Chinese, English, French, German, Italian, Spanish, Japanese and Korean**.

> Documentation: [https://kaiyuan.github.io/lanPrint/](https://kaiyuan.github.io/lanPrint/)
>
> Releases: [GitHub Releases](https://github.com/Kaiyuan/lanPrint/releases)

## Features

- **Host and client in one app** — no separate service install.
- **Real virtual printers** — added queues appear in the system print dialog.
  - Windows: local TCP spool port + Win32 spooler API.
  - Linux/macOS: CUPS backend + `lpadmin` queue.
- **Preferences synchronisation** — paper size, colour, duplex and resolution shared from the host.
- **Scanning** — WIA on Windows, SANE/`scanimage` on Linux and macOS.
- **Per-printer access codes** — host code plus optional codes. Passwords are Argon2id hashes; clients keep signed tokens, never plaintext.
- **LAN discovery** — mDNS `_lanprint._tcp.local.` plus manual host/IP entry.
- **Online/offline status** for hosts and printers.
- **System tray** — autostart, Open/Quit menu, click to show the window.
- **Cloud bridge** — independent Rust relay server with Docker, one-line install and HTTPS reverse-proxy support. No inbound port on the host.
- **Nine UI languages**, switchable without a restart.
- **Low resource usage** — Slint software renderer, no WebView/Electron.

### Ports

| Port | Protocol | Purpose |
| --- | --- | --- |
| 52333 | TCP | Host API / client connections / cloud relay origin |
| 52335 | TCP | Local single-instance check (loopback only) |
| 5353 | UDP | mDNS discovery (optional) |
| 19100+ | TCP | Windows local virtual spool port (loopback only) |
| 52334 | TCP | Bridge server default listen port |

## Repository layout

```text
Cargo.toml                 Rust workspace
crates/core/               Shared models, config, Argon2 auth, tokens, mDNS, protocol
crates/desktop/            Slint desktop app (host + client + CUPS backend)
  ui/app.slint             Main window and tray
crates/bridge/             Cloud relay server
locales/                   Nine UI translation files
docs/                      Multilingual GitHub Pages site
deploy/                    Docker Compose / Caddy examples
packaging/                 Linux desktop entry, systemd unit, installer
scripts/                   One-line installers
.github/workflows/         Release (multi-platform + Docker) and Pages
```

## Install

### Windows 7 / 8 / 10 / 11

Download `lanPrint_*_Windows_x86_64.zip` from [Releases](https://github.com/Kaiyuan/lanPrint/releases), extract and run `lanPrint.exe`. It starts in the notification area and enables autostart by default.

```powershell
iwr -useb https://raw.githubusercontent.com/Kaiyuan/lanPrint/main/scripts/install.ps1 | iex
```

A dedicated Windows 7 build target is produced by CI. Because upstream Slint/winit Windows 7 support varies by release, that build is best-effort; standard builds require current Windows updates and the VC++ runtime.

### macOS 10.15+

Download the archive for your CPU (Intel or Apple Silicon), move `LanPrint.app` to Applications. The release is ad-hoc signed, so use right-click → Open on first launch. To add printers as a client, install the CUPS backend:

```bash
sudo ln -sf /Applications/LanPrint.app/Contents/MacOS/lanPrint /usr/libexec/cups/backend/lanprint
```

### Ubuntu / Debian / UOS / Deepin

```bash
curl -fsSL https://raw.githubusercontent.com/Kaiyuan/lanPrint/main/scripts/install.sh | bash
```

Or extract an archive and run:

```bash
sudo ./install.sh
```

The installer adds the binary, desktop entry, icon and CUPS backend symlink. On UOS 20 / Deepin 20 the tray uses StatusNotifierItem over D-Bus, so no GTK AppIndicator dependency is required.

> UOS / Deepin ship an older glibc. Releases also include best-effort `Linux_*_musl` static builds; use one of those if the standard binary does not start, or build from source on the target system.

## Usage

### Host

1. **Settings → General**: set a host access code.
2. **Settings → Shared printers**: toggle the printers to share.
3. Optionally set a printer-specific access code.
4. The status bar shows “Sharing on port 52333” and the host is advertised over mDNS.

### Client

1. Click **Connect device** to scan the LAN, or **Enter manually** to type a host name/IP and code.
2. **Test connection**, then **Save**.
3. Select the host, browse shared printers and click **Add**.
4. The queue appears in the system print dialog. Use **Test print** to verify.
5. All-in-one printers show a **Scan** button with resolution, colour and source options.

### Cloud

1. Deploy the bridge server.
2. Host: **Settings → Cloud sharing**, enable, enter the bridge URL and server code. Save to receive an 8-character share code.
3. Client: **Connect device → Manual**, enable cloud connection, enter the bridge URL, share code and host code.
4. Add printers exactly as on a LAN. The host still verifies every printer code. Traffic is relayed by default; in a trusted LAN you can set `LANPRINT_ALLOW_DIRECT=1` to try the address hint returned by the bridge before falling back to the relay.

## Build from source

### One-click build

```bash
# Linux / macOS
bash scripts/build.sh
```

```powershell
# Windows (PowerShell)
powershell -ExecutionPolicy Bypass -File .\scripts\build.ps1
```

The script builds the desktop app and the bridge server, and writes binaries,
archives (and the Windows installer when Inno Setup is available) to `dist/`.
The first build downloads dependencies; on Linux install the packages listed
below first.

### Manual build

Requirements:

- Rust 1.95+ (rustup stable recommended)
- Linux: `libxkbcommon-dev`, `libwayland-dev`, `libx11-dev`, `libfontconfig1-dev` (winit/runtime fonts)
- Windows: MSVC Build Tools
- macOS: Xcode Command Line Tools

```bash
cargo build --release -p lanprint-desktop
cargo build --release -p lanprint-bridge
cargo run -p lanprint-desktop
cargo run -p lanprint-bridge -- --help
```

Headless helpers (also used by installers and services):

```bash
lanPrint --set-password 'your-code'
lanPrint --share 'Printer Name'
lanPrint --unshare 'Printer Name'
lanPrint --device-name 'Office-PC'
lanPrint --cloud https://bridge.example.com 'server-code'
lanPrint --cloud-disable
lanPrint --headless          # run services without opening a window
```

### CUPS backend

Client queues use a `lanprint://` URI. The installer creates:

```bash
sudo ln -sf /usr/local/bin/lanPrint /usr/lib/cups/backend/lanprint
```

CUPS invokes the binary with `DEVICE_URI`; it reads the spool file and forwards it to the host.

### Windows virtual printers

When adding a printer, lanPrint:

1. listens on `127.0.0.1:19100+`;
2. creates a TCP printer port via `Add-PrinterPort` / `prnport.vbs`;
3. creates the queue via `Add-Printer` or `rundll32 printui.dll`;
4. forwards spool data to the host.

On Windows 7 it falls back to the inbox `Generic / Text Only` driver. If the same driver as the host is installed, it is preferred.

## Deploy the bridge

### Docker

The bridge listens on port **52334** by default.

```bash
docker run -d --name lanprint-bridge \
  --restart unless-stopped \
  -p 52334:52334 \
  -e LANPRINT_BRIDGE_AUTH_CODE='replace-with-a-long-random-string' \
  -e LANPRINT_BRIDGE_PUBLIC_URL='https://bridge.example.com' \
  -v lanprint-data:/data \
  kaiyuan/lanprint-bridge:latest
```

Or use Compose:

```bash
cd deploy
export LANPRINT_BRIDGE_AUTH_CODE='change-me'
docker compose up -d
```

For production use `deploy/docker-compose.caddy.yml` or Nginx/Traefik with HTTPS and WebSocket upgrades.

### Linux / macOS one-line install

```bash
curl -fsSL https://raw.githubusercontent.com/Kaiyuan/lanPrint/main/scripts/install-bridge.sh | sudo bash
```

The script generates a server code, installs a systemd unit (Linux) or launchd daemon (macOS), and starts the service. Configuration lives in `/etc/lanprint-bridge/bridge.env` on Linux.

On Windows, download `lanprint-bridge_*_Windows_x86_64.zip` and run it via a scheduled task or NSSM. For production, a Linux host behind Docker/Caddy is recommended.

### Configuration

| Variable | Default | Description |
| --- | --- | --- |
| `LANPRINT_BRIDGE_ADDR` | `0.0.0.0:52334` | Listen address |
| `LANPRINT_BRIDGE_AUTH_CODE` | required | Server code used by hosts |
| `LANPRINT_BRIDGE_DATA` | `data/hosts.json` | Persistent share-code database |
| `LANPRINT_BRIDGE_PUBLIC_URL` | empty | Public HTTPS URL returned to clients |
| `RUST_LOG` | `info` | Log level |

## Security

- Host codes are Argon2id hashes. Changing the host code rotates the signing key and invalidates old tokens.
- Clients exchange codes for short-lived signed tokens (host 7 days, printer 30 days); plaintext passwords are not persisted.
- Printer-specific codes can add a second factor per queue.
- The bridge stores only device identity, share code, timestamps and host tokens. Print jobs are relayed, not written to disk.
- Always use HTTPS/WSS in production. The bridge speaks plain HTTP and is designed to sit behind Caddy, Nginx or Traefik.
- The LAN host API uses HTTP with signed tokens. Only share on trusted local networks, or use the cloud bridge's HTTPS channel across the internet.

## Tests

```bash
cargo test --workspace
cargo clippy --workspace --all-targets
```

Windows 7 build (best effort in CI):

```powershell
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly
cargo +nightly build -Z build-std=std,panic_abort --release `
  --target x86_64-win7-windows-msvc -p lanprint-desktop
```

## License

[MIT License](LICENSE)

Slint is tri-licensed under GPL-3.0, Royalty-free 2.0 and Software 3.0. This project uses Slint under the Royalty-free 2.0 terms; redistributions must comply with the Slint license. See [slint.dev/license](https://slint.dev/license).

## Release pipeline

Pushing a `v*` tag runs `.github/workflows/release.yml`:

- builds Windows x86_64 / i686, Windows 7, macOS Intel / Apple Silicon, Linux x86_64 / aarch64 and best-effort static musl binaries;
- creates Windows zip + Inno Setup installer, macOS `.app`, Linux tar.gz;
- builds the bridge server;
- builds and pushes the multi-arch `kaiyuan/lanprint-bridge` Docker image;
- publishes a GitHub release with every artifact.

Repository secrets:

| Secret | Purpose |
| --- | --- |
| `DOCKERHUB_USERNAME` | Docker Hub account |
| `DOCKERHUB_TOKEN` | Docker Hub access token |

GitHub Pages is deployed from `docs/` by `.github/workflows/pages.yml`.
