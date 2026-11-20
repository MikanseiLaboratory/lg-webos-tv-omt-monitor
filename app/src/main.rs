//! OMT (Open Media Transport) monitor for LG webOS TVs.
//!
//! Receives an OMT video stream, decodes VMX1 in software (NEON on armv7) and
//! presents BGRA frames through EGL/GLES2 on the TV's own wayland compositor.
//!
//! Decoding and presentation run on separate threads so the ~14 ms decode and
//! the ~12 ms texture upload/vsync overlap instead of serialising.

use std::env;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use openmediatransport::{Discovery, FrameType, ReceiverConfig, ReceiverSession};

static LOG_PATH: OnceLock<String> = OnceLock::new();

fn log_path() -> &'static str {
    LOG_PATH.get_or_init(|| {
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

unsafe extern "C" {
    fn presenter_init(w: i32, h: i32) -> i32;
    fn presenter_frame(pixels: *const u8, w: i32, h: i32) -> i32;
    fn presenter_roundtrip() -> i32;
    fn presenter_shutdown();
    fn presenter_set_log(cb: extern "C" fn(*const core::ffi::c_char));
}

extern "C" fn c_log(msg: *const core::ffi::c_char) {
    if !msg.is_null() {
        let s = unsafe { core::ffi::CStr::from_ptr(msg) }.to_string_lossy();
        info!("[egl] {s}");
    }
}

struct Shared {
    latest: Mutex<Option<(std::sync::Arc<[u8]>, i32, i32)>>,
    cv: Condvar,
    quit: AtomicBool,
    decode_us: AtomicU64,
    decode_frames: AtomicU64,
}

impl Shared {
    fn new() -> Self {
        Shared {
            latest: Mutex::new(None),
            cv: Condvar::new(),
            quit: AtomicBool::new(false),
            decode_us: AtomicU64::new(0),
            decode_frames: AtomicU64::new(0),
        }
    }

    fn publish(&self, pixels: std::sync::Arc<[u8]>, w: i32, h: i32) {
        let mut slot = self.latest.lock().unwrap();
        *slot = Some((pixels, w, h)); // newest wins, older frame dropped
        self.cv.notify_one();
    }
}

struct Args {
    url: Option<String>,
    list_only: bool,
    width: i32,
    height: i32,
}

impl Args {
    fn parse() -> Self {
        let mut a = Args {
            url: env::var("OMT_SOURCE").ok(),
            list_only: false,
            width: 1920,
            height: 1080,
        };
        let mut it = env::args().skip(1);
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--connect" | "-c" => a.url = it.next(),
                "--list" | "-l" => a.list_only = true,
                "--width" => a.width = it.next().and_then(|v| v.parse().ok()).unwrap_or(1920),
                "--height" => a.height = it.next().and_then(|v| v.parse().ok()).unwrap_or(1080),
                _ => {}
            }
        }
        a
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

fn cpu_seconds() -> f64 {
    let Ok(stat) = std::fs::read_to_string("/proc/self/stat") else {
        return 0.0;
    };
    let Some(rest) = stat.rsplit_once(')').map(|(_, r)| r) else {
        return 0.0;
    };
    let f: Vec<&str> = rest.split_whitespace().collect();
    let utime: f64 = f.get(11).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let stime: f64 = f.get(12).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    (utime + stime) / 100.0
}

fn main() {
    let args = Args::parse();
    info!("omt-monitor starting");
    let nthreads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    vmx::use_shared_thread_pool(Some(nthreads));
    info!("vmx thread pool: {nthreads} threads");
    let caps = vmx::SimdCapabilities::detect();
    info!("vmx caps: neon={} ssse3={} avx2={}", caps.neon, caps.ssse3, caps.avx2);

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

    let shared = Arc::new(Shared::new());

    // ---- presenter thread (owns the EGL context) ----
    let pres = Arc::clone(&shared);
    let (pw, ph) = (args.width, args.height);
    let presenter = std::thread::spawn(move || {
        unsafe { presenter_set_log(c_log) };
        let rc = unsafe { presenter_init(pw, ph) };
        if rc != 0 {
            info!("EGL presenter init failed: {rc}");
            pres.quit.store(true, Ordering::SeqCst);
            pres.cv.notify_all();
            return;
        }
        info!("EGL presenter ready");

        let mut frames: u64 = 0;
        let mut window = Instant::now();
        let mut cpu_start = cpu_seconds();
        let mut decode_us_window: u64 = 0;
        let mut decode_frames_window: u64 = 0;
        let mut present_us_window: u64 = 0;

        loop {
            let mut slot = pres.latest.lock().unwrap();
            while slot.is_none() && !pres.quit.load(Ordering::SeqCst) {
                let (g, _) = pres.cv.wait_timeout(slot, Duration::from_millis(200)).unwrap();
                slot = g;
            }
            if pres.quit.load(Ordering::SeqCst) {
                break;
            }
            let Some((px, w, h)) = slot.take() else {
                continue;
            };
            drop(slot);

            let t0 = Instant::now();
            let _ = unsafe { presenter_frame(px.as_ptr(), w, h) };
            present_us_window += t0.elapsed().as_micros() as u64;
            let _ = unsafe { presenter_roundtrip() };
            frames += 1;

            decode_us_window += pres.decode_us.swap(0, Ordering::Relaxed);
            decode_frames_window += pres.decode_frames.swap(0, Ordering::Relaxed);

            let secs = window.elapsed().as_secs_f64();
            if secs >= 2.0 {
                let n = frames.max(1) as f64;
                let dn = decode_frames_window.max(1) as f64;
                let cpu = (cpu_seconds() - cpu_start) / secs * 100.0;
                info!(
                    "fps={:.1} decode={:.2}ms present={:.2}ms cpu={:.0}% frames={}",
                    frames as f64 / secs,
                    decode_us_window as f64 / dn / 1000.0,
                    present_us_window as f64 / n / 1000.0,
                    cpu,
                    frames
                );
                frames = 0;
                decode_us_window = 0;
                decode_frames_window = 0;
                present_us_window = 0;
                window = Instant::now();
                cpu_start = cpu_seconds();
            }
        }
        unsafe { presenter_shutdown() };
        info!("presenter stopped");
    });

    // ---- receive thread (main) ----
    let mut running = true;
    while running && !shared.quit.load(Ordering::SeqCst) {
        let session = match connect(&url, &addresses) {
            Ok(s) => {
                info!("connected");
                s
            }
            Err(e) => {
                info!("{e}; retrying in 2s");
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
        };
        let mut last_frame_at = Instant::now();
        loop {
            if shared.quit.load(Ordering::SeqCst) {
                running = false;
                break;
            }
            let t0 = Instant::now();
            let Some(frame) = session.recv_video_timeout(Duration::from_millis(100)) else {
                if last_frame_at.elapsed() > Duration::from_secs(3) {
                    info!("no frames for 3s; reconnecting");
                    break;
                }
                continue;
            };
            shared
                .decode_us
                .fetch_add(t0.elapsed().as_micros() as u64, Ordering::Relaxed);
            shared.decode_frames.fetch_add(1, Ordering::Relaxed);

            shared.publish(frame.pixels, frame.width as i32, frame.height as i32);
            last_frame_at = Instant::now();
        }
        session.disconnect();
    }

    shared.quit.store(true, Ordering::SeqCst);
    shared.cv.notify_all();
    let _ = presenter.join();
    info!("bye");
}
