package localprint

import (
	"fmt"
	"io"
	"net/url"
	"os"
)

// RunAsCUPSBackend 执行 CUPS 后端逻辑
func RunAsCUPSBackend() {
	// CUPS backend 启动时如果不带参数，应该输出发现的设备信息
	if len(os.Args) == 1 {
		fmt.Println(`network lanprint "Unknown" "lanPrint Network Printer"`)
		os.Exit(0)
	}

	// 参数: job-id user title copies options [file]
	if len(os.Args) < 6 {
		fmt.Fprintf(os.Stderr, "ERROR: Invalid number of arguments\n")
		os.Exit(1)
	}

	deviceURI := os.Getenv("DEVICE_URI")
	if deviceURI == "" {
		fmt.Fprintf(os.Stderr, "ERROR: DEVICE_URI environment variable not set\n")
		os.Exit(1)
	}

	u, err := url.Parse(deviceURI)
	if err != nil {
		fmt.Fprintf(os.Stderr, "ERROR: Invalid DEVICE_URI: %v\n", err)
		os.Exit(1)
	}

	serverAddr := u.Host
	remotePrinterName := u.Path
	if len(remotePrinterName) > 0 && remotePrinterName[0] == '/' {
		remotePrinterName = remotePrinterName[1:] // 去掉开头的 /
	}
	remotePrinterName, _ = url.PathUnescape(remotePrinterName)

	password := ""
	if u.User != nil {
		password, _ = u.User.Password()
	}

	var data []byte
	if len(os.Args) == 7 {
		// 从文件读取数据
		filePath := os.Args[6]
		data, err = os.ReadFile(filePath)
		if err != nil {
			fmt.Fprintf(os.Stderr, "ERROR: Read print file failed: %v\n", err)
			os.Exit(1)
		}
	} else {
		// 从标准输入读取数据
		data, err = io.ReadAll(os.Stdin)
		if err != nil {
			fmt.Fprintf(os.Stderr, "ERROR: Read stdin failed: %v\n", err)
			os.Exit(1)
		}
	}

	// 发送任务到远程服务端
	err = SendJob(serverAddr, remotePrinterName, password, data)
	if err != nil {
		fmt.Fprintf(os.Stderr, "ERROR: Send job to lanPrint server failed: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("INFO: Print job successfully sent to lanPrint server")
	os.Exit(0)
}
