#![recursion_limit = "256"]

mod agent;
mod app;
mod board_doc;
mod board_sync;
mod cli;
mod cloud;
mod db;
mod dflog;
mod firmware;
mod firmware_native;
mod http;
mod link;
mod mcp;
mod migrate;
mod ports;
mod research;
#[cfg(not(target_os = "android"))]
mod serial_link;
mod sitl;
mod tlog;
mod udp;
mod update;

pub fn migrate_force(on: bool) {
    migrate::force_latest(on);
}

pub fn wants_mcp(args: &[String]) -> bool {
    matches!(args.first().map(String::as_str), Some("mcp" | "--mcp"))
}

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use crate::link::{Cmd, OnSample, Sample};

pub fn run_cli(args: &[String]) -> i32 {
    if args.first().map(String::as_str) == Some("--firmware-worker") {
        return args
            .get(1)
            .map(|id| {
                firmware_native::run_worker(id)
                    .map(|_| 0)
                    .unwrap_or_else(|e| {
                        eprintln!("{e}");
                        1
                    })
            })
            .unwrap_or(2);
    }
    cli::run(args)
}

fn spawn_backend(rx: std::sync::mpsc::Receiver<Cmd>, tx_http: Sender<Cmd>, on_sample: OnSample) {
    let latest = Arc::new(Mutex::new(Sample::empty()));
    let url = Arc::new(Mutex::new(String::new()));
    let sitl = sitl::SitlCtl::new();
    let latest_http = latest.clone();
    let sitl_loop = sitl.clone();
    std::thread::spawn(move || http::serve(&http::bridge_addr(), latest_http, tx_http));
    std::thread::spawn(move || link::run_loop(on_sample, rx, latest, url, sitl_loop));
    crate::app::kick_refresh();
    crate::board_sync::start();
}

pub fn run_bridge() {
    agent::note_stopped_turns();
    let (tx, rx) = mpsc::channel::<Cmd>();
    println!("ArduLoops  http://{}  (headless MAVLink for the browser UI)", http::bridge_addr());
    println!("UI         http://127.0.0.1:5173");
    spawn_backend(rx, tx, Arc::new(|_: &Sample| {}));
    loop {
        std::thread::park();
    }
}

#[cfg(feature = "desktop")]
mod desktop {
    use super::*;
    use std::sync::OnceLock;
    use tauri::Manager;
    use tauri_plugin_opener::OpenerExt;

    static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

    pub fn app_handle() -> Option<tauri::AppHandle> {
        APP.get().cloned()
    }

    pub fn open_system_url(url: &str) -> Result<(), String> {
        let app = APP.get().ok_or_else(|| "open the link in the browser".to_string())?;
        let (tx, rx) = mpsc::channel();
        let url = url.to_string();
        app.run_on_main_thread(move || {
            let result = match APP.get() {
                Some(app) => app
                    .opener()
                    .open_url(url, None::<&str>)
                    .map_err(|err| err.to_string()),
                None => Err("open the link in the browser".into()),
            };
            let _ = tx.send(result);
        })
        .map_err(|err| err.to_string())?;
        rx.recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| "open the link in the browser".to_string())?
    }

    fn show_main(app: &tauri::AppHandle) {
        if let Some(win) = app.get_webview_window("main") {
            // Mobile windows cannot be minimized and have no unminimize.
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            let _ = win.unminimize();
            let _ = win.show();
            let _ = win.set_focus();
        }
    }

    /// `arduloops://open?frame=…&ws=…` (src/lib/nav.ts). Other arduloops:// links (auth) are not places.
    fn app_link(raw: &str) -> Option<tauri::Url> {
        let url = tauri::Url::parse(raw).ok()?;
        let verb = url.host_str().unwrap_or_else(|| url.path().trim_matches('/'));
        (url.scheme() == "arduloops" && verb.trim_matches('/') == "open").then_some(url)
    }

    /// The link that launched the app, until the main window takes it (`take_app_link`).
    static LAUNCH_LINK: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

    /// The main window asks once its `app-link` listener is up, so a launch link is not lost before that.
    #[tauri::command]
    fn take_app_link() -> Option<String> {
        LAUNCH_LINK.lock().ok()?.take()
    }

    /// A link while the app runs: the main window moves there (src/lib/nav.ts listens for `app-link`).
    fn open_app_link(app: &tauri::AppHandle, raw: &str) {
        use tauri::Emitter;
        if app_link(raw).is_some() {
            let _ = app.emit_to("main", "app-link", raw.to_string());
            show_main(app);
        }
    }

    pub fn run() {
        let (tx, rx) = mpsc::channel::<Cmd>();
        let builder = tauri::Builder::default();
        // First plugin: a second launch focuses this window, and its deep link reaches `on_open_url` here.
        #[cfg(not(target_os = "android"))]
        let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)));
        let builder = builder
            .plugin(tauri_plugin_log::Builder::default().build())
            .plugin(tauri_plugin_opener::init())
            .plugin(tauri_plugin_deep_link::init());
        #[cfg(not(target_os = "android"))]
        let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
        #[cfg(not(target_os = "android"))]
        let builder = builder.plugin(tauri_plugin_window_state::Builder::default().build());
        builder
            .invoke_handler(tauri::generate_handler![take_app_link])
            .on_window_event(|window, event| {
                // Detached panels (lib/detach.ts) belong to the main window. Closing them from inside this event
                // swallows main's own close on Windows, so the places of all windows are saved and the app exits.
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    if window.label() == "main" && window.app_handle().webview_windows().len() > 1 {
                        api.prevent_close();
                        let app = window.app_handle().clone();
                        #[cfg(not(target_os = "android"))]
                        {
                            use tauri_plugin_window_state::{AppHandleExt, StateFlags};
                            let _ = app.save_window_state(StateFlags::all());
                        }
                        app.exit(0);
                    }
                }
            })
            .setup(move |app| {
                if let Ok(path) = app.path().app_local_data_dir() {
                    crate::db::set_app_data_dir(path);
                }
                crate::agent::note_stopped_turns();
                spawn_backend(rx, tx, Arc::new(|_: &Sample| {}));
                let _ = APP.set(app.handle().clone());
                {
                    use tauri_plugin_deep_link::DeepLinkExt;
                    // An installed build registers the scheme from tauri.conf.json; a dev build points it at itself.
                    #[cfg(all(debug_assertions, any(windows, target_os = "linux")))]
                    let _ = app.deep_link().register_all();
                    let handle = app.handle().clone();
                    let _ = app.deep_link().on_open_url(move |event| {
                        for item in event.urls() {
                            crate::agent::complete_auth_url(item.as_str());
                            open_app_link(&handle, item.as_str());
                        }
                    });
                    if let Ok(Some(urls)) = app.deep_link().get_current() {
                        for item in urls {
                            crate::agent::complete_auth_url(item.as_str());
                            if app_link(item.as_str()).is_some() {
                                if let Ok(mut slot) = LAUNCH_LINK.lock() {
                                    *slot = Some(item.to_string());
                                }
                            }
                        }
                    }
                }
                Ok(())
            })
            .run(tauri::generate_context!())
            .expect("error while running ArduLoops");
    }
}

#[cfg(feature = "desktop")]
pub fn open_system_url(url: &str) -> Result<(), String> {
    desktop::open_system_url(url)
}

#[cfg(feature = "desktop")]
pub(crate) fn app_handle() -> Option<tauri::AppHandle> {
    desktop::app_handle()
}

#[cfg(not(feature = "desktop"))]
pub fn open_system_url(_: &str) -> Result<(), String> {
    Err("open the link in the browser".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
#[cfg(feature = "desktop")]
pub fn run() {
    desktop::run();
}
