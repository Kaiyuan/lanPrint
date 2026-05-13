//go:build windows

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

		if iconBytes, err := fs.ReadFile(webassets.Files, "favicon.ico"); err == nil {
			systray.SetIcon(iconBytes)
		}

		mOpen := systray.AddMenuItem("\u6253\u5f00\u8bbe\u7f6e", "Open settings")
		mStartup := systray.AddMenuItemCheckbox("\u5f00\u673a\u542f\u52a8", "Run at startup", startup.IsEnabled())
		systray.AddSeparator()
		mQuit := systray.AddMenuItem("\u9000\u51fa", "Quit")

		go func() {
			for {
				select {
				case <-mOpen.ClickedCh:
					if h.OpenSettings != nil {
						h.OpenSettings()
					}
				case <-mStartup.ClickedCh:
					if mStartup.Checked() {
						_ = startup.Disable()
						mStartup.Uncheck()
					} else {
						_ = startup.Enable()
						mStartup.Check()
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
