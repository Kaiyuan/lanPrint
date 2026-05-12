package db

import (
	"database/sql"
	"fmt"
	"os"
	"path/filepath"

	_ "modernc.org/sqlite"
)

var DB *sql.DB

type RemoteDevice struct {
	ID      int64  `json:"id"`
	Name    string `json:"name"`
	Address string `json:"address"`
	Port    int    `json:"port"`
}

type SharedPrinter struct {
	Name         string `json:"name"`
	Capabilities string `json:"capabilities"` // JSON: {color, duplex, a3, ...}
	HasPassword  bool   `json:"has_password"`
}

// PrinterCapabilities 描述打印机能力（服务端查询后返回给客户端）
type PrinterCapabilities struct {
	Color          bool   `json:"color"`           // 是否彩色打印机
	Duplex         bool   `json:"duplex"`          // 是否支持双面打印
	A3             bool   `json:"a3"`              // 是否支持 A3 纸张
	MaxCopies      int    `json:"max_copies"`      // 最大份数
	MakeModel      string `json:"make_model"`      // 品牌型号
	ColorModes     []string `json:"color_modes"`   // 支持的颜色模式
	MediaSizes     []string `json:"media_sizes"`   // 支持的纸张
	ResolutionDPI  int    `json:"resolution_dpi"` // 分辨率
}

func Init() error {
	exePath, err := os.Executable()
	if err != nil {
		return err
	}
	dbDir := filepath.Dir(exePath)
	dbPath := filepath.Join(dbDir, "lanPrint.db")

	var dbErr error
	DB, dbErr = sql.Open("sqlite", dbPath)
	if dbErr != nil {
		return fmt.Errorf("open db failed: %w", dbErr)
	}

	queries := []string{
		`CREATE TABLE IF NOT EXISTS printers (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT UNIQUE,
            display_name TEXT,
            is_shared INTEGER DEFAULT 0,
            permission_level INTEGER DEFAULT 0,
            password_hash TEXT DEFAULT '',
            capabilities TEXT DEFAULT '{}',
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );`,
		`CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            username TEXT UNIQUE,
            password TEXT,
            role TEXT DEFAULT 'user',
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );`,
		`CREATE TABLE IF NOT EXISTS print_logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            printer_name TEXT,
            user_name TEXT,
            document_name TEXT,
            pages INTEGER,
            status TEXT,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );`,
		`CREATE TABLE IF NOT EXISTS remote_devices (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            address TEXT NOT NULL,
            port INTEGER NOT NULL DEFAULT 52333,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(address, port)
        );`,
		`CREATE TABLE IF NOT EXISTS client_connected_printers (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            local_name TEXT NOT NULL UNIQUE,
            remote_name TEXT NOT NULL,
            remote_address TEXT NOT NULL,
            saved_password TEXT DEFAULT '',
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );`,
		`CREATE TABLE IF NOT EXISTS app_settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );`,
	}

	for _, q := range queries {
		if _, err := DB.Exec(q); err != nil {
			return fmt.Errorf("create table failed: %w", err)
		}
	}

	return nil
}

func AddLog(printer, user, document string, pages int, status string) error {
	_, err := DB.Exec("INSERT INTO print_logs (printer_name, user_name, document_name, pages, status) VALUES (?, ?, ?, ?, ?)",
		printer, user, document, pages, status)
	return err
}

func Authenticate(username, password string) (bool, error) {
	var storedPassword string
	err := DB.QueryRow("SELECT password FROM users WHERE username = ?", username).Scan(&storedPassword)
	if err == sql.ErrNoRows {
		return false, nil
	}
	if err != nil {
		return false, err
	}
	return storedPassword == password, nil
}

func SetPrinterShared(name string, shared bool, passwordHash string, capabilities string) error {
	value := 0
	if shared {
		value = 1
	}
	if capabilities == "" {
		capabilities = "{}"
	}
	_, err := DB.Exec(`
        INSERT INTO printers(name, display_name, is_shared, password_hash, capabilities)
        VALUES(?, ?, ?, ?, ?)
        ON CONFLICT(name) DO UPDATE SET
            is_shared = excluded.is_shared,
            display_name = excluded.display_name,
            password_hash = excluded.password_hash,
            capabilities = excluded.capabilities
    `, name, name, value, passwordHash, capabilities)
	return err
}

func GetSharedPrinterAuth(name string) (passwordHash string, capabilities string, err error) {
	err = DB.QueryRow(
		"SELECT password_hash, capabilities FROM printers WHERE name = ? AND is_shared = 1",
		name,
	).Scan(&passwordHash, &capabilities)
	return
}

func ListSharedPrinters() ([]SharedPrinter, error) {
	rows, err := DB.Query("SELECT name, capabilities, (password_hash != '') as has_password FROM printers WHERE is_shared = 1 ORDER BY name")
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	out := make([]SharedPrinter, 0)
	for rows.Next() {
		var p SharedPrinter
		if err := rows.Scan(&p.Name, &p.Capabilities, &p.HasPassword); err != nil {
			return nil, err
		}
		out = append(out, p)
	}
	return out, rows.Err()
}

func UpsertRemoteDevice(name, address string, port int) error {
	_, err := DB.Exec(`
        INSERT INTO remote_devices(name, address, port)
        VALUES(?, ?, ?)
        ON CONFLICT(address, port) DO UPDATE SET name = excluded.name
    `, name, address, port)
	return err
}

func ListRemoteDevices() ([]RemoteDevice, error) {
	rows, err := DB.Query("SELECT id, name, address, port FROM remote_devices ORDER BY id DESC")
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	out := make([]RemoteDevice, 0)
	for rows.Next() {
		var d RemoteDevice
		if err := rows.Scan(&d.ID, &d.Name, &d.Address, &d.Port); err != nil {
			return nil, err
		}
		out = append(out, d)
	}
	return out, rows.Err()
}

func GetRemoteDevice(id int64) (*RemoteDevice, error) {
	var d RemoteDevice
	err := DB.QueryRow("SELECT id, name, address, port FROM remote_devices WHERE id = ?", id).
		Scan(&d.ID, &d.Name, &d.Address, &d.Port)
	if err != nil {
		return nil, err
	}
	return &d, nil
}

func DeleteRemoteDevice(id int64) error {
	_, err := DB.Exec("DELETE FROM remote_devices WHERE id = ?", id)
	return err
}

// ClientConnectedPrinter 表示客户端已连接的远程打印机
type ClientConnectedPrinter struct {
	ID            int64  `json:"id"`
	LocalName     string `json:"local_name"`
	RemoteName    string `json:"remote_name"`
	RemoteAddress string `json:"remote_address"`
	SavedPassword string `json:"saved_password"`
}

func UpsertClientConnectedPrinter(localName, remoteName, remoteAddress, savedPassword string) (int64, error) {
	_, err := DB.Exec(`
        INSERT INTO client_connected_printers(local_name, remote_name, remote_address, saved_password)
        VALUES(?, ?, ?, ?)
        ON CONFLICT(local_name) DO UPDATE SET
            remote_name = excluded.remote_name,
            remote_address = excluded.remote_address,
            saved_password = excluded.saved_password
    `, localName, remoteName, remoteAddress, savedPassword)
	if err != nil {
		return 0, err
	}
	var id int64
	err = DB.QueryRow("SELECT id FROM client_connected_printers WHERE local_name = ?", localName).Scan(&id)
	return id, err
}

func GetClientConnectedPrinterPassword(remoteName, remoteAddress string) (string, error) {
	var pw string
	err := DB.QueryRow(
		"SELECT saved_password FROM client_connected_printers WHERE remote_name = ? AND remote_address = ?",
		remoteName, remoteAddress,
	).Scan(&pw)
	if err == sql.ErrNoRows {
		return "", nil
	}
	return pw, err
}

func ListClientConnectedPrinterNames() (map[string]bool, error) {
	rows, err := DB.Query("SELECT local_name FROM client_connected_printers")
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	out := make(map[string]bool)
	for rows.Next() {
		var name string
		if err := rows.Scan(&name); err != nil {
			return nil, err
		}
		out[name] = true
	}
	return out, rows.Err()
}

func GetAllClientConnectedPrinters() ([]ClientConnectedPrinter, error) {
	rows, err := DB.Query("SELECT id, local_name, remote_name, remote_address, saved_password FROM client_connected_printers")
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var out []ClientConnectedPrinter
	for rows.Next() {
		var p ClientConnectedPrinter
		if err := rows.Scan(&p.ID, &p.LocalName, &p.RemoteName, &p.RemoteAddress, &p.SavedPassword); err != nil {
			return nil, err
		}
		out = append(out, p)
	}
	return out, rows.Err()
}

func GetClientConnectedPrinter(localName string) (*ClientConnectedPrinter, error) {
	var p ClientConnectedPrinter
	err := DB.QueryRow("SELECT id, local_name, remote_name, remote_address, saved_password FROM client_connected_printers WHERE local_name = ?", localName).
		Scan(&p.ID, &p.LocalName, &p.RemoteName, &p.RemoteAddress, &p.SavedPassword)
	if err != nil {
		return nil, err
	}
	return &p, nil
}

func DeleteClientConnectedPrinter(localName string) error {
	_, err := DB.Exec("DELETE FROM client_connected_printers WHERE local_name = ?", localName)
	return err
}

func UpsertSetting(key, value string) error {
	_, err := DB.Exec(`
        INSERT INTO app_settings(key, value, updated_at)
        VALUES(?, ?, CURRENT_TIMESTAMP)
        ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = CURRENT_TIMESTAMP
    `, key, value)
	return err
}

func GetSetting(key string) (string, bool, error) {
	var value string
	err := DB.QueryRow("SELECT value FROM app_settings WHERE key = ?", key).Scan(&value)
	if err == sql.ErrNoRows {
		return "", false, nil
	}
	if err != nil {
		return "", false, err
	}
	return value, true, nil
}
