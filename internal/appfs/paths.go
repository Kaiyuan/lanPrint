package appfs

import (
	"os"
	"path/filepath"
)

func ExeDir() string {
	exe, err := os.Executable()
	if err != nil {
		return "."
	}
	return filepath.Dir(exe)
}

func AppPath(parts ...string) string {
	all := append([]string{ExeDir()}, parts...)
	return filepath.Join(all...)
}

func Resolve(parts ...string) string {
	candidates := []string{
		filepath.Join(append([]string{ExeDir()}, parts...)...),
		filepath.Join(append([]string{ExeDir(), ".."}, parts...)...),
		filepath.Join(parts...),
	}
	for _, p := range candidates {
		if _, err := os.Stat(p); err == nil {
			return p
		}
	}
	return candidates[0]
}

func ResolveFirst(candidateParts ...[]string) string {
	for _, parts := range candidateParts {
		p := Resolve(parts...)
		if _, err := os.Stat(p); err == nil {
			return p
		}
	}
	return ""
}
