//go:build !windows
// +build !windows

package sysprint

import (
	"fmt"
	"os"
	"os/exec"

	"github.com/kaiyuan/lanPrint/internal/applog"
)

// PrintData 根据数据格式（PDF或RAW）分发打印任务
func PrintData(printerName string, data []byte) error {
	if len(data) == 0 {
		return fmt.Errorf("empty print data")
	}

	applog.Infof("Unix printing to '%s' (%d bytes)", printerName, len(data))

	tmpFile, err := os.CreateTemp("", "lanprint-job-*")
	if err != nil {
		return fmt.Errorf("create temp file failed: %w", err)
	}
	defer os.Remove(tmpFile.Name())

	if _, err := tmpFile.Write(data); err != nil {
		return fmt.Errorf("write temp file failed: %w", err)
	}
	tmpFile.Close()

	// lp 命令可以自动识别大多数数据格式 (PDF, PostScript, RAW PCL 等) 并打印
	cmd := exec.Command("lp", "-d", printerName, tmpFile.Name())
	
	if out, err := cmd.CombinedOutput(); err != nil {
		return fmt.Errorf("lp failed: %v, out: %s", err, string(out))
	}

	return nil
}
