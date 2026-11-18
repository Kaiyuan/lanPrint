# lanPrint

![lanPrint](crates/desktop/ui/assets/lanprint-icon.png)

**lanPrint** 是一个使用 **Rust + Slint** 开发的跨平台共享打印机工具。它不依赖 Windows 文件共享、IPP、WSD 或 Samba，而是通过自有的轻量 HTTP 协议，把任意已安装的打印机安全地共享给局域网或互联网上的其他设备。

同一个程序既可以作为**服务端（主机）**，也可以作为**客户端**：

- 主机：选择本机打印机共享，设置连接授权码和每台打印机的独立授权码。
- 客户端：通过设备名称 / IP 地址连接主机，输入授权码后列出共享打印机，一键添加为本地虚拟打印机。
- 云端：部署独立的桥接服务端后，主机可获得 8 位共享码，客户端可在任意网络通过共享码连接。

支持 **Windows 7+、macOS 10.15+、Ubuntu / Debian / UOS / Deepin**，界面提供 **简体中文、繁體中文、English、Français、Deutsch、Italiano、Español、日本語、한국어** 九种语言。

> 文档站点：[https://kaiyuan.github.io/lanPrint/](https://kaiyuan.github.io/lanPrint/)
>
> 发布包：[Releases](https://github.com/Kaiyuan/lanPrint/releases)

## ✨ 功能

- **主机 / 客户端二合一**：同一个托盘程序完成共享与连接，无需额外安装服务。
- **真实虚拟打印机**：添加后出现在系统打印对话框中。
  - Windows：本地 TCP 打印端口 + 打印后台 API。
  - Linux / macOS：CUPS Backend + `lpadmin` 队列。
- **打印机首选项同步**：主机共享纸张、彩色、双面、分辨率等默认设置，客户端应用到本地队列。
- **扫描共享**：多功能打印机可将扫描功能共享给客户端。
  - Windows：WIA
  - Linux / macOS：SANE / `scanimage`
- **每台打印机独立授权码**：主机授权码 + 可选打印机授权码。密码使用 **Argon2id** 哈希，客户端保存签名令牌而不是明文。
- **局域网自动发现**：mDNS `_lanprint._tcp.local.`，也支持手动输入主机名 / IP。
- **在线 / 脱机状态**：客户端定期检测主机与打印机状态。
- **系统托盘**：开机自启动，右键菜单提供“打开 LanPrint / 退出”，双击打开主界面。
- **macOS 风格界面**：统一的工具栏、侧边栏、卡片与表单；自动跟随系统浅色/深色外观，也可手动固定浅色/深色。
- **可自定义强调色**：蓝、紫、粉、红、橙、绿、石墨 7 种颜色，即时生效并保存。
- **云端桥接**：独立 Rust 桥接服务端，支持 Docker、一键脚本、HTTPS 反向代理；主机位于 NAT 后也无需开放端口。
- **九种界面语言**：运行时即时切换，无需重启。
- **低资源占用**：Slint 软件渲染器，无 WebView、无 Electron；空闲时 CPU / 内存占用极低。

### 端口

| 端口 | 协议 | 用途 |
| --- | --- | --- |
| 52333 | TCP | 主机 API / 客户端连接 / 云中继回源 |
| 52335 | TCP | 本机单实例检测（仅回环） |
| 5353 | UDP | mDNS 自动发现（可选） |
| 19100+ | TCP | Windows 客户端本地虚拟打印端口（仅回环） |
| 52334 | TCP | 桥接服务端默认监听端口 |

## 🧱 项目结构

```text
Cargo.toml                 Rust workspace
crates/
  core/                    共享模型、配置、Argon2 授权、令牌、mDNS、协议
  desktop/                 Slint 桌面程序（主机 + 客户端 + CUPS backend）
    ui/app.slint           主界面与托盘
    locales/               9 种语言翻译（位于仓库根目录 locales/）
  bridge/                  云端桥接服务端
locales/                   界面翻译 JSON（9 种语言）
docs/                      GitHub Pages 多语言文档
deploy/                    Docker Compose / Caddy 示例
packaging/                 Linux 桌面项、systemd、安装脚本
scripts/                   一键安装脚本（客户端 / 服务端）
.github/workflows/         Release（多平台 + Docker）与 Pages
```

## 📥 安装

### Windows 7 / 8 / 10 / 11

从 [Releases](https://github.com/Kaiyuan/lanPrint/releases) 下载 `lanPrint_*_Windows_x86_64.zip`，解压后运行 `lanPrint.exe`。程序会驻留通知区域，并默认加入开机启动。

PowerShell 一键安装（当前用户）：

```powershell
iwr -useb https://raw.githubusercontent.com/Kaiyuan/lanPrint/main/scripts/install.ps1 | iex
```

> Windows 7 构建使用 `x86_64-win7-windows-msvc` 目标，在 CI 中单独生成。由于 Slint/winit 上游对 Windows 7 的支持存在版本差异，该构建为 best-effort；标准构建请确保系统已安装最新补丁和 VC++ 运行库。

### macOS 10.15+

下载对应架构（Intel / Apple Silicon）的 `LanPrint.app`，拖入“应用程序”。由于发布包使用 ad-hoc 签名，首次启动请右键 → 打开。若要作为客户端添加打印机，还需安装 CUPS Backend：

```bash
sudo ln -sf /Applications/LanPrint.app/Contents/MacOS/lanPrint /usr/libexec/cups/backend/lanprint
```

### Ubuntu / Debian / UOS / Deepin

```bash
curl -fsSL https://raw.githubusercontent.com/Kaiyuan/lanPrint/main/scripts/install.sh | bash
```

或手动解压后：

```bash
sudo ./install.sh
```

脚本会安装主程序、桌面入口、图标和 CUPS Backend 软链接。UOS 20 / Deepin 20 的托盘通过 StatusNotifierItem 实现，无需 GTK AppIndicator 依赖。

> UOS / Deepin 的系统 glibc 较旧。发布页同时提供 `Linux_*_musl` 静态构建（best-effort）；若标准包无法启动，请优先使用 musl 包，或在目标系统上从源码构建。

## 🚀 使用

> 桌面端配置默认保存在用户 **文档** 文件夹下的 `lanPrint/config.toml`（Windows、macOS、Linux 相同）。旧版本位于 `%APPDATA%\lanPrint` 的配置会在首次启动时自动复制迁移。

### 主机：共享打印机

1. 打开 **设置 → 常规**，设置主机授权码。
2. 打开 **设置 → 共享打印机**，开启需要共享的打印机。
3. 可选：为单台打印机设置独立授权码。
4. 状态栏显示“正在端口 52333 共享”，同时通过 mDNS 广播。
5. 在 **设置 → 常规 → 外观** 中选择跟随系统、浅色或深色，并可选择 7 种强调色。

### 客户端：连接并添加打印机

1. 点击 **连接设备**，扫描局域网；或点击底部 **手动输入**，填写主机名 / IP 和授权码。
2. 点击 **测试连接**，成功后 **保存**。
3. 主机以单列卡片显示。点击主机展开已连接的打印机；右键主机可展开/收起、列出所有打印机或断开所有打印机。卡片右侧的箭头表示展开状态。
4. 打印机会出现在系统打印对话框中；点击 **测试打印** 验证链路。
5. 带扫描功能的打印机显示 **扫描** 按钮，可选择分辨率、彩色/灰度、平板/ADF。

### 云端共享

1. 按下方说明部署桥接服务端。
2. 主机：**设置 → 云共享**，启用并填写桥接地址与服务器授权码，保存后获得 8 位共享码。
3. 客户端：**连接设备 → 手动输入**，勾选“通过云端桥接连接”，填写桥接地址、共享码和主机授权码。
4. 添加打印机的方式与局域网完全一致。桥接服务器只转发流量，打印机授权码始终由主机校验。默认通过中继传输；在可信局域网中可设置 `LANPRINT_ALLOW_DIRECT=1` 尝试直连桥接服务器返回的地址提示。

## 🛠 从源码构建

### 一键编译

```bash
# Linux / macOS
bash scripts/build.sh
```

```powershell
# Windows (PowerShell)
powershell -ExecutionPolicy Bypass -File .\scripts\build.ps1
```

脚本会自动编译桌面端和桥接服务端，并把可执行文件、压缩包（以及 Windows 安装包，如果已安装 Inno Setup）放到 `dist/`。首次编译需要联网下载依赖；Linux 还需先安装下方列出的构建依赖。

### 手动构建

前置要求：

- Rust 1.95+（建议使用 rustup stable）
- Linux 构建可选依赖：`libxkbcommon-dev`、`libwayland-dev`、`libx11-dev`、`libfontconfig1-dev`（winit / 字体）
- Windows：Visual Studio Build Tools（MSVC）
- macOS：Xcode Command Line Tools

```bash
# 桌面端
cargo build --release -p lanprint-desktop

# 桥接服务端
cargo build --release -p lanprint-bridge

# 运行
cargo run -p lanprint-desktop
cargo run -p lanprint-bridge -- --help
```

无界面辅助命令（同样适用于安装脚本/服务）：

```bash
lanPrint --set-password 'your-code'
lanPrint --share 'Printer Name'
lanPrint --unshare 'Printer Name'
lanPrint --device-name 'Office-PC'
lanPrint --cloud https://bridge.example.com 'server-code'
lanPrint --cloud-disable
lanPrint --headless          # 仅运行服务，不打开窗口
```

### CUPS 后端

Linux / macOS 客户端队列使用 `lanprint://` URI。安装包或 `install.sh` 会创建软链接：

```bash
sudo ln -sf /usr/local/bin/lanPrint /usr/lib/cups/backend/lanprint
```

CUPS 通过 `DEVICE_URI` 调用该程序，程序读取任务数据并转发给主机。

### Windows 虚拟打印机

客户端添加打印机时，lanPrint：

1. 在 `127.0.0.1:19100+` 启动本地 TCP 监听；
2. 通过 `Add-PrinterPort` / `prnport.vbs` 创建 TCP 打印端口；
3. 通过 `Add-Printer` 或 `rundll32 printui.dll` 创建本地队列；
4. 将后台写入的数据转发到主机。

Windows 7 下会回退到 inbox `Generic / Text Only` 驱动；若客户端已安装与主机相同的驱动，会优先使用该驱动。

## ☁️ 部署桥接服务端

### Docker

桥接服务默认监听 **52334** 端口。

```bash
docker run -d --name lanprint-bridge \
  --restart unless-stopped \
  -p 52334:52334 \
  -e LANPRINT_BRIDGE_AUTH_CODE='请替换为足够长的随机字符串' \
  -e LANPRINT_BRIDGE_PUBLIC_URL='https://bridge.example.com' \
  -v lanprint-data:/data \
  kaiyuan/lanprint-bridge:latest
```

或使用 Compose：

```bash
cd deploy
export LANPRINT_BRIDGE_AUTH_CODE='change-me'
docker compose up -d
```

生产环境请使用 `deploy/docker-compose.caddy.yml` 或 Nginx/Traefik 提供 HTTPS，并转发 WebSocket 升级。

### Linux / macOS 一键安装

```bash
curl -fsSL https://raw.githubusercontent.com/Kaiyuan/lanPrint/main/scripts/install-bridge.sh | sudo bash
```

脚本会生成服务器授权码，安装 systemd 服务（Linux）或 launchd 守护进程（macOS），配置文件位于 `/etc/lanprint-bridge/bridge.env`（Linux）。

Windows 服务端可直接下载 `lanprint-bridge_*_Windows_x86_64.zip`，解压后以计划任务或 NSSM 运行；生产环境仍建议放在 Linux + Docker/Caddy 后面。

### 配置项

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `LANPRINT_BRIDGE_ADDR` | `0.0.0.0:52334` | 监听地址 |
| `LANPRINT_BRIDGE_AUTH_CODE` | 必填 | 服务器授权码，主机注册时使用 |
| `LANPRINT_BRIDGE_DATA` | `data/hosts.json` | 共享码持久化文件 |
| `LANPRINT_BRIDGE_PUBLIC_URL` | 空 | 返回给客户端的公网 HTTPS 地址 |
| `RUST_LOG` | `info` | 日志级别 |

## 🔒 安全设计

- 主机授权码使用 Argon2id 哈希存储；修改授权码会轮换签名密钥并立即使旧令牌失效。
- 客户端通过授权码换取短期签名令牌（主机 7 天、打印机 30 天），不在配置中保存明文密码。
- 打印机授权码可在主机授权码之外额外设置。
- 云端桥接服务端只保存设备标识、共享码、时间戳和主机令牌；打印任务仅转发，不落盘。
- 生产环境必须使用 HTTPS/WSS；桥接服务本身不终止 TLS，建议放在 Caddy/Nginx/Traefik 后面。
- 局域网主机 API 使用 HTTP + 签名令牌。请仅在可信局域网中共享，或通过桥接服务的 HTTPS 通道跨公网使用。

## 🧪 测试

```bash
cargo test --workspace
cargo clippy --workspace --all-targets
```

Windows 7 构建（CI 中为 best-effort）：

```powershell
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly
cargo +nightly build -Z build-std=std,panic_abort --release `
  --target x86_64-win7-windows-msvc -p lanprint-desktop
```

## 📄 许可证

[MIT License](LICENSE)

Slint 采用 GPL-3.0 / Royalty-free 2.0 多许可协议。本项目以 Slint Royalty-free 2.0 方式使用 Slint；分发二进制时请遵守其条款。详见 [Slint 许可说明](https://slint.dev/license)。

## 🚢 发布流程

推送 `v*` 标签会触发 `.github/workflows/release.yml`：

- 编译 Windows x86_64 / i686、Windows 7、macOS Intel / Apple Silicon、Linux x86_64 / aarch64，以及 best-effort 的静态 musl 版本；
- 生成 Windows zip 与 Inno Setup 安装包、macOS `.app`、Linux tar.gz；
- 编译桥接服务端并上传；
- 构建 `kaiyuan/lanprint-bridge` 多架构 Docker 镜像并推送 Docker Hub；
- 创建 GitHub Release 并附加所有产物。

需要在仓库 Secrets 中配置：

| Secret | 说明 |
| --- | --- |
| `DOCKERHUB_USERNAME` | Docker Hub 用户名 |
| `DOCKERHUB_TOKEN` | Docker Hub Access Token |

GitHub Pages 由 `.github/workflows/pages.yml` 在 `docs/` 变更时自动部署。
