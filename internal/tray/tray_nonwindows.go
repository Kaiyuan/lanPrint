//go:build !windows && !linux

package tray

type Handlers struct {
	OpenSettings func()
	Quit         func()
}

func Run(h Handlers) {
	select {}
}
