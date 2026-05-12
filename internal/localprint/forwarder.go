package localprint

import (
	"bytes"
	"fmt"
	"io"
	"mime/multipart"
	"net/http"
	"time"
)

// SendJob 发送打印任务到远程服务端
// serverAddr: 服务端地址 (e.g., 192.168.1.100:52333)
// printerName: 远程打印机名称
// password: 密码（如果设置了）
// data: 打印数据 (PDF 或 RAW)
func SendJob(serverAddr, printerName, password string, data []byte) error {
	url := fmt.Sprintf("http://%s/api/v1/rawprint", serverAddr)

	body := &bytes.Buffer{}
	writer := multipart.NewWriter(body)

	// 写入打印机名称
	if err := writer.WriteField("printer_name", printerName); err != nil {
		return fmt.Errorf("write printer_name failed: %w", err)
	}

	// 写入密码（如果有）
	if password != "" {
		if err := writer.WriteField("password", password); err != nil {
			return fmt.Errorf("write password failed: %w", err)
		}
	}

	// 写入打印数据
	part, err := writer.CreateFormFile("print_data", "job.dat")
	if err != nil {
		return fmt.Errorf("create form file failed: %w", err)
	}
	if _, err := io.Copy(part, bytes.NewReader(data)); err != nil {
		return fmt.Errorf("write print data failed: %w", err)
	}

	if err := writer.Close(); err != nil {
		return fmt.Errorf("close multipart writer failed: %w", err)
	}

	// 发送请求
	req, err := http.NewRequest("POST", url, body)
	if err != nil {
		return fmt.Errorf("create request failed: %w", err)
	}
	req.Header.Set("Content-Type", writer.FormDataContentType())

	// 超时设置得比较长，因为打印数据可能比较大
	client := &http.Client{Timeout: 5 * time.Minute}
	resp, err := client.Do(req)
	if err != nil {
		return fmt.Errorf("send request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		respBody, _ := io.ReadAll(resp.Body)
		return fmt.Errorf("server returned status: %s, body: %s", resp.Status, string(respBody))
	}

	return nil
}
