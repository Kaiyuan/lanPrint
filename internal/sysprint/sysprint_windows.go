package sysprint

import (
	"bytes"
	"fmt"
	"os"
	"os/exec"
	"syscall"
	"unsafe"

	"github.com/kaiyuan/lanPrint/internal/applog"
	"github.com/kaiyuan/lanPrint/internal/procutil"
)

var (
	winspool             = syscall.NewLazyDLL("winspool.drv")
	procOpenPrinter      = winspool.NewProc("OpenPrinterW")
	procClosePrinter     = winspool.NewProc("ClosePrinter")
	procStartDocPrinter  = winspool.NewProc("StartDocPrinterW")
	procEndDocPrinter    = winspool.NewProc("EndDocPrinter")
	procStartPagePrinter = winspool.NewProc("StartPagePrinter")
	procEndPagePrinter   = winspool.NewProc("EndPagePrinter")
	procWritePrinter     = winspool.NewProc("WritePrinter")
)

type DOC_INFO_1 struct {
	pDocName    *uint16
	pOutputFile *uint16
	pDatatype   *uint16
}

// PrintData 根据数据格式（PDF或RAW）分发打印任务
func PrintData(printerName string, data []byte) error {
	if len(data) == 0 {
		return fmt.Errorf("empty print data")
	}

	// 检查是否为 PDF
	isPDF := len(data) > 4 && bytes.HasPrefix(data, []byte("%PDF"))
	// 检查是否为 XPS (ZIP 格式，包含 FixedDocument 内容)
	isXPS := len(data) > 4 && bytes.HasPrefix(data, []byte("PK\x03\x04")) && bytes.Contains(data, []byte("FixedDocument"))

	if isPDF {
		applog.Infof("Detected PDF data, using system native tools for '%s'", printerName)
		return printPDF(printerName, data)
	}
	if isXPS {
		applog.Infof("Detected XPS data, using system native tools for '%s'", printerName)
		return printXPS(printerName, data)
	}

	applog.Infof("Detected RAW data, using WinAPI WritePrinter for '%s'", printerName)
	return printRaw(printerName, data)
}

func printPDF(printerName string, data []byte) error {
	tmpFile, err := os.CreateTemp("", "lanprint-*.pdf")
	if err != nil {
		return fmt.Errorf("create temp pdf failed: %w", err)
	}
	defer os.Remove(tmpFile.Name())

	if _, err := tmpFile.Write(data); err != nil {
		return fmt.Errorf("write temp pdf failed: %w", err)
	}
	tmpFile.Close()

	// 方案 A: 使用系统 Shell Verb "PrintTo" (需要安装了 PDF 阅读器)
	script := fmt.Sprintf(`Start-Process -FilePath '%s' -Verb PrintTo -ArgumentList '"%s"' -Wait -WindowStyle Hidden`, tmpFile.Name(), printerName)
	cmd := exec.Command("powershell", "-NoProfile", "-NonInteractive", "-Command", script)
	procutil.HideWindow(cmd)
	
	if err := cmd.Run(); err == nil {
		return nil
	} else {
		applog.Warnf("PrintTo failed, trying Edge fallback: %v", err)
	}

	// 方案 B: 使用系统自带的 Microsoft Edge (Windows 10/11 原生内置)
	edgePaths := []string{
		`C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe`,
		`C:\Program Files\Microsoft\Edge\Application\msedge.exe`,
	}
	
	for _, path := range edgePaths {
		if _, err := os.Stat(path); err == nil {
			// Edge 命令行打印: --headless --print-to-default (如果想指定打印机，可以使用 --print-to)
			cmdEdge := exec.Command(path, "--headless", "--disable-gpu", "--print-to="+printerName, tmpFile.Name())
			procutil.HideWindow(cmdEdge)
			if err := cmdEdge.Run(); err == nil {
				applog.Infof("Successfully printed PDF using Microsoft Edge fallback")
				return nil
			}
		}
	}

	return fmt.Errorf("PDF 打印失败: 系统未找到关联的 PDF 阅读器，且尝试使用 Edge 浏览器打印也失败了。请在服务端安装 PDF 阅读器 (如 Adobe Reader) 或确保 Edge 浏览器可用。")
}

func printXPS(printerName string, data []byte) error {
	tmpFile, err := os.CreateTemp("", "lanprint-*.xps")
	if err != nil {
		return fmt.Errorf("create temp xps failed: %w", err)
	}
	defer os.Remove(tmpFile.Name())

	if _, err := tmpFile.Write(data); err != nil {
		return fmt.Errorf("write temp xps failed: %w", err)
	}
	tmpFile.Close()

	// XPS 在 Windows 7+ 是系统原生支持的格式，几乎都有默认关联程序
	script := fmt.Sprintf(`Start-Process -FilePath '%s' -Verb PrintTo -ArgumentList '"%s"' -Wait -WindowStyle Hidden`, tmpFile.Name(), printerName)
	cmd := exec.Command("powershell", "-NoProfile", "-NonInteractive", "-Command", script)
	procutil.HideWindow(cmd)
	
	if out, err := cmd.CombinedOutput(); err != nil {
		return fmt.Errorf("XPS PrintTo failed: %v, out: %s", err, string(out))
	}
	return nil
}

func printRaw(printerName string, data []byte) error {
	pNamePtr, err := syscall.UTF16PtrFromString(printerName)
	if err != nil {
		return err
	}

	var hPrinter syscall.Handle
	ret, _, err := procOpenPrinter.Call(uintptr(unsafe.Pointer(pNamePtr)), uintptr(unsafe.Pointer(&hPrinter)), 0)
	if ret == 0 {
		return fmt.Errorf("OpenPrinter failed: %v", err)
	}
	defer procClosePrinter.Call(uintptr(hPrinter))

	docName, _ := syscall.UTF16PtrFromString("lanPrint Job")
	dataType, _ := syscall.UTF16PtrFromString("RAW")

	docInfo := DOC_INFO_1{
		pDocName:  docName,
		pDatatype: dataType,
	}

	ret, _, err = procStartDocPrinter.Call(uintptr(hPrinter), 1, uintptr(unsafe.Pointer(&docInfo)))
	if ret == 0 {
		return fmt.Errorf("StartDocPrinter failed: %v", err)
	}
	defer procEndDocPrinter.Call(uintptr(hPrinter))

	ret, _, err = procStartPagePrinter.Call(uintptr(hPrinter))
	if ret == 0 {
		return fmt.Errorf("StartPagePrinter failed: %v", err)
	}

	var bytesWritten uint32
	ret, _, err = procWritePrinter.Call(
		uintptr(hPrinter),
		uintptr(unsafe.Pointer(&data[0])),
		uintptr(len(data)),
		uintptr(unsafe.Pointer(&bytesWritten)),
	)
	if ret == 0 {
		return fmt.Errorf("WritePrinter failed: %v", err)
	}

	procEndPagePrinter.Call(uintptr(hPrinter))
	return nil
}
