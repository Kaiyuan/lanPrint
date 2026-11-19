#![cfg_attr(windows, windows_subsystem = "windows")]

mod activity;
mod api;
mod app;
mod autostart;
mod client;
mod cloud;
mod cups;
mod error;
mod forwarder;
mod platform;
mod runtime;
mod testpage;
mod ui;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::sync::Arc;

use lanprint_core::{ConfigStore, INSTANCE_PORT};
use tracing::{error, info, warn};

fn main() {
    install_panic_hook();
    let args: Vec<String> = std::env::args().collect();
    let version = args.iter().any(|a| a == "--version" || a == "-v");
    if version {
        println!("lanPrint {}", lanprint_core::VERSION);
        return;
    }
    let is_cups = args.iter().any(|a| a == "--cups-backend") || args[0].contains("/cups/backend/");
    if is_cups {
        cups::run();
    }
    let headless = args.iter().any(|a| a == "--headless" || a == "--service");
    let tray_mode = args.iter().any(|a| a == "--tray");

    let config = match ConfigStore::load_or_create(None) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("cannot load configuration: {err}");
            std::process::exit(2);
        }
    };
    init_logging(&config);

    // Small automation helpers used by installers, tests and headless setups.
    if let Some(pos) = args.iter().position(|a| a == "--set-password") {
        if let Some(password) = args.get(pos + 1) {
            match config.edit(|cfg| cfg.set_host_password(password)) {
                Ok(()) => {
                    println!("host access code updated");
                    return;
                }
                Err(err) => {
                    eprintln!("cannot set access code: {err}");
                    std::process::exit(2);
                }
            }
        }
    }
    if let Some(pos) = args.iter().position(|a| a == "--share") {
        if let Some(printer) = args.get(pos + 1) {
            let result = config.edit(|cfg| cfg.upsert_shared_printer(printer, true, None, None));
            match result {
                Ok(()) => {
                    println!("sharing {printer}");
                    return;
                }
                Err(err) => {
                    eprintln!("cannot share printer: {err}");
                    std::process::exit(2);
                }
            }
        }
    }
    if let Some(pos) = args.iter().position(|a| a == "--unshare") {
        if let Some(printer) = args.get(pos + 1) {
            config.edit(|cfg| {
                if let Some(shared) = cfg.shared_printer_mut(printer) {
                    shared.enabled = false;
                }
            });
            println!("no longer sharing {printer}");
            return;
        }
    }
    if let Some(pos) = args.iter().position(|a| a == "--device-name") {
        if let Some(name) = args.get(pos + 1) {
            config.edit(|cfg| cfg.device_name = name.clone());
            println!("device name updated");
            return;
        }
    }
    if let Some(pos) = args.iter().position(|a| a == "--cloud") {
        if let (Some(server), Some(code)) = (args.get(pos + 1), args.get(pos + 2)) {
            config.edit(|cfg| {
                cfg.cloud.enabled = true;
                cfg.cloud.server_url = server.trim_end_matches('/').to_string();
                cfg.cloud.server_code = code.clone();
                cfg.cloud.host_token.clear();
                cfg.cloud.share_code.clear();
            });
            println!("cloud sharing enabled for {server}");
            return;
        }
    }
    if args.iter().any(|a| a == "--cloud-disable") {
        config.edit(|cfg| {
            cfg.cloud.enabled = false;
            cfg.cloud.connected = false;
            cfg.cloud.share_code.clear();
            cfg.cloud.host_token.clear();
        });
        println!("cloud sharing disabled");
        return;
    }

    // Single instance: a second launch just asks the running UI to come up.
    let instance = TcpListener::bind(("127.0.0.1", INSTANCE_PORT));
    if instance.is_err() {
        let api_port = config.read().api_port;
        for _ in 0..10 {
            if signal_existing(api_port) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        warn!("another instance seems to be running but did not answer");
        return;
    }
    let _instance = instance.ok();

    if config.read().autostart {
        autostart::apply_on_startup(true);
    }

    let (event_tx, event_rx) = mpsc::channel();
    let api_port = config.read().api_port;
    let app = match app::App::new(config, event_tx) {
        Ok(app) => app,
        Err(err) => {
            eprintln!("cannot initialize application: {err}");
            std::process::exit(2);
        }
    };

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("lanprint-io")
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("cannot start async runtime: {err}");
            std::process::exit(2);
        }
    };
    runtime.spawn({
        let app = app.clone();
        async move {
            if let Err(err) = api::serve(app, api_port).await {
                error!("API server stopped: {err}");
            }
        }
    });
    runtime.spawn({
        let app = app.clone();
        async move {
            app.initialize().await;
            info!("background services ready");
        }
    });
    runtime::install(runtime.handle().clone());

    if headless {
        info!("running headless; press Ctrl+C to stop");
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }

    // Autostart and tray mode stay in the tray; only an explicit launch opens the window.
    let show_window = !tray_mode;
    let open_settings = args.iter().any(|a| a == "--settings");
    let settings_tab = args
        .iter()
        .position(|a| a == "--settings-tab")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if let Err(err) = ui::run(
        app.clone(),
        event_rx,
        show_window,
        open_settings,
        settings_tab,
    ) {
        error!("UI error: {err}");
        eprintln!("UI error: {err}");
    }
    runtime.block_on(async {
        app.shutdown().await;
    });
}

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let dir = platform::log_dir();
        let _ = std::fs::create_dir_all(&dir);
        let message = format!(
            "{} panic: {info}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        );
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("panic.log"))
        {
            use std::io::Write;
            let _ = file.write_all(message.as_bytes());
        }
        eprintln!("{message}");
    }));
}

fn init_logging(config: &Arc<ConfigStore>) {
    let dir = platform::log_dir();
    let _ = std::fs::create_dir_all(&dir);
    let level = config.read().log_level.clone();
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("lanPrint.log"));
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| level.as_str().into());
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false);
    match file {
        Ok(file) => {
            let writer = move || file.try_clone().expect("clone log file");
            builder.with_writer(writer).with_ansi(false).init();
        }
        Err(_) => {
            builder.init();
        }
    }
}

fn signal_existing(api_port: u16) -> bool {
    let mut stream = match std::net::TcpStream::connect_timeout(
        &format!("127.0.0.1:{api_port}").parse().unwrap(),
        std::time::Duration::from_millis(800),
    ) {
        Ok(stream) => stream,
        Err(_) => return false,
    };
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(800)));
    let request = format!(
        "GET /api/v1/ui/open HTTP/1.1\r\nHost: 127.0.0.1:{api_port}\r\nConnection: close\r\n\r\n"
    );
    let _ = stream.write_all(request.as_bytes());
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response.contains("200") || response.contains("status")
}
