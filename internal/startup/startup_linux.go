//go:build linux

package startup

import (
	"fmt"
	"os"
	"path/filepath"
)

func getAutostartPath() string {
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".config", "autostart", "lanprint.desktop")
}

func Enable() error {
	exe, err := os.Executable()
	if err != nil {
		return err
	}

	content := fmt.Sprintf(`[Desktop Entry]
Type=Application
Name=lanPrint
Exec=%s
Icon=%s
Comment=lanPrint Network Printer Service
Terminal=false
Categories=Utility;
`, exe, exe) // 假设可执行文件自带图标或者之后指定

	path := getAutostartPath()
	dir := filepath.Dir(path)
	if _, err := os.Stat(dir); os.IsNotExist(err) {
		_ = os.MkdirAll(dir, 0755)
	}

	return os.WriteFile(path, []byte(content), 0644)
}

func Disable() error {
	path := getAutostartPath()
	if _, err := os.Stat(path); err == nil {
		return os.Remove(path)
	}
	return nil
}

func IsEnabled() bool {
	path := getAutostartPath()
	_, err := os.Stat(path)
	return err == nil
}
