//go:build windows

package startup

import (
	"fmt"
	"os"

	"golang.org/x/sys/windows/registry"
)

const (
	runKeyPath = `Software\\Microsoft\\Windows\\CurrentVersion\\Run`
	runKeyName = "lanPrint"
)

func Enable() error {
	exe, err := os.Executable()
	if err != nil {
		return err
	}

	k, _, err := registry.CreateKey(registry.CURRENT_USER, runKeyPath, registry.SET_VALUE)
	if err != nil {
		return err
	}
	defer k.Close()

	return k.SetStringValue(runKeyName, fmt.Sprintf("\"%s\"", exe))
}

func Disable() error {
	k, err := registry.OpenKey(registry.CURRENT_USER, runKeyPath, registry.SET_VALUE)
	if err != nil {
		return err
	}
	defer k.Close()

	err = k.DeleteValue(runKeyName)
	if err == registry.ErrNotExist {
		return nil
	}
	return err
}

func IsEnabled() bool {
	k, err := registry.OpenKey(registry.CURRENT_USER, runKeyPath, registry.QUERY_VALUE)
	if err != nil {
		return false
	}
	defer k.Close()

	_, _, err = k.GetStringValue(runKeyName)
	return err == nil
}
