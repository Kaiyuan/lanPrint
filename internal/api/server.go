package api

import (
	"bytes"
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"io/fs"
	"net/http"
	"net/url"
	"strconv"
	"time"

	"github.com/gin-gonic/gin"
	"github.com/kaiyuan/lanPrint/internal/applog"
	"github.com/kaiyuan/lanPrint/internal/db"
	"github.com/kaiyuan/lanPrint/internal/localprint"
	"github.com/kaiyuan/lanPrint/internal/printer"
	"github.com/kaiyuan/lanPrint/internal/sysprint"
	webassets "github.com/kaiyuan/lanPrint/web"
)

var AppVersion = "dev"

func SetVersion(v string) {
	AppVersion = v
}

func hashPassword(pw string) string {
	h := sha256.Sum256([]byte(pw))
	return hex.EncodeToString(h[:])
}

func StartServer(port string) error {
	r := gin.Default()

	r.Use(func(c *gin.Context) {
		c.Writer.Header().Set("Access-Control-Allow-Origin", "*")
		c.Writer.Header().Set("Access-Control-Allow-Methods", "POST, GET, OPTIONS, PUT, DELETE")
		c.Writer.Header().Set("Access-Control-Allow-Headers", "Content-Type, Content-Length, Accept-Encoding, X-CSRF-Token, Authorization")
		if c.Request.Method == "OPTIONS" {
			c.AbortWithStatus(http.StatusNoContent)
			return
		}
		c.Next()
	})

	staticFS, _ := fs.Sub(webassets.Files, "static")
	i18nFS, _ := fs.Sub(webassets.Files, "i18n")

	r.StaticFS("/static", http.FS(staticFS))
	r.StaticFS("/i18n", http.FS(i18nFS))
	r.GET("/favicon.ico", func(c *gin.Context) { serveEmbedded(c, "favicon.ico", "image/x-icon") })
	r.GET("/", func(c *gin.Context) { serveEmbedded(c, "index.html", "text/html; charset=utf-8") })
	r.GET("/settings", func(c *gin.Context) { serveEmbedded(c, "index.html", "text/html; charset=utf-8") })
	r.NoRoute(func(c *gin.Context) {
		if c.Request.Method == http.MethodGet {
			serveEmbedded(c, "index.html", "text/html; charset=utf-8")
			return
		}
		c.JSON(http.StatusNotFound, gin.H{"error": "not found"})
	})

	v1 := r.Group("/api/v1")
	{
		v1.GET("/printers", getPrinters)
		v1.GET("/printers/shared", getSharedPrinters)
		v1.POST("/printers/share", sharePrinter)
		v1.GET("/printers/capabilities", getPrinterCapabilities)
		v1.POST("/printers/verify", verifyPrinterPassword)
		v1.POST("/rawprint", handleRawPrint)
		v1.GET("/client/discover", discoverClientDevices)
		v1.GET("/client/devices", listClientDevices)
		v1.POST("/client/devices", addClientDevice)
		v1.DELETE("/client/devices/:id", deleteClientDevice)
		v1.GET("/client/devices/:id/printers", listRemoteSharedPrinters)
		v1.POST("/client/connect", connectRemotePrinter)
		v1.DELETE("/client/connect", disconnectRemotePrinter)
		v1.GET("/settings", getAppSettings)
		v1.PUT("/settings", updateAppSettings)
		v1.GET("/stats", getStats)
		v1.GET("/logs", getLogs)
		v1.GET("/users", getUsers)
		v1.POST("/users", createUser)
		v1.DELETE("/users/:id", deleteUser)
		v1.GET("/version", func(c *gin.Context) {
			c.JSON(http.StatusOK, gin.H{"version": AppVersion})
		})
	}

	return r.Run(":" + port)
}

func serveEmbedded(c *gin.Context, name string, contentType string) {
	data, err := fs.ReadFile(webassets.Files, name)
	if err != nil {
		c.JSON(http.StatusNotFound, gin.H{"error": "embedded file not found", "file": name})
		return
	}
	c.Data(http.StatusOK, contentType, data)
}

func getPrinters(c *gin.Context) {
	printers, err := printer.GetLocalPrinters()
	if err != nil {
		c.JSON(http.StatusInternalServerError, gin.H{"error": err.Error()})
		return
	}
	sharedMap := make(map[string]bool)
	if sharedList, err := db.ListSharedPrinters(); err == nil {
		for _, p := range sharedList {
			sharedMap[p.Name] = true
		}
	}
	clientAddedMap, err := db.ListClientConnectedPrinterNames()
	if err != nil {
		clientAddedMap = map[string]bool{}
	}
	for i := range printers {
		printers[i].Shared = sharedMap[printers[i].Name]
		printers[i].AddedViaClient = clientAddedMap[printers[i].Name]
		printers[i].AnalyzedByAgent = true
	}
	c.JSON(http.StatusOK, printers)
}

func getSharedPrinters(c *gin.Context) {
	shared, err := db.ListSharedPrinters()
	if err != nil {
		c.JSON(http.StatusInternalServerError, gin.H{"error": err.Error()})
		return
	}
	c.JSON(http.StatusOK, shared)
}

// sharePrinter 处理打印机共享设置，支持密码保护
func sharePrinter(c *gin.Context) {
	var req struct {
		Name     string `json:"name"`
		Shared   bool   `json:"shared"`
		Password string `json:"password"` // 新增：可选密码
	}
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": "invalid request"})
		return
	}

	// 查询实际打印机能力并序列化为 JSON
	caps := printer.QueryLocalPrinterCapabilities(req.Name)
	capsJSON, _ := json.Marshal(caps)

	// 密码 hash（使用简单 sha256）
	pwHash := ""
	if req.Password != "" {
		pwHash = hashPassword(req.Password)
	}

	if err := db.SetPrinterShared(req.Name, req.Shared, pwHash, string(capsJSON)); err != nil {
		c.JSON(http.StatusInternalServerError, gin.H{"error": err.Error()})
		return
	}
	if req.Shared {
		_ = printer.RegisterPrinterBroadcast(req.Name, 631)
	}
	msg := "printer is now shared"
	if !req.Shared {
		msg = "printer sharing disabled"
	}
	c.JSON(http.StatusOK, gin.H{"status": "success", "message": msg})
}

// getPrinterCapabilities 供客户端查询指定打印机的能力
func getPrinterCapabilities(c *gin.Context) {
	name := c.Query("name")
	if name == "" {
		c.JSON(http.StatusBadRequest, gin.H{"error": "name required"})
		return
	}
	_, capsJSON, err := db.GetSharedPrinterAuth(name)
	if err != nil {
		c.JSON(http.StatusNotFound, gin.H{"error": "printer not found or not shared"})
		return
	}
	c.Data(http.StatusOK, "application/json", []byte(capsJSON))
}

// verifyPrinterPassword 服务端验证客户端提供的密码
func verifyPrinterPassword(c *gin.Context) {
	var req struct {
		Name     string `json:"name"`
		Password string `json:"password"`
	}
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": "invalid request"})
		return
	}
	pwHash, _, err := db.GetSharedPrinterAuth(req.Name)
	if err != nil {
		c.JSON(http.StatusNotFound, gin.H{"error": "printer not found"})
		return
	}
	// 如果打印机没有设置密码，直接通过
	if pwHash == "" {
		c.JSON(http.StatusOK, gin.H{"status": "ok"})
		return
	}
	// 验证密码
	if hashPassword(req.Password) != pwHash {
		c.JSON(http.StatusUnauthorized, gin.H{"error": "wrong password"})
		return
	}
	c.JSON(http.StatusOK, gin.H{"status": "ok"})
}

func discoverClientDevices(c *gin.Context) {
	devices, err := printer.DiscoverLanPrintDevices(4 * time.Second)
	if err != nil {
		c.JSON(http.StatusInternalServerError, gin.H{"error": err.Error()})
		return
	}
	c.JSON(http.StatusOK, devices)
}

func listClientDevices(c *gin.Context) {
	devices, err := db.ListRemoteDevices()
	if err != nil {
		c.JSON(http.StatusInternalServerError, gin.H{"error": err.Error()})
		return
	}
	c.JSON(http.StatusOK, devices)
}

func addClientDevice(c *gin.Context) {
	var req struct {
		Name    string `json:"name"`
		Address string `json:"address"`
		Port    int    `json:"port"`
	}
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": "invalid request"})
		return
	}
	if req.Address == "" {
		c.JSON(http.StatusBadRequest, gin.H{"error": "address is required"})
		return
	}
	if req.Name == "" {
		req.Name = req.Address
	}
	if req.Port == 0 {
		req.Port = 52333
	}
	if err := db.UpsertRemoteDevice(req.Name, req.Address, req.Port); err != nil {
		c.JSON(http.StatusInternalServerError, gin.H{"error": err.Error()})
		return
	}
	c.JSON(http.StatusOK, gin.H{"status": "success"})
}

func deleteClientDevice(c *gin.Context) {
	id, err := strconv.ParseInt(c.Param("id"), 10, 64)
	if err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": "invalid id"})
		return
	}
	if err := db.DeleteRemoteDevice(id); err != nil {
		c.JSON(http.StatusInternalServerError, gin.H{"error": err.Error()})
		return
	}
	c.JSON(http.StatusOK, gin.H{"status": "success"})
}

func listRemoteSharedPrinters(c *gin.Context) {
	id, err := strconv.ParseInt(c.Param("id"), 10, 64)
	if err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": "invalid id"})
		return
	}
	device, err := db.GetRemoteDevice(id)
	if err != nil {
		if err == sql.ErrNoRows {
			c.JSON(http.StatusNotFound, gin.H{"error": "device not found"})
			return
		}
		c.JSON(http.StatusInternalServerError, gin.H{"error": err.Error()})
		return
	}
	client := &http.Client{Timeout: 6 * time.Second}
	url := fmt.Sprintf("http://%s:%d/api/v1/printers/shared", device.Address, device.Port)
	resp, err := client.Get(url)
	if err != nil {
		c.JSON(http.StatusBadGateway, gin.H{"error": err.Error()})
		return
	}
	defer resp.Body.Close()
	var remote any
	if err := json.NewDecoder(resp.Body).Decode(&remote); err != nil {
		c.JSON(http.StatusBadGateway, gin.H{"error": "remote response parse failed"})
		return
	}
	c.JSON(http.StatusOK, remote)
}

func connectRemotePrinter(c *gin.Context) {
	var req struct {
		DeviceID    int64  `json:"device_id"`
		PrinterName string `json:"printer_name"`
		Password    string `json:"password"` // 用户提供的密码（首次连接）
	}
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": "invalid request"})
		return
	}
	device, err := db.GetRemoteDevice(req.DeviceID)
	if err != nil {
		if err == sql.ErrNoRows {
			c.JSON(http.StatusNotFound, gin.H{"error": "device not found"})
			return
		}
		c.JSON(http.StatusInternalServerError, gin.H{"error": err.Error()})
		return
	}

	// 先检查本地是否已保存密码
	savedPw, _ := db.GetClientConnectedPrinterPassword(req.PrinterName, device.Address)
	passwordToUse := savedPw
	if passwordToUse == "" && req.Password != "" {
		// 用户首次提供密码，在服务端验证
		passwordToUse = req.Password
	}

	// 向服务端发起验证请求
	verifyURL := fmt.Sprintf("http://%s:%d/api/v1/printers/verify", device.Address, device.Port)
	client := &http.Client{Timeout: 6 * time.Second}
	verifyBody, _ := json.Marshal(map[string]string{
		"name":     req.PrinterName,
		"password": passwordToUse,
	})
	vResp, err := client.Post(verifyURL, "application/json",
		bytes.NewReader(verifyBody))
	if err != nil {
		c.JSON(http.StatusBadGateway, gin.H{"error": "cannot reach server: " + err.Error()})
		return
	}
	defer vResp.Body.Close()
	if vResp.StatusCode == http.StatusUnauthorized {
		c.JSON(http.StatusUnauthorized, gin.H{"error": "password_required", "message": "需要输入打印机密码"})
		return
	}
	if vResp.StatusCode != http.StatusOK {
		c.JSON(http.StatusBadGateway, gin.H{"error": "server verification failed"})
		return
	}

	localName := "lanPrint-" + req.PrinterName

	id, err := db.UpsertClientConnectedPrinter(localName, req.PrinterName, device.Address, passwordToUse)
	if err != nil {
		c.JSON(http.StatusInternalServerError, gin.H{"error": err.Error()})
		return
	}

	// 尝试获取远程打印机的真实驱动名
	var targetDriverName string
	capsURL := fmt.Sprintf("http://%s:%d/api/v1/printers/capabilities?name=%s", device.Address, device.Port, url.QueryEscape(req.PrinterName))
	cResp, err := client.Get(capsURL)
	if err == nil {
		defer cResp.Body.Close()
		if cResp.StatusCode == http.StatusOK {
			var caps printer.PrinterCapabilities
			if err := json.NewDecoder(cResp.Body).Decode(&caps); err == nil {
				targetDriverName = caps.MakeModel
			}
		}
	}

	// 使用 localprint 安装跨平台虚拟打印机
	localPort := int(9100 + id) // 动态分配本地端口
	err = localprint.InstallRemotePrinter(localName, req.PrinterName, fmt.Sprintf("%s:%d", device.Address, device.Port), passwordToUse, localPort, targetDriverName)
	if err != nil {
		c.JSON(http.StatusInternalServerError, gin.H{"error": err.Error()})
		return
	}

	c.JSON(http.StatusOK, gin.H{"status": "success", "message": "remote printer connected", "local_printer_name": localName})
}

// handleRawPrint 接收来自客户端的原始打印数据并发送到物理打印机
func handleRawPrint(c *gin.Context) {
	printerName := c.PostForm("printer_name")
	password := c.PostForm("password")
	if printerName == "" {
		c.JSON(http.StatusBadRequest, gin.H{"error": "printer_name is required"})
		return
	}

	pwHash, _, err := db.GetSharedPrinterAuth(printerName)
	if err != nil {
		c.JSON(http.StatusNotFound, gin.H{"error": "printer not found"})
		return
	}
	if pwHash != "" && hashPassword(password) != pwHash {
		c.JSON(http.StatusUnauthorized, gin.H{"error": "invalid password"})
		return
	}

	fileHeader, err := c.FormFile("print_data")
	if err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": "print_data is required"})
		return
	}

	file, err := fileHeader.Open()
	if err != nil {
		c.JSON(http.StatusInternalServerError, gin.H{"error": err.Error()})
		return
	}
	defer file.Close()

	data, err := io.ReadAll(file)
	if err != nil {
		c.JSON(http.StatusInternalServerError, gin.H{"error": "read data failed"})
		return
	}

	applog.Infof("Received raw print job for '%s' (%d bytes)", printerName, len(data))

	// 分发给本地物理打印机
	if err := sysprint.PrintData(printerName, data); err != nil {
		applog.Errorf("Print job failed: %v", err)
		c.JSON(http.StatusInternalServerError, gin.H{"error": err.Error()})
		return
	}

	// 记录打印日志
	_ = db.AddLog(printerName, c.ClientIP(), fileHeader.Filename, 1, "success")

	c.JSON(http.StatusOK, gin.H{"status": "ok"})
}

func disconnectRemotePrinter(c *gin.Context) {
	var req struct {
		LocalName string `json:"local_name"`
	}
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": "invalid request"})
		return
	}

	p, err := db.GetClientConnectedPrinter(req.LocalName)
	if err != nil {
		c.JSON(http.StatusNotFound, gin.H{"error": "printer not found in database"})
		return
	}

	// 卸载虚拟打印机
	localPort := int(9100 + p.ID)
	if err := localprint.UninstallRemotePrinter(req.LocalName, localPort); err != nil {
		applog.Errorf("Uninstall virtual printer failed: %v", err)
		// 即使卸载失败，我们也继续从数据库中删除，以免死循环
	}

	if err := db.DeleteClientConnectedPrinter(req.LocalName); err != nil {
		c.JSON(http.StatusInternalServerError, gin.H{"error": err.Error()})
		return
	}

	c.JSON(http.StatusOK, gin.H{"status": "success", "message": "printer disconnected"})
}

func getAppSettings(c *gin.Context) {
	logLevel := applog.LevelString()
	if v, ok, err := db.GetSetting("log_level"); err == nil && ok && v != "" {
		logLevel = v
	}
	c.JSON(http.StatusOK, gin.H{"log_level": logLevel})
}

func updateAppSettings(c *gin.Context) {
	var req struct {
		LogLevel string `json:"log_level"`
	}
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": "invalid request"})
		return
	}
	if req.LogLevel == "" {
		req.LogLevel = "info"
	}
	if err := db.UpsertSetting("log_level", req.LogLevel); err != nil {
		c.JSON(http.StatusInternalServerError, gin.H{"error": err.Error()})
		return
	}
	applog.SetLevelByString(req.LogLevel)
	applog.Infof("log level updated to %s", req.LogLevel)
	c.JSON(http.StatusOK, gin.H{"status": "success"})
}

func getStats(c *gin.Context) {
	shared, _ := db.ListSharedPrinters()
	local, _ := printer.GetLocalPrinters()
	c.JSON(http.StatusOK, gin.H{"total_printers": len(local), "shared_printers": len(shared), "today_jobs": 0})
}
func getLogs(c *gin.Context)    { c.JSON(http.StatusOK, []interface{}{}) }
func getUsers(c *gin.Context)   { c.JSON(http.StatusOK, []interface{}{}) }
func createUser(c *gin.Context) { c.JSON(http.StatusOK, gin.H{"status": "success"}) }
func deleteUser(c *gin.Context) { c.JSON(http.StatusOK, gin.H{"status": "success"}) }
