package localprint

import (
	"context"
	"io"
	"net"
	"sync"
	"time"

	"github.com/kaiyuan/lanPrint/internal/applog"
)

// TCPReceiver 监听本地端口并转发打印数据到远程服务器
type TCPReceiver struct {
	LocalPort   int
	ServerAddr  string
	PrinterName string
	Password    string

	listener net.Listener
	cancel   context.CancelFunc
	wg       sync.WaitGroup
}

var (
	receiversMu sync.Mutex
	receivers   = make(map[int]*TCPReceiver)
)

// StartReceiver 启动一个 TCP 接收器
func StartReceiver(localPort int, serverAddr, printerName, password string) error {
	receiversMu.Lock()
	defer receiversMu.Unlock()

	if _, exists := receivers[localPort]; exists {
		// 已经存在，先停止
		StopReceiver(localPort)
	}

	addr := &net.TCPAddr{IP: net.ParseIP("127.0.0.1"), Port: localPort}
	l, err := net.ListenTCP("tcp", addr)
	if err != nil {
		return err
	}

	ctx, cancel := context.WithCancel(context.Background())
	r := &TCPReceiver{
		LocalPort:   localPort,
		ServerAddr:  serverAddr,
		PrinterName: printerName,
		Password:    password,
		listener:    l,
		cancel:      cancel,
	}

	receivers[localPort] = r

	r.wg.Add(1)
	go r.serve(ctx)

	applog.Infof("Started TCP receiver for '%s' on port %d", printerName, localPort)
	return nil
}

// StopReceiver 停止指定的 TCP 接收器
func StopReceiver(localPort int) {
	if r, exists := receivers[localPort]; exists {
		r.cancel()
		r.listener.Close()
		r.wg.Wait()
		delete(receivers, localPort)
		applog.Infof("Stopped TCP receiver on port %d", localPort)
	}
}

// StopAllReceivers 停止所有接收器
func StopAllReceivers() {
	receiversMu.Lock()
	defer receiversMu.Unlock()
	for port := range receivers {
		StopReceiver(port)
	}
}

func (r *TCPReceiver) serve(ctx context.Context) {
	defer r.wg.Done()
	for {
		conn, err := r.listener.Accept()
		if err != nil {
			select {
			case <-ctx.Done():
				return
			default:
				applog.Errorf("TCP Accept error on port %d: %v", r.LocalPort, err)
				time.Sleep(1 * time.Second)
				continue
			}
		}

		go r.handleConnection(conn)
	}
}

func (r *TCPReceiver) handleConnection(conn net.Conn) {
	defer conn.Close()
	applog.Infof("Receiving print job on port %d for '%s'", r.LocalPort, r.PrinterName)

	// 设置超时时间
	conn.SetReadDeadline(time.Now().Add(30 * time.Second))

	// 读取所有打印数据
	data, err := io.ReadAll(conn)
	if err != nil && err != io.EOF {
		applog.Errorf("Read print data failed: %v", err)
		return
	}

	if len(data) == 0 {
		applog.Warnf("Received empty print job on port %d", r.LocalPort)
		return
	}

	applog.Infof("Received %d bytes of print data, forwarding to %s...", len(data), r.ServerAddr)

	// 转发到服务端
	err = SendJob(r.ServerAddr, r.PrinterName, r.Password, data)
	if err != nil {
		applog.Errorf("Forward print job failed: %v", err)
	} else {
		applog.Infof("Successfully forwarded print job to '%s'", r.PrinterName)
	}
}
