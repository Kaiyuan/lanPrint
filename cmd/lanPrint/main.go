package main

import (
	"flag"
	"fmt"
	"log"
	"net"
	"os"
	"os/exec"
	"runtime"
	"strings"

	"github.com/kaiyuan/lanPrint/internal/api"
	"github.com/kaiyuan/lanPrint/internal/applog"
	"github.com/kaiyuan/lanPrint/internal/db"
	"github.com/kaiyuan/lanPrint/internal/localprint"
	"github.com/kaiyuan/lanPrint/internal/printer"
	"github.com/kaiyuan/lanPrint/internal/procutil"
	"github.com/kaiyuan/lanPrint/internal/tray"
	"github.com/kardianos/service"
)

var (
	version = "dev"
	commit  = "none"
	date    = "unknown"
)

type program struct {
	exit chan struct{}
}

func initRuntime() {
	if err := db.Init(); err != nil {
		log.Fatalf("db init failed: %v", err)
	}
	if err := applog.Init(); err != nil {
		log.Fatalf("log init failed: %v", err)
	}
	if level, ok, _ := db.GetSetting("log_level"); ok && level != "" {
		applog.SetLevelByString(level)
	} else {
		applog.SetLevelByString("info")
	}
	applog.Infof("runtime initialized with log level: %s", applog.LevelString())
}

func (p *program) Start(s service.Service) error {
	go p.run()
	return nil
}

func (p *program) run() {
	initRuntime()
	startServersOnly()
	<-p.exit
}

func (p *program) Stop(s service.Service) error {
	printer.StopAllBroadcasts()
	close(p.exit)
	return nil
}

func startServersOnly() {
	go func() {
		if err := api.StartServer("52333"); err != nil {
			applog.Errorf("api server start failed: %v", err)
		}
	}()

	// 恢复已连接的客户端虚拟打印机端口监听
	localprint.RestoreReceivers()
}

func openSettingsPage() {
	url := "http://127.0.0.1:52333/"
	var cmd *exec.Cmd

	switch runtime.GOOS {
	case "windows":
		cmd = exec.Command("rundll32", "url.dll,FileProtocolHandler", url)
	case "darwin":
		cmd = exec.Command("open", url)
	default:
		cmd = exec.Command("xdg-open", url)
	}

	procutil.HideWindow(cmd)
	if err := cmd.Start(); err != nil {
		applog.Errorf("open settings failed: %v", err)
	}
}

func runDesktop() {
	initRuntime()
	startServersOnly()
	tray.Run(tray.Handlers{
		OpenSettings: openSettingsPage,
		Quit: func() {
			printer.StopAllBroadcasts()
			os.Exit(0)
		},
	})
}

func main() {
	api.SetVersion(version)
	svcFlag := flag.String("service", "", "service operation: install, uninstall, start, stop, restart")
	versionFlag := flag.Bool("v", false, "show version")
	backendFlag := flag.Bool("backend", false, "run as CUPS backend")
	flag.Parse()

	// 兼容 Unix: 仅当明确指定 -backend 或被 CUPS (路径包含 /cups/backend/) 调用时进入后端模式
	isCUPS := strings.Contains(os.Args[0], "/cups/backend/")
	if *backendFlag || isCUPS {
		localprint.RunAsCUPSBackend()
		return
	}

	if *versionFlag {
		fmt.Printf("lanPrint version: %s\ncommit: %s\nbuild date: %s\n", version, commit, date)
		return
	}

	svcConfig := &service.Config{
		Name:        "lanPrint",
		DisplayName: "lanPrint Printer Service",
		Description: "Convert local printers to network printers",
	}

	prg := &program{exit: make(chan struct{})}
	s, err := service.New(prg, svcConfig)
	if err != nil {
		log.Fatal(err)
	}

	if *svcFlag != "" {
		if err := service.Control(s, *svcFlag); err != nil {
			log.Fatalf("service operation failed: %v", err)
		}
		fmt.Printf("service operation succeeded: %s\n", *svcFlag)
		return
	}

	if !service.Interactive() {
		if err := s.Run(); err != nil {
			log.Fatal(err)
		}
		return
	}

	// 单实例检测：如果 55233 端口已被占用，说明已有实例在运行
	// 此时直接打开浏览器并退出，避免重复启动多个进程
	ln, err := net.Listen("tcp", ":55233")
	if err != nil {
		// 端口被占用，尝试唤起已存在的实例（即打开设置页面）
		openSettingsPage()
		return
	}
	_ = ln.Close()

	runDesktop()
}
