# Changelog

## 2.0.2 — 2026-11-20

### 中文

**打印**
- 客户端任务改用 PDF 管线；Windows 主机通过 WinRT 渲染 PDF，并用真实打印机驱动输出，支持双面、小册子、纸张、进纸盒、彩色、份数和分辨率。
- 客户端队列优先使用主机同款驱动，Windows 系统打印对话框会显示真实的双面、彩色、纸盒和装订选项；已连接队列会自动切换，缺少驱动时回退 PDF 队列。
- 新增每台打印机的“打印设置”对话框，主机按打印机真实能力裁剪任务参数。
- 本地打印队列按“设备名-打印机名”命名。
- 测试打印改为彩色 A4 项目介绍页，单面单份，用于检查彩色和套准。
- 修复打印设置窗口关闭、取消无效的问题。

**主机与连接**
- 主机新增“活动”页面：记录设备连接和打印任务，显示时间、设备名以及局域网/云端来源，保留最近 500 条。
- 连接主机后自动打开该主机的打印机列表。
- 修复 macOS 连接 Windows 主机时授权码被误判的问题（Unicode NFC/NFD 规范化与换行处理）。

**界面与系统**
- 顶部按钮改为纯图标，鼠标停留 3 秒显示功能提示。
- 设置窗口内容左右留白对称。
- macOS 菜单栏只显示图标，不再显示软件名称。
- 新增自动更新：默认开启，发现新版本时在底部版本号旁显示黄色提示，点击即可下载安装。
- 文档站点支持亮色主题并跟随系统外观。

### English

**Printing**
- Client jobs now use a PDF pipeline; Windows hosts render PDFs through WinRT and print with the real driver, including duplex, booklet, paper, trays, colour, copies and resolution.
- Client queues reuse the host printer's driver when it is installed, so the Windows print dialog exposes the real duplex, colour, tray and finishing options. Existing queues are switched automatically; a PDF queue remains the fallback.
- Added per-printer print settings; the host clamps each job to the printer's actual capabilities.
- Local queues are named after the device and printer.
- Test print now sends a colour A4 project page, one-sided and single copy.
- Fixed the print-settings close and cancel actions.

**Host and connectivity**
- New host Activity page records device connections and print jobs with timestamps, device names and LAN/cloud markers, keeping the latest 500 entries.
- Opening a host now shows its printer list immediately.
- Fixed macOS-to-Windows access-code failures by normalising Unicode (NFC/NFD) and stray line breaks.

**UI and system**
- Toolbar buttons are icon-only, with a tooltip after a three-second hover.
- Symmetric padding in the settings content area.
- The macOS menu bar item shows only the icon, without the app name.
- Automatic updates are enabled by default; a yellow marker next to the version opens a download-and-install prompt.
- The documentation site now has a light theme that follows the system appearance.

## 2.0.1

Previous release.
