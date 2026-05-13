//go:build linux

package tray

import (
	"io/fs"

	"fyne.io/systray"
	"github.com/kaiyuan/lanPrint/internal/startup"
	webassets "github.com/kaiyuan/lanPrint/web"
)

type Handlers struct {
	OpenSettings func()
	Quit         func()
}

func Run(h Handlers) {
	systray.Run(func() {
		systray.SetTitle("lanPrint")
		systray.SetTooltip("lanPrint")

		// Linux 下优先使用 PNG 图标以获得更好的兼容性
		if iconBytes, err := fs.ReadFile(webassets.Files, "static/images/lanprint-icon.png"); err == nil {
			systray.SetIcon(iconBytes)
		}

		mOpen := systray.AddMenuItem("打开设置", "Open settings")
		mStartup := systray.AddMenuItemCheckbox("开机启动", "Run at startup", startup.IsEnabled())
		systray.AddSeparator()
		mQuit := systray.AddMenuItem("退出", "Quit")

		go func() {
			for {
				select {
				case <-mOpen.ClickedCh:
					if h.OpenSettings != nil {
						h.OpenSettings()
					}
				case <-mStartup.ClickedCh:
					if mStartup.Checked() {
						if err := startup.Disable(); err == nil {
							mStartup.Uncheck()
						}
					} else {
						if err := startup.Enable(); err == nil {
							mStartup.Check()
						}
					}
				case <-mQuit.ClickedCh:
					if h.Quit != nil {
						h.Quit()
					}
					systray.Quit()
					return
				}
			}
		}()
	}, func() {})
}
