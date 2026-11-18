use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

use lanprint_core::i18n::Translations;
use lanprint_core::{
    AppConfig, ClientPrinter, DiscoveredDevice, Language, ScanRequest, SharedPrinter,
};
use slint::{CloseRequestResponse, ComponentHandle, ModelRc, Timer, TimerMode, VecModel};

use crate::app::{App, AppEvent};
use crate::autostart;
use crate::error::AppError;
use crate::platform;

slint::include_modules!();

thread_local! {
    static EXPANDED_DEVICES: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    #[cfg(windows)]
    static WINDOW_EFFECTS_APPLIED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Windows 11 rounds decorated windows automatically; frameless windows need
/// to opt in through DWM. Extending the frame by one pixel also lets DWM draw
/// the standard window shadow around the borderless window.
#[cfg(windows)]
fn apply_window_effects(window: &MainWindow) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let handle = window.window().window_handle();
    let Ok(raw) = handle.window_handle() else {
        return false;
    };
    let RawWindowHandle::Win32(win32) = raw.as_raw() else {
        return false;
    };
    let hwnd = win32.hwnd.get() as winapi::shared::windef::HWND;
    unsafe {
        let margins = winapi::um::uxtheme::MARGINS {
            cxLeftWidth: 1,
            cxRightWidth: 1,
            cyTopHeight: 1,
            cyBottomHeight: 1,
        };
        winapi::um::dwmapi::DwmExtendFrameIntoClientArea(hwnd, &margins);

        let preference: i32 = 2; // DWMWCP_ROUND
        winapi::um::dwmapi::DwmSetWindowAttribute(
            hwnd,
            33, // DWMWA_WINDOW_CORNER_PREFERENCE
            &preference as *const _ as *const _,
            std::mem::size_of::<i32>() as u32,
        );
    }
    true
}

#[cfg(windows)]
fn try_apply_window_effects(window: &MainWindow) {
    if WINDOW_EFFECTS_APPLIED.with(|applied| applied.get()) || !window.window().is_visible() {
        return;
    }
    if apply_window_effects(window) {
        WINDOW_EFFECTS_APPLIED.with(|applied| applied.set(true));
    }
}

pub fn run(
    app: Arc<App>,
    events: Receiver<AppEvent>,
    show_window: bool,
    open_settings: bool,
    settings_tab: i32,
) -> Result<(), slint::PlatformError> {
    let window = MainWindow::new()?;
    let tray = TrayIcon::new()?;
    let cfg = app.config.read();
    let language = cfg.language();
    apply_language(&window, language);
    apply_theme(&window, &cfg);
    drop(cfg);
    apply_tray_language(&tray, language);
    refresh_ui(&window, &app);

    wire_callbacks(&window, &tray, &app);
    if open_settings {
        window.invoke_open_settings();
        window.set_settings_tab(settings_tab);
    }

    let weak = window.as_weak();
    window.window().on_close_requested(move || {
        if let Some(window) = weak.upgrade() {
            let _ = window.hide();
        }
        CloseRequestResponse::KeepWindowShown
    });

    let weak = window.as_weak();
    let app_for_timer = app.clone();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(150), move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        #[cfg(windows)]
        try_apply_window_effects(&window);
        while let Ok(event) = events.try_recv() {
            match event {
                AppEvent::Refresh => refresh_ui(&window, &app_for_timer),
                AppEvent::Toast(text) => {
                    show_event_toast(&window, &text);
                }
                AppEvent::ShowWindow => {
                    let _ = window.show();
                }
            }
        }
    });

    if show_window {
        window.show()?;
    }
    #[cfg(windows)]
    {
        let weak = window.as_weak();
        Timer::single_shot(Duration::from_millis(30), move || {
            if let Some(window) = weak.upgrade() {
                try_apply_window_effects(&window);
            }
        });
    }
    tray.show()?;
    slint::run_event_loop()?;
    drop(timer);
    Ok(())
}

fn wire_callbacks(window: &MainWindow, tray: &TrayIcon, app: &Arc<App>) {
    let weak = window.as_weak();

    window.on_refresh({
        let app = app.clone();
        move || {
            let app = app.clone();
            crate::runtime::spawn(async move {
                app.refresh_local().await;
                app.refresh_statuses().await;
            });
        }
    });

    window.on_open_connect({
        let app = app.clone();
        let weak = weak.clone();
        move || {
            let Some(window) = weak.upgrade() else { return };
            window.set_connect_open(true);
            window.set_connect_tab(0);
            window.set_connect_error("".into());
            window.set_connect_ok("".into());
            window.set_connect_scanning(true);
            start_lan_scan(&window, app.clone());
        }
    });

    window.on_scan_lan({
        let app = app.clone();
        let weak = weak.clone();
        move || {
            let Some(window) = weak.upgrade() else { return };
            window.set_connect_scanning(true);
            start_lan_scan(&window, app.clone());
        }
    });

    window.on_open_settings({
        let app = app.clone();
        let weak = weak.clone();
        move || {
            let Some(window) = weak.upgrade() else { return };
            let cfg = app.config.read();
            window.set_host_password_set(cfg.has_host_password());
            window.set_autostart(cfg.autostart);
            window.set_cloud_enabled(cfg.cloud.enabled);
            window.set_cloud_server(cfg.cloud.server_url.clone().into());
            window.set_cloud_code(cfg.cloud.server_code.clone().into());
            window.set_host_password_input("".into());
            apply_theme(&window, &cfg);
            window.set_settings_open(true);
            window.set_settings_tab(0);
        }
    });

    window.on_minimize_window({
        let weak = weak.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                window.window().set_minimized(true);
            }
        }
    });

    window.on_toggle_expand({
        let app = app.clone();
        let weak = weak.clone();
        move |id| {
            let id = id.to_string();
            let expanded_now = EXPANDED_DEVICES.with(|expanded| {
                let mut set = expanded.borrow_mut();
                if set.remove(&id) {
                    false
                } else {
                    set.insert(id.clone());
                    true
                }
            });
            if let Some(window) = weak.upgrade() {
                refresh_ui(&window, &app);
            }
            if expanded_now {
                let app = app.clone();
                crate::runtime::spawn(async move {
                    if let Err(err) = app.refresh_device(&id).await {
                        tracing::debug!("refresh device {id}: {err}");
                    }
                });
            }
        }
    });

    window.on_list_all_printers({
        let app = app.clone();
        let weak = weak.clone();
        move |id| {
            let Some(window) = weak.upgrade() else { return };
            let id = id.to_string();
            let name = app
                .config
                .read()
                .remote_devices
                .iter()
                .find(|d| d.id == id)
                .map(|d| d.name.clone())
                .unwrap_or_default();
            window.set_selected_device(id.clone().into());
            window.set_all_printers_id(id.clone().into());
            window.set_all_printers_name(name.into());
            window.set_all_printers_open(true);
            EXPANDED_DEVICES.with(|expanded| {
                expanded.borrow_mut().insert(id.clone());
            });
            let app = app.clone();
            crate::runtime::spawn(async move {
                if let Err(err) = app.refresh_device(&id).await {
                    tracing::debug!("refresh all printers: {err}");
                }
            });
        }
    });

    window.on_disconnect_all({
        let app = app.clone();
        let weak = weak.clone();
        move |id| {
            let Some(window) = weak.upgrade() else { return };
            let id = id.to_string();
            run_async(&window, app.clone(), {
                let app = app.clone();
                async move { app.disconnect_all_printers(&id).await }
            });
        }
    });

    window.on_remove_device({
        let app = app.clone();
        let weak = weak.clone();
        move |id| {
            let Some(window) = weak.upgrade() else { return };
            let id = id.to_string();
            EXPANDED_DEVICES.with(|expanded| {
                expanded.borrow_mut().remove(&id);
            });
            window.set_selected_device("".into());
            run_async(&window, app.clone(), {
                let app = app.clone();
                async move { app.remove_device(id).await }
            });
        }
    });

    window.on_add_printer({
        let app = app.clone();
        let weak = weak.clone();
        move |device_id, printer, password| {
            let Some(window) = weak.upgrade() else { return };
            let device_id = device_id.to_string();
            let printer = printer.to_string();
            let password = password.to_string();
            run_async(&window, app.clone(), {
                let app = app.clone();
                async move {
                    app.add_remote_printer(device_id, printer, password).await?;
                    Ok(())
                }
            });
        }
    });

    window.on_remove_printer({
        let app = app.clone();
        let weak = weak.clone();
        move |local_name| {
            let Some(window) = weak.upgrade() else { return };
            let local_name = local_name.to_string();
            run_async(&window, app.clone(), {
                let app = app.clone();
                async move { app.remove_printer(local_name).await }
            });
        }
    });

    window.on_test_print({
        let app = app.clone();
        let weak = weak.clone();
        move |local_name| {
            let Some(window) = weak.upgrade() else { return };
            let local_name = local_name.to_string();
            run_async(&window, app.clone(), {
                let app = app.clone();
                async move { app.test_print(local_name).await }
            });
        }
    });

    window.on_open_preferences({
        let weak = weak.clone();
        move |local_name| {
            if let Err(err) = platform::open_printer_preferences(local_name.as_str()) {
                if let Some(window) = weak.upgrade() {
                    show_toast(&window, &err.to_string());
                }
            }
        }
    });

    window.on_open_scan({
        let weak = weak.clone();
        move |local_name| {
            if let Some(window) = weak.upgrade() {
                window.set_scan_target(local_name);
                window.set_scan_busy(false);
                window.set_scan_open(true);
            }
        }
    });

    window.on_start_scan({
        let app = app.clone();
        let weak = weak.clone();
        move |local_name, resolution, color, source| {
            let Some(window) = weak.upgrade() else { return };
            let local_name = local_name.to_string();
            window.set_scan_busy(true);
            let request = ScanRequest {
                resolution: match resolution {
                    0 => 150,
                    1 => 200,
                    2 => 300,
                    _ => 600,
                },
                color: color == 0,
                format: "png".into(),
                source: if source == 1 {
                    "adf".into()
                } else {
                    "flatbed".into()
                },
                paper: "A4".into(),
                ..Default::default()
            };
            let weak_inner = window.as_weak();
            let app_inner = app.clone();
            crate::runtime::spawn(async move {
                let result = app_inner.scan(local_name, request).await;
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(window) = weak_inner.upgrade() {
                        window.set_scan_busy(false);
                        match result {
                            Ok(path) => {
                                window.set_scan_open(false);
                                show_toast(
                                    &window,
                                    &format!(
                                        "{} {}",
                                        translation(&app_inner, "scan_saved"),
                                        path.display()
                                    ),
                                );
                                let _ =
                                    platform::open_path(path.parent().unwrap_or(path.as_path()));
                            }
                            Err(err) => show_toast(&window, &err.to_string()),
                        }
                    }
                });
            });
        }
    });

    window.on_test_connection({
        let app = app.clone();
        let weak = weak.clone();
        move |_name, address, code, server, share_code, cloud| {
            let Some(window) = weak.upgrade() else { return };
            window.set_connect_testing(true);
            window.set_connect_error("".into());
            window.set_connect_ok("".into());
            if !cloud && address.trim().is_empty() {
                window.set_connect_testing(false);
                window.set_connect_error(window.global::<L>().get_host_required());
                return;
            }
            if cloud && share_code.trim().is_empty() {
                window.set_connect_testing(false);
                window.set_connect_error(window.global::<L>().get_share_code_placeholder());
                return;
            }
            let app_inner = app.clone();
            let weak_inner = window.as_weak();
            crate::runtime::spawn(async move {
                let cloud_arg = if cloud {
                    Some((server.to_string(), share_code.to_string()))
                } else {
                    None
                };
                let result = app_inner
                    .test_connection(address.as_str(), code.as_str(), cloud_arg)
                    .await;
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(window) = weak_inner.upgrade() {
                        window.set_connect_testing(false);
                        match result {
                            Ok(_) => {
                                window.set_connect_ok(window.global::<L>().get_connection_ok())
                            }
                            Err(err) => window.set_connect_error(error_text(&window, &err).into()),
                        }
                    }
                });
            });
        }
    });

    window.on_save_connection({
        let app = app.clone();
        let weak = weak.clone();
        move |name, address, code, server, share_code, cloud| {
            let Some(window) = weak.upgrade() else { return };
            if !cloud && address.trim().is_empty() {
                window.set_connect_error(window.global::<L>().get_host_required());
                return;
            }
            if cloud && share_code.trim().is_empty() {
                window.set_connect_error(window.global::<L>().get_share_code_placeholder());
                return;
            }
            window.set_connect_testing(true);
            window.set_connect_error("".into());
            let app_inner = app.clone();
            let weak_inner = window.as_weak();
            crate::runtime::spawn(async move {
                let cloud_arg = if cloud {
                    Some((server.to_string(), share_code.to_string()))
                } else {
                    None
                };
                let result = app_inner
                    .add_device(
                        name.to_string(),
                        address.to_string(),
                        code.to_string(),
                        cloud_arg,
                    )
                    .await;
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(window) = weak_inner.upgrade() {
                        window.set_connect_testing(false);
                        match result {
                            Ok(device) => {
                                EXPANDED_DEVICES.with(|expanded| {
                                    expanded.borrow_mut().insert(device.id.clone());
                                });
                                window.set_connect_open(false);
                                window.set_selected_device(device.id.clone().into());
                                refresh_ui(&window, &app_inner);
                            }
                            Err(err) => window.set_connect_error(error_text(&window, &err).into()),
                        }
                    }
                });
            });
        }
    });

    window.on_close_connect({
        let weak = weak.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                window.set_connect_open(false);
                window.set_connect_error("".into());
                window.set_connect_ok("".into());
            }
        }
    });

    window.on_language_changed({
        let app = app.clone();
        let weak = weak.clone();
        let tray_weak = tray.as_weak();
        move |value| {
            let language = Language::ALL
                .iter()
                .copied()
                .find(|l| l.native_name() == value.as_str())
                .unwrap_or(Language::En);
            app.config
                .edit(|cfg| cfg.language = language.code().to_string());
            if let Some(window) = weak.upgrade() {
                apply_language(&window, language);
                if let Some(tray) = tray_weak.upgrade() {
                    apply_tray_language(&tray, language);
                }
                refresh_ui(&window, &app);
            }
        }
    });

    window.on_set_autostart({
        let app = app.clone();
        let weak = weak.clone();
        move |enabled| {
            app.config.edit(|cfg| cfg.autostart = enabled);
            autostart::apply_on_startup(enabled);
            if let Some(window) = weak.upgrade() {
                show_toast(&window, &translation(&app, "settings_saved"));
            }
        }
    });

    window.on_appearance_changed({
        let app = app.clone();
        let weak = weak.clone();
        move |index| {
            let code = appearance_code(index);
            app.config.edit(|cfg| cfg.appearance = code.to_string());
            if let Some(window) = weak.upgrade() {
                let cfg = app.config.read();
                apply_theme(&window, &cfg);
            }
        }
    });

    window.on_accent_changed({
        let app = app.clone();
        let weak = weak.clone();
        move |index| {
            let code = accent_code(index);
            app.config.edit(|cfg| cfg.accent = code.to_string());
            if let Some(window) = weak.upgrade() {
                let cfg = app.config.read();
                apply_theme(&window, &cfg);
            }
        }
    });

    window.on_set_host_password({
        let app = app.clone();
        let weak = weak.clone();
        move |password| {
            let Some(window) = weak.upgrade() else { return };
            if password.trim().len() < 4 {
                show_toast(
                    &window,
                    &window.global::<L>().get_password_required().as_str(),
                );
                return;
            }
            let password = password.to_string();
            window.set_host_password_input("".into());
            window.set_host_password_set(true);
            run_async(&window, app.clone(), {
                let app = app.clone();
                async move { app.set_host_password(password).await }
            });
        }
    });

    window.on_share_printer({
        let app = app.clone();
        let weak = weak.clone();
        move |name, enabled| {
            let Some(window) = weak.upgrade() else { return };
            if enabled && !app.config.read().has_host_password() {
                show_toast(
                    &window,
                    &window.global::<L>().get_status_no_host_password().as_str(),
                );
            }
            let name = name.to_string();
            run_async(&window, app.clone(), {
                let app = app.clone();
                async move { app.share_printer(name, enabled, None, None).await }
            });
        }
    });

    window.on_edit_printer_password({
        let weak = weak.clone();
        move |name| {
            if let Some(window) = weak.upgrade() {
                window.set_password_target(name);
                window.set_password_kind(1);
                window.set_password_title(window.global::<L>().get_printer_password());
                window.set_password_input("".into());
                window.set_password_open(true);
            }
        }
    });

    window.on_password_confirm({
        let app = app.clone();
        let weak = weak.clone();
        move |kind, target, value| {
            let Some(window) = weak.upgrade() else { return };
            window.set_password_open(false);
            let target = target.to_string();
            let value = value.to_string();
            let device_id = window.get_selected_device().to_string();
            run_async(&window, app.clone(), {
                let app = app.clone();
                async move {
                    if kind == 2 {
                        app.add_remote_printer(device_id, target, value).await?;
                        Ok(())
                    } else if kind == 1 {
                        app.set_printer_password(target, value).await
                    } else {
                        app.set_host_password(value).await
                    }
                }
            });
        }
    });

    window.on_close_password({
        let weak = weak.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                window.set_password_open(false);
            }
        }
    });

    window.on_close_scan({
        let weak = weak.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                window.set_scan_open(false);
            }
        }
    });

    window.on_cloud_save({
        let app = app.clone();
        let weak = weak.clone();
        move |enabled, server, code| {
            let Some(window) = weak.upgrade() else { return };
            app.config.edit(|cfg| {
                cfg.cloud.server_url = server.trim().trim_end_matches('/').to_string();
            });
            run_async(&window, app.clone(), {
                let app = app.clone();
                async move {
                    app.set_cloud_config(enabled, server.to_string(), code.to_string())
                        .await
                }
            });
        }
    });

    window.on_copy_share_code({
        let app = app.clone();
        let weak = weak.clone();
        move || {
            let code = app.config.read().cloud.share_code.clone();
            if code.is_empty() {
                return;
            }
            let _ = platform::copy_text(&code);
            if let Some(window) = weak.upgrade() {
                show_toast(&window, &window.global::<L>().get_copied().as_str());
            }
        }
    });

    window.on_toast_close({
        let weak = weak.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                window.set_toast_open(false);
            }
        }
    });

    window.on_open_docs({
        let weak = weak.clone();
        move || {
            if let Err(err) = open::that("https://kaiyuan.github.io/lanPrint/") {
                if let Some(window) = weak.upgrade() {
                    show_toast(&window, &err.to_string());
                }
            }
        }
    });

    window.on_open_logs({
        let app = app.clone();
        move || {
            app.open_logs();
        }
    });

    window.on_open_scan_folder({
        move || {
            let dir = lanprint_core::config::default_download_dir().join("lanPrint-scans");
            let _ = std::fs::create_dir_all(&dir);
            let _ = platform::open_path(&dir);
        }
    });

    window.on_set_first_run_password({
        let app = app.clone();
        let weak = weak.clone();
        move |password| {
            let Some(window) = weak.upgrade() else { return };
            if password.trim().len() < 4 {
                show_toast(
                    &window,
                    &window.global::<L>().get_password_required().as_str(),
                );
                return;
            }
            let password = password.to_string();
            window.set_first_run_open(false);
            run_async(&window, app.clone(), {
                let app = app.clone();
                async move { app.set_host_password(password).await }
            });
        }
    });

    window.on_skip_first_run({
        let app = app.clone();
        let weak = weak.clone();
        move || {
            app.config.edit(|cfg| cfg.first_run = false);
            if let Some(window) = weak.upgrade() {
                window.set_first_run_open(false);
            }
        }
    });

    window.on_close_overlays({
        let weak = weak.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                window.set_connect_open(false);
                window.set_settings_open(false);
                window.set_password_open(false);
                window.set_scan_open(false);
                window.set_first_run_open(false);
            }
        }
    });

    window.on_quit_app({
        let app = app.clone();
        move || {
            let app = app.clone();
            crate::runtime::spawn(async move {
                app.shutdown().await;
                let _ = slint::invoke_from_event_loop(|| {
                    let _ = slint::quit_event_loop();
                });
            });
        }
    });

    let window_weak = window.as_weak();
    tray.on_open(move || {
        if let Some(window) = window_weak.upgrade() {
            let _ = window.show();
        }
    });

    let app_quit = app.clone();
    tray.on_quit(move || {
        let app = app_quit.clone();
        crate::runtime::spawn(async move {
            app.shutdown().await;
            let _ = slint::invoke_from_event_loop(|| {
                let _ = slint::quit_event_loop();
            });
        });
    });
}

fn start_lan_scan(window: &MainWindow, app: Arc<App>) {
    let weak = window.as_weak();
    crate::runtime::spawn(async move {
        let devices = app.scan_lan().await;
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(window) = weak.upgrade() {
                window.set_connect_scanning(false);
                window.set_discoveries(ModelRc::new(VecModel::from(
                    devices.iter().map(discovery_item).collect::<Vec<_>>(),
                )));
            }
        });
    });
}

fn run_async<F>(window: &MainWindow, app: Arc<App>, future: F)
where
    F: std::future::Future<Output = crate::error::Result<()>> + Send + 'static,
{
    let weak = window.as_weak();
    crate::runtime::spawn(async move {
        let result = future.await;
        let app = app.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(window) = weak.upgrade() {
                match result {
                    Ok(()) => refresh_ui(&window, &app),
                    Err(err) => {
                        let message = error_text(&window, &err);
                        show_toast(&window, &message);
                    }
                }
            }
        });
    });
}

fn refresh_ui(window: &MainWindow, app: &Arc<App>) {
    let cfg = app.config.read();
    let snap = app.snapshot();
    let mut selected = window.get_selected_device().to_string();

    if selected.is_empty() {
        if let Some(first) = cfg.remote_devices.first() {
            selected = first.id.clone();
            window.set_selected_device(first.id.clone().into());
        }
    }

    let remote: &[SharedPrinter] = snap
        .remote_printers
        .get(&selected)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let device_online = cfg
        .remote_devices
        .iter()
        .find(|d| d.id == selected)
        .map(|d| d.online)
        .unwrap_or(false);
    let printers: Vec<PrinterItem> = remote
        .iter()
        .map(|p| {
            let cp = cfg
                .client_printers
                .iter()
                .find(|cp| cp.device_id == selected && cp.remote_name == p.name);
            let online = p.online && device_online;
            let status = if online {
                translation_lang(cfg.language(), "printer_online")
            } else {
                translation_lang(cfg.language(), "printer_offline")
            };
            PrinterItem {
                local_name: cp.map(|c| c.local_name.clone()).unwrap_or_default().into(),
                remote_name: p.name.clone().into(),
                device_id: selected.clone().into(),
                display_name: p.display_name.clone().into(),
                driver: p.driver.clone().into(),
                status: status.into(),
                online,
                supports_scan: p.supports_scan,
                added: cp.is_some(),
                has_password: p.has_password,
                preferences: preferences_text(&p.preferences),
            }
        })
        .collect();
    window.set_printers(ModelRc::new(VecModel::from(printers)));

    let expanded = EXPANDED_DEVICES.with(|set| set.borrow().clone());
    let mut rows = Vec::new();
    for device in &cfg.remote_devices {
        let connected: Vec<&ClientPrinter> = cfg
            .client_printers
            .iter()
            .filter(|printer| printer.device_id == device.id)
            .collect();
        let is_expanded = expanded.contains(&device.id);
        let address = if device.cloud {
            format!("{} · {}", device.address, device.share_code)
        } else {
            format!("{}:{}", device.address, device.port)
        };
        rows.push(HostRow {
            kind: 0,
            id: device.id.clone().into(),
            name: device.name.clone().into(),
            address: address.into(),
            online: device.online,
            cloud: device.cloud,
            printer_count: connected.len() as i32,
            expanded: is_expanded,
            local_name: "".into(),
            remote_name: "".into(),
            display_name: "".into(),
            driver: "".into(),
            status: "".into(),
            supports_scan: false,
            added: false,
            has_password: false,
            preferences: "".into(),
        });
        if !is_expanded {
            continue;
        }
        for printer in connected {
            let shared = snap
                .remote_printers
                .get(&device.id)
                .and_then(|list| list.iter().find(|p| p.name == printer.remote_name));
            let online =
                device.online && printer.online && shared.map(|p| p.online).unwrap_or(true);
            let status = translation_lang(
                cfg.language(),
                if online {
                    "printer_online"
                } else {
                    "printer_offline"
                },
            );
            rows.push(HostRow {
                kind: 1,
                id: device.id.clone().into(),
                name: device.name.clone().into(),
                address: "".into(),
                online,
                cloud: device.cloud,
                printer_count: 0,
                expanded: false,
                local_name: printer.local_name.clone().into(),
                remote_name: printer.remote_name.clone().into(),
                display_name: printer.display_name.clone().into(),
                driver: shared.map(|p| p.driver.clone()).unwrap_or_default().into(),
                status: status.into(),
                supports_scan: printer.supports_scan
                    || shared.map(|p| p.supports_scan).unwrap_or(false),
                added: true,
                has_password: shared.map(|p| p.has_password).unwrap_or(false),
                preferences: preferences_text(&printer.preferences),
            });
        }
    }
    window.set_rows(ModelRc::new(VecModel::from(rows)));

    let locals: Vec<LocalPrinterItem> = snap
        .local_printers
        .iter()
        .map(|p| {
            let shared = cfg.shared_printer(&p.name);
            LocalPrinterItem {
                name: p.name.clone().into(),
                driver: p.driver.clone().into(),
                status: p.status.clone().into(),
                supports_scan: p.capabilities.supports_scan,
                shared: shared.map(|s| s.enabled).unwrap_or(false),
                has_password: shared.map(|s| s.has_password()).unwrap_or(false),
            }
        })
        .collect();
    window.set_local_printers(ModelRc::new(VecModel::from(locals)));

    let lang_index = Language::ALL
        .iter()
        .position(|l| l.code() == cfg.language)
        .unwrap_or(0) as i32;
    window.set_language_index(lang_index);
    window.set_host_password_set(cfg.has_host_password());
    window.set_autostart(cfg.autostart);
    window.set_cloud_enabled(cfg.cloud.enabled);
    apply_theme(window, &cfg);
    window.set_share_code(cfg.cloud.share_code.clone().into());
    window.set_version_text(lanprint_core::VERSION.into());
    window.set_busy(snap.busy);

    let status = if !cfg.has_host_password() {
        translation_lang(cfg.language(), "status_no_host_password")
    } else {
        translation_lang(cfg.language(), "status_host_running")
            .replace("{port}", &cfg.api_port.to_string())
    };
    window.set_status_text(status.into());
    let lang = cfg.language();
    window.set_cloud_status(
        if cfg.cloud.enabled {
            match snap.cloud_status.as_str() {
                "cloud_connected" => translation_lang(lang, "cloud_connected"),
                "cloud_disconnected" => translation_lang(lang, "cloud_disconnected"),
                s if s.starts_with("error:") => translation_lang(lang, "cloud_offline"),
                _ => String::new(),
            }
        } else {
            String::new()
        }
        .into(),
    );

    if cfg.first_run && !cfg.has_host_password() {
        window.set_first_run_open(true);
    }
}

fn discovery_item(d: &DiscoveredDevice) -> DiscoveryItem {
    DiscoveryItem {
        id: d.id.clone().into(),
        name: d.name.clone().into(),
        address: d.address.clone().into(),
        os: d.os.clone().into(),
        version: d.version.clone().into(),
        cloud: d.cloud,
        shared_count: d.shared_count as i32,
        requires_password: d.requires_password,
    }
}

fn preferences_text(p: &lanprint_core::PrinterPreferences) -> slint::SharedString {
    let mut parts = Vec::new();
    if !p.paper.is_empty() {
        parts.push(p.paper.clone());
    }
    parts.push(if p.color {
        "Color".into()
    } else {
        "Mono".into()
    });
    parts.push(match p.duplex {
        lanprint_core::DuplexMode::Simplex => "Simplex".into(),
        lanprint_core::DuplexMode::LongEdge => "Duplex".into(),
        lanprint_core::DuplexMode::ShortEdge => "Duplex (short)".into(),
    });
    if p.resolution > 0 {
        parts.push(format!("{} dpi", p.resolution));
    }
    parts.join(" · ").into()
}

fn error_text(window: &MainWindow, err: &AppError) -> String {
    let text = err.to_string();
    let l = window.global::<L>();
    if text.contains("wrong_password")
        || text.contains("invalid access code")
        || text.contains("401")
    {
        l.get_wrong_password().to_string()
    } else if text.contains("token_expired") || text.contains("expired") {
        l.get_token_expired().to_string()
    } else if text.contains("printer password required") {
        l.get_required_for_printer().to_string()
    } else if text.contains("host access code is not configured") {
        l.get_status_no_host_password().to_string()
    } else {
        text
    }
}

fn show_toast(window: &MainWindow, text: &str) {
    window.set_toast_text(text.into());
    window.set_toast_open(true);
}

fn show_event_toast(window: &MainWindow, event: &str) {
    if let Some(path) = event.strip_prefix("scan_saved:") {
        show_toast(
            window,
            &format!("{} {}", window.global::<L>().get_scan_saved(), path),
        );
        return;
    }
    let l = window.global::<L>();
    let text = match event {
        "password_saved" => l.get_password_saved().to_string(),
        "settings_saved" => l.get_settings_saved().to_string(),
        "cloud_registered" => l.get_cloud_registered().to_string(),
        "test_print_sent" => l.get_test_print_sent().to_string(),
        "printer_added" => l.get_printer_added().to_string(),
        "printer_removed" => l.get_printer_removed().to_string(),
        "device_removed" => l.get_device_removed().to_string(),
        other => other.to_string(),
    };
    show_toast(window, &text);
}

fn translation(app: &Arc<App>, key: &str) -> String {
    let lang = app.config.read().language();
    Translations::load(lang).get(key)
}

fn translation_lang(language: Language, key: &str) -> String {
    Translations::load(language).get(key)
}

fn apply_tray_language(tray: &TrayIcon, language: Language) {
    let tr = Translations::load(language);
    tray.set_open_text(tr.get("tray_open").into());
    tray.set_quit_text(tr.get("tray_quit").into());
}

fn appearance_index(code: &str) -> i32 {
    match code {
        "light" => 1,
        "dark" => 2,
        _ => 0,
    }
}

fn appearance_code(index: i32) -> &'static str {
    match index {
        1 => "light",
        2 => "dark",
        _ => "system",
    }
}

fn accent_index(code: &str) -> i32 {
    match code {
        "purple" => 1,
        "pink" => 2,
        "red" => 3,
        "orange" => 4,
        "green" => 5,
        "graphite" => 6,
        _ => 0,
    }
}

fn accent_code(index: i32) -> &'static str {
    match index {
        1 => "purple",
        2 => "pink",
        3 => "red",
        4 => "orange",
        5 => "green",
        6 => "graphite",
        _ => "blue",
    }
}

fn apply_theme(window: &MainWindow, cfg: &AppConfig) {
    let appearance = appearance_index(&cfg.appearance);
    let accent = accent_index(&cfg.accent);
    window.set_appearance_index(appearance);
    window.set_accent_index(accent);
    let theme = window.global::<Theme>();
    theme.set_appearance(appearance);
    theme.set_accent_index(accent);
}

fn apply_language(window: &MainWindow, language: Language) {
    let tr = Translations::load(language);
    let l = window.global::<L>();
    macro_rules! set_l {
        ($($setter:ident = $key:literal),* $(,)?) => {
            $( l.$setter(tr.get($key).into()); )*
        };
    }
    set_l!(
        set_app_name = "app_name",
        set_app_tagline = "app_tagline",
        set_nav_devices = "nav_devices",
        set_connect_device = "connect_device",
        set_settings = "settings",
        set_refresh = "refresh",
        set_close = "close",
        set_cancel = "cancel",
        set_save = "save",
        set_delete = "delete",
        set_add = "add",
        set_remove = "remove",
        set_done = "done",
        set_confirm = "confirm",
        set_retry = "retry",
        set_copy = "copy",
        set_copied = "copied",
        set_open_folder = "open_folder",
        set_empty_devices_title = "empty_devices_title",
        set_empty_devices_desc = "empty_devices_desc",
        set_device_online = "device_online",
        set_device_offline = "device_offline",
        set_device_cloud = "device_cloud",
        set_device_lan = "device_lan",
        set_shared_printers = "shared_printers",
        set_no_printers = "no_printers",
        set_printer_online = "printer_online",
        set_printer_offline = "printer_offline",
        set_added = "added",
        set_add_printer = "add_printer",
        set_remove_printer = "remove_printer",
        set_scan = "scan",
        set_scan_now = "scan_now",
        set_scanning = "scanning",
        set_print_test = "print_test",
        set_connect_title = "connect_title",
        set_connect_scan_tab = "connect_scan_tab",
        set_connect_manual_tab = "connect_manual_tab",
        set_scan_lan = "scan_lan",
        set_scanning_lan = "scanning_lan",
        set_scan_empty = "scan_empty",
        set_manual_input = "manual_input",
        set_host_name_or_ip = "host_name_or_ip",
        set_host_placeholder = "host_placeholder",
        set_access_code = "access_code",
        set_access_code_placeholder = "access_code_placeholder",
        set_test_connection = "test_connection",
        set_testing = "testing",
        set_save_device = "save_device",
        set_connection_ok = "connection_ok",
        set_connection_failed = "connection_failed",
        set_wrong_password = "wrong_password",
        set_host_required = "host_required",
        set_scanned_devices = "scanned_devices",
        set_connect = "connect",
        set_device_already_added = "device_already_added",
        set_settings_title = "settings_title",
        set_general = "general",
        set_language = "language",
        set_autostart = "autostart",
        set_autostart_desc = "autostart_desc",
        set_host_sharing = "host_sharing",
        set_host_password = "host_password",
        set_host_password_desc = "host_password_desc",
        set_set_password = "set_password",
        set_change_password = "change_password",
        set_password_saved = "password_saved",
        set_share_printers = "share_printers",
        set_share_printers_desc = "share_printers_desc",
        set_no_local_printers = "no_local_printers",
        set_share = "share",
        set_shared = "shared",
        set_printer_password = "printer_password",
        set_printer_password_desc = "printer_password_desc",
        set_scan_support = "scan_support",
        set_preferences = "preferences",
        set_cloud_title = "cloud_title",
        set_cloud_enable = "cloud_enable",
        set_cloud_enable_desc = "cloud_enable_desc",
        set_server_url = "server_url",
        set_server_code = "server_code",
        set_share_code = "share_code",
        set_share_code_desc = "share_code_desc",
        set_cloud_connect = "cloud_connect",
        set_cloud_connected = "cloud_connected",
        set_cloud_disconnected = "cloud_disconnected",
        set_cloud_save = "cloud_save",
        set_cloud_hint = "cloud_hint",
        set_about = "about",
        set_version = "version",
        set_open_logs = "open_logs",
        set_quit = "quit",
        set_minimize_to_tray = "minimize_to_tray",
        set_tray_open = "tray_open",
        set_tray_quit = "tray_quit",
        set_printer_added = "printer_added",
        set_printer_add_failed = "printer_add_failed",
        set_printer_removed = "printer_removed",
        set_device_removed = "device_removed",
        set_scan_saved = "scan_saved",
        set_scan_failed = "scan_failed",
        set_password_required = "password_required",
        set_token_expired = "token_expired",
        set_cloud_registered = "cloud_registered",
        set_status_ready = "status_ready",
        set_status_scanning = "status_scanning",
        set_status_starting = "status_starting",
        set_status_no_host_password = "status_no_host_password",
        set_status_host_running = "status_host_running",
        set_status_error = "status_error",
        set_scan_title = "scan_title",
        set_scan_resolution = "scan_resolution",
        set_scan_color = "scan_color",
        set_scan_gray = "scan_gray",
        set_scan_bw = "scan_bw",
        set_scan_format = "scan_format",
        set_scan_source = "scan_source",
        set_flatbed = "flatbed",
        set_adf = "adf",
        set_start_scan = "start_scan",
        set_password_dialog_title = "password_dialog_title",
        set_printer_access_code = "printer_access_code",
        set_required_for_printer = "required_for_printer",
        set_test_print = "test_print",
        set_test_print_sent = "test_print_sent",
        set_first_run_title = "first_run_title",
        set_first_run_desc = "first_run_desc",
        set_first_run_set = "first_run_set",
        set_skip = "skip",
        set_cloud_client_title = "cloud_client_title",
        set_cloud_connect_desc = "cloud_connect_desc",
        set_connect_cloud = "connect_cloud",
        set_device_address = "device_address",
        set_device_os = "device_os",
        set_device_printers = "device_printers",
        set_settings_saved = "settings_saved",
        set_error_generic = "error_generic",
        set_help = "help",
        set_docs = "docs",
        set_printer_driver = "printer_driver",
        set_printer_location = "printer_location",
        set_printer_status = "printer_status",
        set_welcome = "welcome",
        set_tray_tooltip = "tray_tooltip",
        set_copy_code = "copy_code",
        set_cloud_offline = "cloud_offline",
        set_share_code_placeholder = "share_code_placeholder",
        set_bridge_url_placeholder = "bridge_url_placeholder",
        set_server_code_placeholder = "server_code_placeholder",
        set_no_scanners = "no_scanners",
        set_save_scan = "save_scan",
        set_scan_in_progress = "scan_in_progress",
        set_show_window = "show_window",
        set_connection_lost = "connection_lost",
        set_reconnect = "reconnect",
        set_cloud_client = "cloud_client",
        set_cloud_client_toggle = "cloud_client_toggle",
        set_appear_online = "appear_online",
        set_preferences_hint = "preferences_hint",
        set_no_jobs = "no_jobs",
        set_jobs = "jobs",
        set_size = "size",
        set_appearance = "appearance",
        set_appearance_system = "appearance_system",
        set_appearance_light = "appearance_light",
        set_appearance_dark = "appearance_dark",
        set_appearance_desc = "appearance_desc",
        set_accent_color = "accent_color",
        set_accent_blue = "accent_blue",
        set_accent_purple = "accent_purple",
        set_accent_pink = "accent_pink",
        set_accent_red = "accent_red",
        set_accent_orange = "accent_orange",
        set_accent_green = "accent_green",
        set_accent_graphite = "accent_graphite",
        set_expand = "expand",
        set_collapse = "collapse",
        set_list_all_printers = "list_all_printers",
        set_disconnect_all = "disconnect_all",
        set_remove_host = "remove_host",
        set_all_printers_title = "all_printers_title",
        set_no_connected_printers = "no_connected_printers",
    );
}
