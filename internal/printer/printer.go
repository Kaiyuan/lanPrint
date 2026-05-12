package printer

import (
	"encoding/json"
	"fmt"
	"os/exec"
	"runtime"
	"strings"

	"github.com/kaiyuan/lanPrint/internal/procutil"
)

type Printer struct {
	Name            string `json:"name"`
	IsDefault       bool   `json:"is_default"`
	Status          string `json:"status"`
	Location        string `json:"location"`
	Shared          bool   `json:"shared"`
	AddedViaClient  bool   `json:"added_via_client"`
	AnalyzedByAgent bool   `json:"analyzed_by_agent"`
}

func GetLocalPrinters() ([]Printer, error) {
	if runtime.GOOS == "windows" {
		return getWindowsPrinters()
	}
	return getUnixPrinters()
}

// PrinterCapabilities 描述打印机能力
type PrinterCapabilities struct {
	Color         bool     `json:"color"`
	Duplex        bool     `json:"duplex"`
	A3            bool     `json:"a3"`
	MaxCopies     int      `json:"max_copies"`
	MakeModel     string   `json:"make_model"`
	MediaSizes    []string `json:"media_sizes"`
	ResolutionDPI int      `json:"resolution_dpi"`
}

// QueryLocalPrinterCapabilities 查询本地打印机实际能力（跨平台）
func QueryLocalPrinterCapabilities(name string) PrinterCapabilities {
	caps := PrinterCapabilities{MaxCopies: 999, ResolutionDPI: 600}
	if runtime.GOOS == "windows" {
		return queryWindowsPrinterCaps(name, caps)
	}
	return queryUnixPrinterCaps(name, caps)
}

func queryWindowsPrinterCaps(name string, caps PrinterCapabilities) PrinterCapabilities {
	// 查询打印机基本属性
	script := fmt.Sprintf(`
$p = Get-Printer -Name '%s' -ErrorAction SilentlyContinue
if ($p) {
    $cfg = Get-PrintConfiguration -PrinterName '%s' -ErrorAction SilentlyContinue
    $props = @{
        Color = ($p.RenderingMode -ne 'mono')
        Duplex = ($cfg.DuplexingMode -ne 'OneSided')
        MakeModel = $p.DriverName
    }
    $props | ConvertTo-Json -Compress
}`, name, name)
	cmd := exec.Command("powershell", "-NoProfile", "-NonInteractive", "-Command", script)
	out, err := cmd.Output()
	if err == nil && len(out) > 0 {
		var result map[string]interface{}
		if err2 := json.Unmarshal(out, &result); err2 == nil {
			if v, ok := result["Color"].(bool); ok {
				caps.Color = v
			}
			if v, ok := result["Duplex"].(bool); ok {
				caps.Duplex = v
			}
			if v, ok := result["MakeModel"].(string); ok {
				caps.MakeModel = v
			}
		}
	}

	// 查询支持的纸张尺寸
	script2 := fmt.Sprintf(`
(New-Object -ComObject SAPI.SpVoice | Out-Null; $null)
$p = New-Object System.Printing.PrintServer
$q = $p.GetPrintQueue('%s')
$caps2 = $q.GetPrintCapabilities()
($caps2.PageMediaSizeCapability | Select-Object -ExpandProperty PageMediaSizeName) -join ','
`, name)
	cmd2 := exec.Command("powershell", "-NoProfile", "-NonInteractive", "-Command", script2)
	if out2, err2 := cmd2.Output(); err2 == nil {
		sizes := strings.Split(strings.TrimSpace(string(out2)), ",")
		for _, s := range sizes {
			s = strings.TrimSpace(s)
			if s != "" {
				caps.MediaSizes = append(caps.MediaSizes, s)
				if strings.Contains(strings.ToUpper(s), "A3") {
					caps.A3 = true
				}
			}
		}
	}
	if len(caps.MediaSizes) == 0 {
		caps.MediaSizes = []string{"A4", "A3", "Letter"}
	}
	return caps
}

func queryUnixPrinterCaps(name string, caps PrinterCapabilities) PrinterCapabilities {
	// Mac/Linux: 使用 lpoptions 查询打印机属性
	cmd := exec.Command("lpoptions", "-p", name, "-l")
	out, err := cmd.Output()
	if err != nil {
		return caps
	}
	output := string(out)
	lines := strings.Split(output, "\n")
	for _, line := range lines {
		line = strings.ToLower(line)
		if strings.HasPrefix(line, "duplex") {
			caps.Duplex = true
		}
		if strings.Contains(line, "color") || strings.Contains(line, "colour") {
			caps.Color = true
		}
		if strings.Contains(line, "a3") {
			caps.A3 = true
		}
	}
	// 查询品牌型号
	cmd2 := exec.Command("lpstat", "-l", "-p", name)
	if out2, err2 := cmd2.Output(); err2 == nil {
		for _, line := range strings.Split(string(out2), "\n") {
			if strings.Contains(strings.ToLower(line), "description") {
				parts := strings.SplitN(line, ":", 2)
				if len(parts) == 2 {
					caps.MakeModel = strings.TrimSpace(parts[1])
				}
			}
		}
	}
	if len(caps.MediaSizes) == 0 {
		caps.MediaSizes = []string{"A4", "Letter"}
	}
	return caps
}

func getUnixPrinters() ([]Printer, error) {
	cmd := exec.Command("lpstat", "-a")
	out, err := cmd.Output()
	if err != nil {
		return nil, fmt.Errorf("lpstat failed: %w", err)
	}
	printers := make([]Printer, 0)
	for _, line := range strings.Split(string(out), "\n") {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}
		parts := strings.Fields(line)
		if len(parts) > 0 {
			printers = append(printers, Printer{
				Name:            parts[0],
				AnalyzedByAgent: true,
			})
		}
	}
	return printers, nil
}

// jsonUnmarshal 解析 JSON 数据
func jsonUnmarshal(data []byte, v interface{}) error {
	return json.Unmarshal(data, v)
}


func getWindowsPrinters() ([]Printer, error) {
	cmd := exec.Command("powershell", "-NoProfile", "-NonInteractive", "-Command", "Get-Printer | Select-Object Name, IsDefault, PrinterStatus, Location | ConvertTo-Csv -NoTypeInformation")
	procutil.HideWindow(cmd)
	output, err := cmd.Output()
	if err != nil {
		return nil, fmt.Errorf("get windows printers failed: %w", err)
	}

	lines := strings.Split(string(output), "\n")
	printers := make([]Printer, 0)

	for i := 1; i < len(lines); i++ {
		line := strings.TrimSpace(lines[i])
		if line == "" {
			continue
		}

		parts := strings.Split(line, ",")
		if len(parts) < 4 {
			continue
		}

		printers = append(printers, Printer{
			Name:            strings.Trim(parts[0], "\""),
			IsDefault:       strings.EqualFold(strings.Trim(parts[1], "\""), "true"),
			Status:          strings.Trim(parts[2], "\""),
			Location:        strings.Trim(parts[3], "\""),
			AnalyzedByAgent: true,
		})
	}

	return printers, nil
}
