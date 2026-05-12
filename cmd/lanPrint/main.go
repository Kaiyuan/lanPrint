package main

import (
	"flag"
	"fmt"
	"log"
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

	// 兼容 Unix: 如果二进制文件名是 "lanprint" 或者带了 -backend 参数，进入 CUPS 后端模式
	if *backendFlag || strings.HasSuffix(strings.ToLower(os.Args[0]), "lanprint") {
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

	runDesktop()
}
