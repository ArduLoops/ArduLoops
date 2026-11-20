//! ArduPilot SITL built as WebAssembly (`public/sitl`, `scripts/build-sitl-wasm.sh`).
//!
//! A local HTTP server on 127.0.0.1 serves the SITL page with cross-origin isolation headers
//! (the WASM build needs SharedArrayBuffer). A WebView loads the page, and its worker sends
//! the SITL serial bytes back over a WebSocket. This module then makes them look like native
//! SITL: SERIAL0 on TCP 127.0.0.1:5770 (this app), SERIAL1 on TCP 5760 (another ground station),
//! SERIAL2 to UDP 127.0.0.1:14550.
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tungstenite::handshake::server::{Request, Response};
use tungstenite::{Message, WebSocket};

const HOST_HTML: &str = include_str!("host.html");
const WORKER_JS: &str = include_str!("worker.js");

/// Firmware files the page may load. Anything else is 404.
const FILES: &[&str] = &["arducopter.js", "arducopter.wasm", "arduplane.js", "arduplane.wasm"];

const SERIAL_APP: u8 = 0;
const SERIAL_GCS: u8 = 1;
const SERIAL_UDP: u8 = 2;
/// A WebSocket frame with this first byte carries the firmware's whole `eeprom.bin`.
const EEPROM_FRAME: u8 = 0xFF;

const APP_ADDR: &str = "127.0.0.1:5770";
const GCS_ADDR: &str = "0.0.0.0:5760";
const UDP_PEER: &str = "127.0.0.1:14550";

/// Time for the WebView to load the page and the firmware to send its first bytes.
pub const START_TIMEOUT: Duration = Duration::from_secs(30);

/// Platforms with a native SITL build (downloaded, or in the macOS bundle).
pub const NATIVE: bool = cfg!(any(windows, target_os = "macos", all(target_os = "linux", target_arch = "x86_64")));

/// The SITL builds this app can start, the default first.
pub fn engines() -> Vec<String> {
    let wasm_first = match std::env::var("ARDULOOPS_SITL").ok().as_deref() {
        Some("wasm") => true,
        Some("native") => false,
        _ => !NATIVE,
    };
    match (NATIVE, wasm_first) {
        (false, _) => vec!["wasm".into()],
        (true, true) => vec!["wasm".into(), "native".into()],
        (true, false) => vec!["native".into(), "wasm".into()],
    }
}

/// Use the WASM build for this start. `engine` is the UI choice ("native", "wasm" or empty for the default).
pub fn wanted(engine: &str) -> bool {
    let engines = engines();
    match engine {
        "wasm" => true,
        "native" => !engines.iter().any(|e| e == "native"),
        _ => engines.first().map(String::as_str) == Some("wasm"),
    }
}

/// The page arguments: firmware file stem and SITL command line.
pub struct Launch {
    pub binary: &'static str,
    pub args: Vec<String>,
    /// Where the firmware's `eeprom.bin` (parameters) is kept between starts.
    pub eeprom: PathBuf,
}

#[derive(Default)]
struct State {
    /// The worker connected and the firmware sent bytes.
    up: bool,
    /// The page or the worker reported this instead of starting.
    error: Option<String>,
    /// The WebSocket closed after `up`.
    ended: bool,
}

/// One running WASM SITL. Dropping it does not stop it: call `stop`.
pub struct WasmRun {
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<State>>,
    pub url: String,
}

impl WasmRun {
    /// Bind the ports and the page server. The caller opens `url` in a WebView.
    pub fn start(launch: Launch) -> Result<Self, String> {
        let app = TcpListener::bind(APP_ADDR).map_err(|e| format!("TCP {APP_ADDR}: {e}"))?;
        let gcs = TcpListener::bind(GCS_ADDR).map_err(|e| format!("TCP {GCS_ADDR}: {e}"))?;
        let udp = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("UDP: {e}"))?;
        let http = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("SITL page server: {e}"))?;
        let port = http.local_addr().map_err(|e| e.to_string())?.port();

        let stop = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(State::default()));
        let relay = Relay::new(udp, launch.eeprom.clone())?;

        relay.listen(SERIAL_APP, app, &stop);
        relay.listen(SERIAL_GCS, gcs, &stop);
        relay.udp_reader(&stop);

        let query = page_query(&launch);
        let url = format!("http://127.0.0.1:{port}/sitl/host.html?{query}");
        {
            let stop = stop.clone();
            let state = state.clone();
            let relay = relay.clone();
            std::thread::spawn(move || serve(http, stop, state, relay));
        }
        log::info!("sitl wasm {} {} at {url}", launch.binary, launch.args.join(" "));
        Ok(Self { stop, state, url })
    }

    /// True once the firmware sent bytes; the page's error if it could not start.
    pub fn status(&self) -> Result<bool, String> {
        let s = self.state.lock().map_err(|_| "sitl state".to_string())?;
        match &s.error {
            Some(err) => Err(err.clone()),
            None => Ok(s.up),
        }
    }

    /// The WebSocket closed: the window or the worker is gone.
    pub fn ended(&self) -> bool {
        self.state.lock().map(|s| s.ended || s.error.is_some()).unwrap_or(true)
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for WasmRun {
    fn drop(&mut self) {
        self.stop();
    }
}

fn page_query(launch: &Launch) -> String {
    let mut q = format!("binary={}", launch.binary);
    for a in &launch.args {
        q.push_str("&arg=");
        q.push_str(&percent(a));
    }
    q
}

fn percent(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Bytes between the TCP/UDP side and the WebSocket. Frames on the WebSocket are
/// `[serial number][bytes]` in both directions.
#[derive(Clone)]
struct Relay {
    /// One client per TCP port, as native SITL. A new client replaces the old one.
    clients: Arc<Mutex<[Option<TcpStream>; 2]>>,
    udp: Arc<UdpSocket>,
    to_ws: Sender<Vec<u8>>,
    from_ports: Arc<Mutex<Receiver<Vec<u8>>>>,
    eeprom: Arc<PathBuf>,
}

impl Relay {
    fn new(udp: UdpSocket, eeprom: PathBuf) -> Result<Self, String> {
        udp.set_read_timeout(Some(Duration::from_millis(200))).map_err(|e| e.to_string())?;
        let (to_ws, rx) = mpsc::channel();
        Ok(Self {
            clients: Arc::new(Mutex::new([None, None])),
            udp: Arc::new(udp),
            to_ws,
            from_ports: Arc::new(Mutex::new(rx)),
            eeprom: Arc::new(eeprom),
        })
    }

    fn listen(&self, serial: u8, listener: TcpListener, stop: &Arc<AtomicBool>) {
        let relay = self.clone();
        let stop = stop.clone();
        std::thread::spawn(move || {
            let _ = listener.set_nonblocking(true);
            while !stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, peer)) => relay.attach(serial, stream, peer, &stop),
                    Err(e) if e.kind() == ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
                    Err(_) => std::thread::sleep(Duration::from_millis(200)),
                }
            }
            if let Ok(mut c) = relay.clients.lock() {
                for s in c.iter_mut().filter_map(Option::take) {
                    let _ = s.shutdown(std::net::Shutdown::Both);
                }
            }
        });
    }

    fn attach(&self, serial: u8, stream: TcpStream, peer: SocketAddr, stop: &Arc<AtomicBool>) {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_nodelay(true);
        let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
        let Ok(reader) = stream.try_clone() else { return };
        if let Ok(mut c) = self.clients.lock() {
            if let Some(old) = c[serial as usize].replace(stream) {
                let _ = old.shutdown(std::net::Shutdown::Both);
            }
        }
        log::info!("sitl wasm serial{serial} client {peer}");
        let tx = self.to_ws.clone();
        let stop = stop.clone();
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut buf = [0u8; 4096];
            while !stop.load(Ordering::SeqCst) {
                match reader.read(&mut buf) {
                    Ok(0) => return,
                    Ok(n) => {
                        let mut frame = Vec::with_capacity(n + 1);
                        frame.push(serial);
                        frame.extend_from_slice(&buf[..n]);
                        if tx.send(frame).is_err() {
                            return;
                        }
                    }
                    Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                    Err(_) => return,
                }
            }
        });
    }

    fn udp_reader(&self, stop: &Arc<AtomicBool>) {
        let udp = self.udp.clone();
        let tx = self.to_ws.clone();
        let stop = stop.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while !stop.load(Ordering::SeqCst) {
                if let Ok((n, _)) = udp.recv_from(&mut buf) {
                    let mut frame = Vec::with_capacity(n + 1);
                    frame.push(SERIAL_UDP);
                    frame.extend_from_slice(&buf[..n]);
                    if tx.send(frame).is_err() {
                        return;
                    }
                }
            }
        });
    }

    /// Bytes the firmware wrote on one serial port.
    fn deliver(&self, frame: &[u8]) {
        let Some((&serial, bytes)) = frame.split_first() else { return };
        if bytes.is_empty() {
            return;
        }
        if serial == SERIAL_UDP {
            let _ = self.udp.send_to(bytes, UDP_PEER);
            return;
        }
        let Ok(mut c) = self.clients.lock() else { return };
        let Some(slot) = c.get_mut(serial as usize) else { return };
        if let Some(s) = slot.as_mut() {
            if s.write_all(bytes).is_err() {
                *slot = None;
            }
        }
    }

    /// Drop what the TCP clients sent while no worker was connected.
    fn drain(&self) {
        if let Ok(rx) = self.from_ports.lock() {
            while rx.try_recv().is_ok() {}
        }
    }
}

fn serve(http: TcpListener, stop: Arc<AtomicBool>, state: Arc<Mutex<State>>, relay: Relay) {
    let _ = http.set_nonblocking(true);
    while !stop.load(Ordering::SeqCst) {
        match http.accept() {
            Ok((stream, _)) => {
                let stop = stop.clone();
                let state = state.clone();
                let relay = relay.clone();
                std::thread::spawn(move || {
                    if let Err(e) = handle(stream, &stop, &state, &relay) {
                        log::debug!("sitl wasm http: {e}");
                    }
                });
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

/// Read the request line without taking it from the stream, so the WebSocket handshake can read it again.
fn peek_request_line(stream: &TcpStream) -> std::io::Result<String> {
    let mut buf = [0u8; 2048];
    for _ in 0..100 {
        let n = stream.peek(&mut buf)?;
        if let Some(end) = buf[..n].windows(2).position(|w| w == b"\r\n") {
            return Ok(String::from_utf8_lossy(&buf[..end]).into_owned());
        }
        if n == buf.len() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err(std::io::Error::new(ErrorKind::InvalidData, "no request line"))
}

fn handle(stream: TcpStream, stop: &AtomicBool, state: &Mutex<State>, relay: &Relay) -> std::io::Result<()> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let line = peek_request_line(&stream)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    let path = target.split('?').next().unwrap_or("/");

    if path == "/sitl/ws" {
        return websocket(stream, stop, state, relay);
    }

    // Drop the rest of the request head.
    let mut stream = stream;
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") && head.len() < 16 * 1024 {
        if stream.read(&mut byte)? == 0 {
            break;
        }
        head.push(byte[0]);
    }

    if method != "GET" {
        return respond(&mut stream, "405 Method Not Allowed", "text/plain", b"GET only");
    }
    match path {
        "/sitl/host.html" => respond(&mut stream, "200 OK", "text/html; charset=utf-8", HOST_HTML.as_bytes()),
        "/sitl/worker.js" => respond(&mut stream, "200 OK", "text/javascript", WORKER_JS.as_bytes()),
        "/sitl/eeprom" => match std::fs::read(relay.eeprom.as_ref()) {
            Ok(body) => respond(&mut stream, "200 OK", "application/octet-stream", &body),
            Err(_) => respond(&mut stream, "404 Not Found", "text/plain", b"no saved parameters"),
        },
        _ => {
            let name = path.strip_prefix("/sitl/").unwrap_or("");
            match FILES.contains(&name).then(|| firmware_file(name)).flatten() {
                Some(body) => {
                    let mime = if name.ends_with(".wasm") { "application/wasm" } else { "text/javascript" };
                    respond(&mut stream, "200 OK", mime, &body)
                }
                None => respond(&mut stream, "404 Not Found", "text/plain", b"not found"),
            }
        }
    }
}

/// Same-origin only, cross-origin isolated (SharedArrayBuffer for the WASM threads).
const ISOLATION: &str = "Cross-Origin-Opener-Policy: same-origin\r\nCross-Origin-Embedder-Policy: require-corp\r\nCross-Origin-Resource-Policy: same-origin\r\n";

fn respond(stream: &mut TcpStream, status: &str, mime: &str, body: &[u8]) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n{ISOLATION}Connection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn websocket(stream: TcpStream, stop: &AtomicBool, state: &Mutex<State>, relay: &Relay) -> std::io::Result<()> {
    let only_local = |req: &Request, resp: Response| {
        // The page is served from this origin. Refuse other web pages.
        let origin = req.headers().get("origin").and_then(|v| v.to_str().ok()).unwrap_or("");
        if origin.is_empty() || origin.starts_with("http://127.0.0.1:") {
            Ok(resp)
        } else {
            let mut deny = tungstenite::handshake::server::ErrorResponse::new(Some("origin".into()));
            *deny.status_mut() = tungstenite::http::StatusCode::FORBIDDEN;
            Err(deny)
        }
    };
    let mut ws = tungstenite::accept_hdr(stream, only_local).map_err(|e| std::io::Error::other(e.to_string()))?;
    ws.get_ref().set_read_timeout(Some(Duration::from_millis(5)))?;
    relay.drain();
    let result = pump(&mut ws, stop, state, relay);
    let _ = ws.close(None);
    let _ = ws.flush();
    if let Ok(mut s) = state.lock() {
        if s.up {
            s.ended = true;
        }
    }
    result
}

fn pump(ws: &mut WebSocket<TcpStream>, stop: &AtomicBool, state: &Mutex<State>, relay: &Relay) -> std::io::Result<()> {
    let err = |e: tungstenite::Error| std::io::Error::other(e.to_string());
    let rx = relay.from_ports.lock().map_err(|_| std::io::Error::other("relay"))?;
    while !stop.load(Ordering::SeqCst) {
        match ws.read() {
            Ok(Message::Binary(frame)) => {
                if let Ok(mut s) = state.lock() {
                    s.up = true;
                }
                if frame.first() == Some(&EEPROM_FRAME) {
                    save_eeprom(&relay.eeprom, &frame[1..]);
                } else {
                    relay.deliver(&frame);
                }
            }
            Ok(Message::Text(text)) => {
                // The page reports why it could not start.
                let text = text.to_string();
                if let Some(msg) = text.strip_prefix("error:") {
                    log::warn!("sitl wasm page: {msg}");
                    if let Ok(mut s) = state.lock() {
                        s.error = Some(msg.trim().to_string());
                    }
                    return Ok(());
                }
                log::info!("sitl wasm page: {text}");
            }
            Ok(Message::Close(_)) => return Ok(()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => return Err(err(e)),
        }
        let mut wrote = false;
        while let Ok(frame) = rx.try_recv() {
            ws.write(Message::Binary(frame.into())).map_err(err)?;
            wrote = true;
        }
        if wrote {
            ws.flush().map_err(err)?;
        }
    }
    Ok(())
}

/// Write the snapshot next to the old one, then swap, so a crash leaves a whole file.
fn save_eeprom(path: &std::path::Path, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("bin.tmp");
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// The firmware from the app's own assets, or from `public/sitl` on disk (dev builds).
fn firmware_file(name: &str) -> Option<Vec<u8>> {
    #[cfg(feature = "desktop")]
    if let Some(app) = crate::app_handle() {
        if let Some(asset) = app.asset_resolver().get(format!("sitl/{name}")) {
            return Some(asset.bytes().to_vec());
        }
    }
    sitl_dirs().into_iter().map(|d| d.join(name)).find(|p| p.is_file()).and_then(|p| std::fs::read(p).ok())
}

fn sitl_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = std::env::var_os("ARDULOOPS_SITL_WASM_DIR") {
        dirs.push(PathBuf::from(dir));
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dirs.push(manifest.join("../public/sitl"));
    dirs.push(manifest.join("../dist/sitl"));
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join("public/sitl"));
        dirs.push(cwd.join("../public/sitl"));
    }
    dirs
}

/// Open the SITL page in a hidden window of this app. The headless bridge (`npm run dev`)
/// has no window, so it opens the page in the system browser instead.
pub fn open_window(url: &str) -> Result<(), String> {
    #[cfg(all(feature = "desktop", not(any(target_os = "android", target_os = "ios"))))]
    if let Some(app) = crate::app_handle() {
        return open_app_window(&app, url);
    }
    open_in_browser(url)
}

fn open_in_browser(url: &str) -> Result<(), String> {
    // `ARDULOOPS_SITL_OPEN=none`: only log the page address (tests open it themselves).
    if std::env::var("ARDULOOPS_SITL_OPEN").as_deref() == Ok("none") {
        log::info!("sitl wasm page (not opened): {url}");
        return Ok(());
    }
    #[cfg(windows)]
    let status = std::process::Command::new("cmd").args(["/C", "start", "", url]).status();
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("open").arg(url).status();
    #[cfg(all(unix, not(target_os = "macos")))]
    let status = std::process::Command::new("xdg-open").arg(url).status();
    match status {
        Ok(s) if s.success() => Ok(()),
        _ => Err(format!("open {url} in Chrome, Edge or Firefox to run WASM SITL")),
    }
}

#[cfg(all(feature = "desktop", not(any(target_os = "android", target_os = "ios"))))]
fn open_app_window(app: &tauri::AppHandle, url: &str) -> Result<(), String> {
    use tauri::Manager;
    if let Some(old) = app.get_webview_window(WINDOW) {
        let _ = old.destroy();
    }
    let parsed: tauri::Url = url.parse().map_err(|e| format!("{e}"))?;
    tauri::WebviewWindowBuilder::new(app, WINDOW, tauri::WebviewUrl::External(parsed))
        .title("ArduLoops SITL")
        .visible(std::env::var_os("ARDULOOPS_SITL_SHOW").is_some())
        .inner_size(480.0, 320.0)
        .build()
        .map(|_| ())
        .map_err(|e| format!("SITL window: {e}"))
}

#[cfg(all(feature = "desktop", not(any(target_os = "android", target_os = "ios"))))]
pub fn close_window() {
    use tauri::Manager;
    if let Some(win) = crate::app_handle().and_then(|app| app.get_webview_window(WINDOW)) {
        let _ = win.destroy();
    }
}

#[cfg(not(all(feature = "desktop", not(any(target_os = "android", target_os = "ios")))))]
pub fn close_window() {}

#[cfg(all(feature = "desktop", not(any(target_os = "android", target_os = "ios"))))]
const WINDOW: &str = "sitl";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_escapes_args() {
        let q = page_query(&Launch {
            binary: "arducopter",
            args: vec!["-M+".into(), "-O-35.3,149.1,584,353".into()],
            eeprom: PathBuf::new(),
        });
        assert_eq!(q, "binary=arducopter&arg=-M%2B&arg=-O-35.3%2C149.1%2C584%2C353");
    }

    #[test]
    fn only_firmware_files() {
        assert!(FILES.contains(&"arducopter.wasm"));
        assert!(!FILES.contains(&"../Cargo.toml"));
    }
}
