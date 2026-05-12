package printer

import (
	"context"
	"fmt"
	"log"
	"net"
	"net/url"
	"strconv"
	"strings"
	"time"

	"github.com/grandcat/zeroconf"
)

var servers []*zeroconf.Server

type DiscoveredDevice struct {
	Name    string `json:"name"`
	Address string `json:"address"`
	Port    int    `json:"port"`
	Source  string `json:"source"`
}

func RegisterPrinterBroadcast(name string, port int) error {
	ip, err := getLocalIP()
	if err != nil {
		return err
	}

	log.Printf("broadcast printer %s (IP: %s, Port: %d)", name, ip, port)

	txt := []string{
		"txtvers=1",
		"qtotal=1",
		"rp=ipp/printers/" + strings.ReplaceAll(name, " ", "-"),
		"ty=" + name,
		"adminurl=http://" + ip + ":52333",
		"note=lanPrint Shared Printer",
		"pdl=application/pdf,image/urf",
		"Color=T",
		"Duplex=T",
	}

	server, err := zeroconf.Register(name, "_ipp._tcp", "local.", port, txt, nil)
	if err != nil {
		return err
	}

	servers = append(servers, server)
	return nil
}

func DiscoverLanPrintDevices(timeout time.Duration) ([]DiscoveredDevice, error) {
	resolver, err := zeroconf.NewResolver(nil)
	if err != nil {
		return nil, err
	}

	entries := make(chan *zeroconf.ServiceEntry)
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()

	found := make(map[string]DiscoveredDevice)

	go func() {
		for entry := range entries {
			device := fromServiceEntry(entry)
			if device.Address == "" {
				continue
			}
			key := fmt.Sprintf("%s:%d", device.Address, device.Port)
			found[key] = device
		}
	}()

	if err := resolver.Browse(ctx, "_ipp._tcp", "local.", entries); err != nil {
		return nil, err
	}

	<-ctx.Done()
	close(entries)

	out := make([]DiscoveredDevice, 0, len(found))
	for _, d := range found {
		out = append(out, d)
	}
	return out, nil
}

func fromServiceEntry(entry *zeroconf.ServiceEntry) DiscoveredDevice {
	device := DiscoveredDevice{
		Name:   strings.TrimSpace(entry.Instance),
		Port:   52333,
		Source: "mdns",
	}

	for _, txt := range entry.Text {
		if strings.HasPrefix(txt, "adminurl=") {
			raw := strings.TrimPrefix(txt, "adminurl=")
			u, err := url.Parse(raw)
			if err == nil {
				if host := u.Hostname(); host != "" {
					device.Address = host
				}
				if p := u.Port(); p != "" {
					if port, convErr := strconv.Atoi(p); convErr == nil {
						device.Port = port
					}
				}
			}
		}

		if strings.HasPrefix(txt, "note=") && strings.Contains(strings.ToLower(txt), "lanprint") {
			device.Source = "lanprint-mdns"
		}
	}

	if device.Address == "" && len(entry.AddrIPv4) > 0 {
		device.Address = entry.AddrIPv4[0].String()
	}
	if device.Address == "" && len(entry.AddrIPv6) > 0 {
		device.Address = entry.AddrIPv6[0].String()
	}
	if device.Name == "" {
		device.Name = device.Address
	}

	return device
}

func StopAllBroadcasts() {
	for _, s := range servers {
		s.Shutdown()
	}
	servers = nil
}

func getLocalIP() (string, error) {
	addrs, err := net.InterfaceAddrs()
	if err != nil {
		return "", err
	}
	for _, address := range addrs {
		if ipnet, ok := address.(*net.IPNet); ok && !ipnet.IP.IsLoopback() {
			if ipnet.IP.To4() != nil {
				return ipnet.IP.String(), nil
			}
		}
	}
	return "", fmt.Errorf("no valid local IPv4 address found")
}
