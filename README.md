# lanPrint

**lanPrint** 是一款轻量、强大的跨平台打印网关，旨在将任意本地打印机转换为可通过网络访问的智能打印机。它完全抛弃了传统的、复杂的 IPP/WSD 协议，自主实现了一套基于 HTTP 的高效传输与虚拟打印机劫持机制，支持 Windows、macOS 和 Linux。

## 🌟 核心特性

- **🚀 零配置快速启动**：一键运行，自动发现局域网内的其他 lanPrint 实例。
- **💻 全平台支持**：
  - **Windows (7/8/10/11)**：基于动态 TCP 端口劫持技术，自动驱动匹配，支持 PDF/XPS 原生降级打印。
  - **macOS (10.15+)**：集成 CUPS Backend，支持系统原生打印队列。
  - **Linux (Ubuntu/CentOS 等)**：支持 CUPS 架构，命令行友好。
- **🔒 安全可靠**：支持共享打印机密码访问，采用 SHA256 加密存储，保障打印安全。
- **⚡️ 零依赖设计**：软件内置所有功能逻辑。在 Windows 上即使未安装 PDF 阅读器，也能利用内置的 Edge 浏览器进行无头打印。
- **📊 任务监控**：实时查看打印任务状态、历史记录及打印机物理能力（彩色、双面、A3 等）。

## 🛠 工作原理

lanPrint 采用自主设计的“本地劫持 -> HTTP 转发 -> 远程落地”架构：
1. **本地劫持**：在客户端安装一个虚拟打印机。Windows 上通过监听本地 TCP 端口截获数据，Unix 上通过自定义 CUPS Backend 截获数据。
2. **HTTP 转发**：将截获的原始打印流（RAW, PDF, XPS 或 PS）通过加密后的 HTTP POST 请求发送至服务端。
3. **远程落地**：服务端接收数据后，根据数据类型自动选择最佳的本地分发方式（WinAPI `WritePrinter` 或是 `lp` 指令）直接下发给物理打印机。

## 📥 安装与运行

### 一键安装

**Linux & macOS:**
```bash
curl -fsSL https://raw.githubusercontent.com/kaiyuan/lanPrint/main/scripts/install.sh | bash
```

**Windows (PowerShell):**
```powershell
iwr -useb https://raw.githubusercontent.com/kaiyuan/lanPrint/main/scripts/install.ps1 | iex
```

### 手动下载
从 [Releases](https://github.com/kaiyuan/lanPrint/releases) 页面下载对应平台的二进制文件。

**Windows:**
1. 右键管理员权限运行 `lanPrint.exe`。
2. 在系统托盘菜单中选择“打开设置”。
3. 在“打印机”选项卡中选择要共享的打印机，点击“共享”。

**macOS/Linux:**
1. 赋予执行权限：`chmod +x lanPrint`
2. 建议以服务模式运行：`./lanPrint -service install && ./lanPrint -service start`

## 📖 开发者说明

### 本地构建

#### 前置要求
- Go 1.22+
- **Linux 必选**: `libappindicator3-dev` 和 `libgtk-3-dev` (用于托盘图标)

#### 构建命令

**Windows:**
```powershell
# 编译为无窗口 GUI 程序
go build -ldflags "-s -w -X main.version=v1.0.0 -H=windowsgui" -o lanPrint.exe ./cmd/lanPrint
```

**Linux (Ubuntu/UOS/Deepin):**
```bash
# 安装依赖
sudo apt-get install -y libappindicator3-dev libgtk-3-dev

# 启用 CGO 编译以支持托盘
CGO_ENABLED=1 go build -ldflags "-s -w -X main.version=v1.0.0" -o lanPrint ./cmd/lanPrint
```

**macOS:**
```bash
go build -ldflags "-s -w -X main.version=v1.0.0" -o lanPrint ./cmd/lanPrint
```

### 跨平台发布 (GoReleaser)
项目使用 GoReleaser 自动化处理多平台发布：
```bash
# 本地快照构建
goreleaser build --snapshot --clean
```

## 📄 开源协议
[MIT License](LICENSE)
