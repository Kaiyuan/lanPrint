package localprint

import (
	"fmt"
	"os/exec"
	"strings"

	"github.com/kaiyuan/lanPrint/internal/applog"
	"github.com/kaiyuan/lanPrint/internal/db"
	"github.com/kaiyuan/lanPrint/internal/procutil"
)

// RestoreReceivers 恢复所有已连接打印机的 TCP 接收器（仅 Windows 需要）
func RestoreReceivers() {
	printers, err := db.GetAllClientConnectedPrinters()
	if err != nil {
		applog.Errorf("Failed to get connected printers for restore: %v", err)
		return
	}
	for _, p := range printers {
		port := int(9100 + p.ID)
		err := StartReceiver(port, p.RemoteAddress, p.RemoteName, p.SavedPassword)
		if err != nil {
			applog.Errorf("Failed to restore receiver for '%s' on port %d: %v", p.LocalName, port, err)
		}
	}
}

// InstallRemotePrinter 创建本地 TCP 端口、安装虚拟打印机并启动数据接收器
func InstallRemotePrinter(localPrinterName, remotePrinterName, serverAddr, password string, localPort int, targetDriverName string) error {
	portName := fmt.Sprintf("lanPrint_Port_%d", localPort)

	// 1. 使用 WMI 创建 TCP/IP 打印机端口 (兼容 Win7/8/10/11)
	portScript := fmt.Sprintf(`
$portName = '%s'
$portNumber = %d
$wmi = [wmiclass]"Win32_TCPIPPrinterPort"
$port = $wmi.CreateInstance()
$port.Name = $portName
$port.HostAddress = "127.0.0.1"
$port.PortNumber = $portNumber
$port.Protocol = 1
$port.Put()
`, portName, localPort)

	cmdPort := exec.Command("powershell", "-NoProfile", "-NonInteractive", "-Command", portScript)
	procutil.HideWindow(cmdPort)
	if out, err := cmdPort.CombinedOutput(); err != nil {
		applog.Errorf("Create printer port failed: %v, out: %s", err, string(out))
		// 继续尝试，端口可能已经存在
	}

	// 2. 安装打印机
	var finalDriverName string
	var err error
	var out []byte

	// 第一志愿：尝试使用服务端传来的真实驱动名（如果客户端碰巧安装了同款驱动）
	if targetDriverName != "" {
		cmdInstall := exec.Command("rundll32", "printui.dll,PrintUIEntry", "/if", "/b", localPrinterName, "/r", portName, "/m", targetDriverName)
		procutil.HideWindow(cmdInstall)
		out, err = cmdInstall.CombinedOutput()
		if err == nil {
			finalDriverName = targetDriverName
		} else {
			applog.Warnf("Failed to install with true driver '%s': %v, out: %s. Falling back to Microsoft Print to PDF", targetDriverName, err, string(out))
		}
	}

	// 第二志愿：Microsoft Print to PDF (Win10+)
	if finalDriverName == "" {
		driverName := "Microsoft Print to PDF"
		cmdInstall := exec.Command("rundll32", "printui.dll,PrintUIEntry", "/if", "/b", localPrinterName, "/r", portName, "/m", driverName)
		procutil.HideWindow(cmdInstall)
		out, err = cmdInstall.CombinedOutput()
		if err == nil {
			finalDriverName = driverName
		} else {
			applog.Warnf("Failed to install with Microsoft Print to PDF: %v, out: %s. Falling back to Microsoft XPS Document Writer", err, string(out))
			
			// 第三志愿：Microsoft XPS Document Writer (Win7/8/10/11 几乎都有)
			driverName = "Microsoft XPS Document Writer"
			cmdInstall = exec.Command("rundll32", "printui.dll,PrintUIEntry", "/if", "/b", localPrinterName, "/r", portName, "/m", driverName)
			procutil.HideWindow(cmdInstall)
			out, err = cmdInstall.CombinedOutput()
			if err == nil {
				finalDriverName = driverName
			} else {
				applog.Warnf("Failed to install with Microsoft XPS Document Writer: %v, out: %s. Falling back to Generic / Text Only", err, string(out))
				
				// 第四志愿：Generic / Text Only
				driverName = "Generic / Text Only"
				cmdInstall = exec.Command("rundll32", "printui.dll,PrintUIEntry", "/if", "/b", localPrinterName, "/r", portName, "/m", driverName)
				procutil.HideWindow(cmdInstall)
				if out2, err2 := cmdInstall.CombinedOutput(); err2 != nil {
					return fmt.Errorf("install printer failed completely: %v, out: %s", err2, string(out2))
				}
				finalDriverName = driverName
			}
		}
	}

	applog.Infof("Successfully installed virtual printer '%s' on port %d with driver '%s'", localPrinterName, localPort, finalDriverName)
	
	// 启动数据接收和转发
	return StartReceiver(localPort, serverAddr, remotePrinterName, password)
}

// UninstallRemotePrinter 删除虚拟打印机、对应的端口并停止接收器
func UninstallRemotePrinter(localPrinterName string, localPort int) error {
	StopReceiver(localPort)

	// 1. 查询打印机使用的端口
	script := fmt.Sprintf(`(Get-WmiObject -Class Win32_Printer -Filter "Name='%s'").PortName`, localPrinterName)
	cmd := exec.Command("powershell", "-NoProfile", "-NonInteractive", "-Command", script)
	procutil.HideWindow(cmd)
	out, err := cmd.Output()
	portName := strings.TrimSpace(string(out))

	// 2. 删除打印机
	cmdDel := exec.Command("rundll32", "printui.dll,PrintUIEntry", "/dl", "/n", localPrinterName)
	procutil.HideWindow(cmdDel)
	if outDel, errDel := cmdDel.CombinedOutput(); errDel != nil {
		applog.Errorf("Delete printer failed: %v, out: %s", errDel, string(outDel))
	}

	// 3. 如果是 lanPrint 创建的端口，则删除该端口
	if portName != "" && strings.HasPrefix(portName, "lanPrint_Port_") {
		delPortScript := fmt.Sprintf(`(Get-WmiObject -Class Win32_TCPIPPrinterPort -Filter "Name='%s'").Delete()`, portName)
		cmdDelPort := exec.Command("powershell", "-NoProfile", "-NonInteractive", "-Command", delPortScript)
		procutil.HideWindow(cmdDelPort)
		cmdDelPort.Run()
	}

	return err
}
