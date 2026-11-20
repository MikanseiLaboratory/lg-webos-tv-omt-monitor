//! OMT (Open Media Transport) monitor for LG webOS TVs.
//!
//! Receives an OMT video stream, decodes VMX1 in software and presents the BGRA
//! frames either through the bridge's EGL/GLES2 path (default) or SDL2.

#[cfg(feature = "sdl2")]
mod sdl;
#[cfg(not(feature = "sdl2"))]
#[path = "sdl_stub.rs"]
mod sdl;

use std::env;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use openmediatransport::{Discovery, FrameType, ReceiverConfig, ReceiverSession};

static LOG_PATH: OnceLock<String> = OnceLock::new();

fn log_path() -> &'static str {
    LOG_PATH.get_or_init(|| {
        // Apps run in a jail; log next to the executable so it can be read back.
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                return dir.join("run.log").to_string_lossy().into_owned();
            }
        }
        "/tmp/omt-monitor.log".to_string()
    })
}

fn log(msg: &str) {
    let line = format!("[{:?}] {msg}\n", std::time::SystemTime::now());
    print!("{line}");
    let _ = std::io::stdout().flush();
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(log_path()) {
        let _ = f.write_all(line.as_bytes());
    }
}

macro_rules! info {
    ($($arg:tt)*) => { crate::log(&format!($($arg)*)) };
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Presenter {
    Egl,
    Sdl2,
}

unsafe extern "C" {
    fn presenter_init(w: i32, h: i32) -> i32;
    fn presenter_frame(pixels: *const u8, w: i32, h: i32) -> i32;
    fn presenter_roundtrip() -> i32;
    fn presenter_set_log(cb: extern "C" fn(*const core::ffi::c_char));
    fn presenter_shutdown();
}

struct Args {
    url: Option<String>,
    list_only: bool,
    windowed: bool,
    width: i32,
    height: i32,
    presenter: Option<Presenter>,
}

impl Args {
    fn parse() -> Self {
        let mut a = Args {
            url: env::var("OMT_SOURCE").ok(),
            list_only: false,
            windowed: false,
            width: 1920,
            height: 1080,
            presenter: None,
        };
        let mut it = env::args().skip(1);
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--connect" | "-c" => a.url = it.next(),
                "--list" | "-l" => a.list_only = true,
                "--windowed" | "-w" => a.windowed = true,
                "--width" => a.width = it.next().and_then(|v| v.parse().ok()).unwrap_or(1920),
                "--height" => a.height = it.next().and_then(|v| v.parse().ok()).unwrap_or(1080),
                "--presenter" => {
                    a.presenter = match it.next().as_deref() {
                        Some("sdl2") => Some(Presenter::Sdl2),
                        Some("egl") => Some(Presenter::Egl),
                        _ => None,
                    }
                }
                "--help" | "-h" => {
                    eprintln!("omt-monitor [--connect URL] [--list] [--presenter egl|sdl2]");
                    std::process::exit(0);
                }
                _ => {}
            }
        }
        a
    }
}

fn presenter_from_file() -> Option<Presenter> {
    let exe = env::current_exe().ok()?;
    let text = std::fs::read_to_string(exe.parent()?.join("presenter.txt")).ok()?;
    match text.trim() {
        "sdl2" => Some(Presenter::Sdl2),
        "egl" => Some(Presenter::Egl),
        _ => None,
    }
}

fn source_from_file() -> Option<String> {
    let exe = env::current_exe().ok()?;
    let text = std::fs::read_to_string(exe.parent()?.join("source.txt")).ok()?;
    let url = text.trim().to_string();
    if url.is_empty() { None } else { Some(url) }
}

fn discover(timeout: Duration) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    match Discovery::new() {
        Ok(mut d) => {
            if let Err(e) = d.refresh_for(timeout) {
                info!("discovery refresh failed: {e}");
                return out;
            }
            for src in d.sources() {
                info!(
                    "source: {} port={} url={} addrs={:?}",
                    src.instance_name(),
                    src.port,
                    src.to_url(),
                    src.addresses
                );
                out.push((src.to_url(), src.addresses.clone()));
            }
        }
        Err(e) => info!("discovery unavailable: {e}"),
    }
    out
}

fn connect(url: &str, addresses: &[String]) -> Result<ReceiverSession, String> {
    let cfg = ReceiverConfig {
        frame_types: FrameType::VIDEO,
        connect_timeout: Duration::from_secs(5),
        ..ReceiverConfig::default()
    };
    ReceiverSession::connect_with_addresses(url, addresses, cfg)
        .map_err(|e| format!("connect {url} failed: {e}"))
}

/// CPU time consumed by this process so far (seconds).
fn cpu_seconds() -> f64 {
    let Ok(stat) = std::fs::read_to_string("/proc/self/stat") else {
        return 0.0;
    };
    let Some(rest) = stat.rsplit_once(')').map(|(_, r)| r) else {
        return 0.0;
    };
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // utime = field 14, stime = field 15 (1-based), i.e. index 11 and 12 here.
    let utime: f64 = fields.get(11).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let stime: f64 = fields.get(12).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    (utime + stime) / 100.0
}

struct Stats {
    window_start: Instant,
    frames: u64,
    decode_us: u64,
    present_us: u64,
    cpu_start: f64,
}

impl Stats {
    fn new() -> Self {
        Stats {
            window_start: Instant::now(),
            frames: 0,
            decode_us: 0,
            present_us: 0,
            cpu_start: cpu_seconds(),
        }
    }

    fn report(&mut self, total: u64, session: &ReceiverSession) {
        let secs = self.window_start.elapsed().as_secs_f64();
        if secs < 1.0 || self.frames == 0 {
            return;
        }
        let fps = self.frames as f64 / secs;
        let decode_ms = self.decode_us as f64 / self.frames as f64 / 1000.0;
        let present_ms = self.present_us as f64 / self.frames as f64 / 1000.0;
        let cpu = (cpu_seconds() - self.cpu_start) / secs * 100.0;
        let st = session.statistics();
        info!(
            "fps={fps:.1} decode={decode_ms:.2}ms present={present_ms:.2}ms cpu={cpu:.0}% total={total} \
             bytes={} drop_wire={} drop_decode={}",
            st.bytes_received, st.frames_dropped_wire, st.frames_dropped_decode
        );
        self.frames = 0;
        self.decode_us = 0;
        self.present_us = 0;
        self.window_start = Instant::now();
        self.cpu_start = cpu_seconds();
    }
}

fn main() {
    let args = Args::parse();
    info!("omt-monitor starting");

    let presenter = args
        .presenter
        .or_else(presenter_from_file)
        .unwrap_or(Presenter::Egl);
    info!("presenter = {}", match presenter {
        Presenter::Egl => "egl",
        Presenter::Sdl2 => "sdl2",
    });

    if args.list_only {
        discover(Duration::from_secs(3));
        return;
    }

    let mut target: Option<(String, Vec<String>)> = args
        .url
        .clone()
        .map(|u| (u, Vec::new()))
        .or_else(|| source_from_file().map(|u| (u, Vec::new())));

    if target.is_none() {
        info!("no source configured; discovering OMT sources for 3s");
        target = discover(Duration::from_secs(3)).into_iter().next();
    }

    let Some((url, addresses)) = target else {
        info!("no OMT source found");
        std::process::exit(2);
    };
    info!("target: {url}");

    // Environment the webOS compositor / bridge expect.
    for (k, v) in [
        ("XDG_RUNTIME_DIR", "/tmp/xdg"),
        ("EGL_PLATFORM", "wayland"),
        ("WAYLAND_DISPLAY", "wayland-0"),
        ("XKB_CONFIG_ROOT", "/usr/share/X11/xkb"),
    ] {
        if env::var_os(k).is_none() {
            unsafe { env::set_var(k, v) };
        }
    }

    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            let proxy = dir.join("gles_proxy");
            if proxy.exists() && env::var_os("GLES_PROXY_BIN").is_none() {
                unsafe { env::set_var("GLES_PROXY_BIN", &proxy) };
                info!("GLES_PROXY_BIN = {}", proxy.display());
            }
        }
    }

    let (mut sdl_window, mut sdl_renderer, mut sdl_texture) = (None, None, None);

    extern "C" fn c_log(msg: *const core::ffi::c_char) {
        if !msg.is_null() {
            let s = unsafe { core::ffi::CStr::from_ptr(msg) }.to_string_lossy();
            info!("[egl] {s}");
        }
    }

    match presenter {
        Presenter::Egl => {
            unsafe { presenter_set_log(c_log) };
            let rc = unsafe { presenter_init(args.width, args.height) };
            if rc != 0 {
                info!("EGL presenter init failed: {rc}");
                std::process::exit(3);
            }
            info!("EGL presenter ready");
        }
        Presenter::Sdl2 => {
            if let Err(e) = sdl::init() {
                info!("SDL init failed: {e}");
                std::process::exit(3);
            }
            let w = match sdl::create_window("OMT Monitor", args.width, args.height, !args.windowed) {
                Ok(w) => w,
                Err(e) => {
                    info!("SDL window failed: {e}");
                    std::process::exit(3);
                }
            };
            let r = match sdl::create_renderer(&w) {
                Ok(r) => r,
                Err(e) => {
                    info!("SDL renderer failed: {e}");
                    std::process::exit(3);
                }
            };
            sdl_window = Some(w);
            sdl_renderer = Some(r);
        }
    }

    let mut events = [0u8; sdl::SDL_EVENT_SIZE];
    let mut total: u64 = 0;
    let mut stats = Stats::new();
    let mut tex_size = (0i32, 0i32);
    let mut last_frame_at = Instant::now();
    let mut running = true;

    'outer: while running {
        let session = match connect(&url, &addresses) {
            Ok(s) => {
                info!("connected");
                s
            }
            Err(e) => {
                info!("{e}; retrying in 2s");
                std::thread::sleep(Duration::from_secs(2));
                continue 'outer;
            }
        };
        last_frame_at = Instant::now();

        while running {
            if presenter == Presenter::Sdl2 {
                while let Some((ty, sym)) = sdl::poll_event(&mut events) {
                    if ty == sdl::SDL_QUIT
                        || (ty == sdl::SDL_KEYDOWN
                            && (sym == sdl::SDLK_ESCAPE || sym == sdl::SDLK_q))
                    {
                        running = false;
                    }
                }
                if !running {
                    break;
                }
            }

            let t0 = Instant::now();
            let Some(frame) = session.recv_video_timeout(Duration::from_millis(100)) else {
                if last_frame_at.elapsed() > Duration::from_secs(3) {
                    info!("no frames for 3s; reconnecting");
                    break;
                }
                continue;
            };
            let decode_us = t0.elapsed().as_micros() as u64;

            let (w, h) = (frame.width as i32, frame.height as i32);
            let t1 = Instant::now();
            match presenter {
                Presenter::Egl => {
                    if tex_size != (w, h) {
                        info!("video {w}x{h}");
                        tex_size = (w, h);
                    }
                    let _ = unsafe { presenter_frame(frame.pixels.as_ptr(), w, h) };
                }
                Presenter::Sdl2 => {
                    if sdl_texture.is_none() || tex_size != (w, h) {
                        info!("video {w}x{h}");
                        sdl_texture = sdl::create_texture(sdl_renderer.as_ref().unwrap(), w, h).ok();
                        tex_size = (w, h);
                    }
                    if let (Some(tex), Some(ren), Some(win)) =
                        (sdl_texture.as_ref(), sdl_renderer.as_ref(), sdl_window.as_ref())
                    {
                        if sdl::update_texture(tex, &frame.pixels, w, h).is_ok() {
                            sdl::present(ren, tex, win);
                        }
                    }
                }
            }
            stats.present_us += t1.elapsed().as_micros() as u64;
            stats.decode_us += decode_us;
            stats.frames += 1;
            total += 1;
            last_frame_at = Instant::now();

            stats.report(total, &session);

            if presenter == Presenter::Egl {
                let _ = unsafe { presenter_roundtrip() };
            }
        }
        session.disconnect();
    }

    match presenter {
        Presenter::Egl => unsafe { presenter_shutdown() },
        Presenter::Sdl2 => {
            if let (Some(tex), Some(ren), Some(win)) =
                (sdl_texture.as_ref(), sdl_renderer.as_ref(), sdl_window.as_ref())
            {
                sdl::destroy(win, ren, tex);
            }
            sdl::quit();
        }
    }
    info!("bye (frames={total})");
}
