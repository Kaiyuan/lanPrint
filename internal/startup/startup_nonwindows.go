//go:build !windows && !linux

package startup

func Enable() error   { return nil }
func Disable() error  { return nil }
func IsEnabled() bool { return false }
