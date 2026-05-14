//go:build !windows
// +build !windows

package localprint

import (
	"fmt"
	"net/url"
	"os"
	"os/exec"
)

// RestoreReceivers 恢复 TCP 接收器（Unix 平台不需要，CUPS 独立运行）
func RestoreReceivers() {
	// No-op
}

// InstallRemotePrinter 使用 CUPS lpadmin 注册虚拟打印机
func InstallRemotePrinter(localPrinterName, remotePrinterName, serverAddr, password string, localPort int, targetDriverName string) error {
	// 确保 backend 已安装
	if err := ensureBackendInstalled(); err != nil {
		return fmt.Errorf("ensure CUPS backend failed: %v", err)
	}

	// CUPS device URI 格式: lanprint://[password@]serverAddr/printerName
	uri := fmt.Sprintf("lanprint://%s/%s", serverAddr, url.PathEscape(remotePrinterName))
	if password != "" {
		uri = fmt.Sprintf("lanprint://%s@%s/%s", url.UserPassword("user", password).String(), serverAddr, url.PathEscape(remotePrinterName))
	}

	// -m raw: 接收所有类型数据并交给 backend 处理
	// -E: 启用并接受任务
	cmd := exec.Command("lpadmin", "-p", localPrinterName, "-v", uri, "-E", "-m", "raw")
	if out, err := cmd.CombinedOutput(); err != nil {
		return fmt.Errorf("lpadmin install failed: %v, out: %s", err, string(out))
	}
	return nil
}

func ensureBackendInstalled() error {
	// 根据系统确定 CUPS backend 路径
	// macOS: /usr/libexec/cups/backend/
	// Linux: /usr/lib/cups/backend/
	backendPath := "/usr/lib/cups/backend/lanprint"
	if _, err := exec.LookPath("sw_vers"); err == nil {
		// macOS
		backendPath = "/usr/libexec/cups/backend/lanprint"
	}

	// 检查是否已存在且可执行
	if _, err := exec.Command("test", "-x", backendPath).CombinedOutput(); err == nil {
		return nil
	}

	// 如果不存在，尝试安装（需要权限）
	selfPath, _ := os.Executable()
	
	// 尝试使用 sudo cp 或 osascript (Mac) / pkexec (Linux)
	var cmd *exec.Cmd
	if _, err := exec.LookPath("osascript"); err == nil {
		// Mac 提权
		script := fmt.Sprintf("do shell script \"cp '%s' '%s' && chmod 755 '%s'\" with administrator privileges", selfPath, backendPath, backendPath)
		cmd = exec.Command("osascript", "-e", script)
	} else {
		// Linux 提权
		cmd = exec.Command("pkexec", "cp", selfPath, backendPath)
	}

	if out, err := cmd.CombinedOutput(); err != nil {
		msg := string(out)
		if strings.Contains(msg, "Read-only file system") || strings.Contains(msg, "只读文件系统") {
			return fmt.Errorf("安装失败：系统目录只读。请使用 .deb 安装包安装 lanPrint，或手动将程序复制到 %s", backendPath)
		}
		return fmt.Errorf("install backend failed (elevation required): %v, out: %s", err, msg)
	}
	
	// 确保权限正确 (Linux)
	if _, err := exec.LookPath("pkexec"); err == nil {
		_ = exec.Command("pkexec", "chmod", "755", backendPath).Run()
	}

	return nil
}

// UninstallRemotePrinter 删除 CUPS 打印机
func UninstallRemotePrinter(localPrinterName string, localPort int) error {
	cmd := exec.Command("lpadmin", "-x", localPrinterName)
	if out, err := cmd.CombinedOutput(); err != nil {
		return fmt.Errorf("lpadmin remove failed: %v, out: %s", err, string(out))
	}
	return nil
}
