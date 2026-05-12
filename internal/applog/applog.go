package applog

import (
	"fmt"
	"io"
	"log"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
)

const (
	LevelDebug int32 = iota
	LevelInfo
	LevelWarn
	LevelError
)

var currentLevel int32 = LevelInfo

func Init() error {
	exePath, err := os.Executable()
	if err != nil {
		return err
	}
	logPath := filepath.Join(filepath.Dir(exePath), "lanPrint.log")
	f, err := os.OpenFile(logPath, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0644)
	if err != nil {
		return err
	}
	log.SetOutput(io.MultiWriter(f))
	log.SetFlags(log.LstdFlags | log.Lmicroseconds)
	return nil
}

func ParseLevel(s string) int32 {
	switch strings.ToLower(strings.TrimSpace(s)) {
	case "debug":
		return LevelDebug
	case "warn", "warning":
		return LevelWarn
	case "error":
		return LevelError
	default:
		return LevelInfo
	}
}

func LevelString() string {
	switch atomic.LoadInt32(&currentLevel) {
	case LevelDebug:
		return "debug"
	case LevelWarn:
		return "warn"
	case LevelError:
		return "error"
	default:
		return "info"
	}
}

func SetLevelByString(s string) {
	atomic.StoreInt32(&currentLevel, ParseLevel(s))
}

func logf(level int32, tag string, format string, args ...any) {
	if level < atomic.LoadInt32(&currentLevel) {
		return
	}
	log.Printf("[%s] %s", tag, fmt.Sprintf(format, args...))
}

func Debugf(format string, args ...any) { logf(LevelDebug, "DEBUG", format, args...) }
func Infof(format string, args ...any)  { logf(LevelInfo, "INFO", format, args...) }
func Warnf(format string, args ...any)  { logf(LevelWarn, "WARN", format, args...) }
func Errorf(format string, args ...any) { logf(LevelError, "ERROR", format, args...) }
