//! OMT (Open Media Transport) monitor for LG webOS TVs.
//!
//! Shows a picker with the OMT sources discovered on the LAN (DNS-SD), plays the
//! selected one with VMX1 software decoding (NEON on armv7) and EGL/GLES2
//! presentation, and keeps the TV awake while playing.

mod font;

use std::env;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use openmediatransport::{Discovery, FrameType, ReceiverConfig, ReceiverSession};

const MODE_PICKER: u8 = 0;
const MODE_PLAYING: u8 = 1;

const UI_W: usize = 1280;
const UI_H: usize = 720;
const ROW_H: usize = 52;
const LIST_TOP: usize = 180;

// Linux evdev key codes used by the webOS remote.
const KEY_UP: i32 = 103;
const KEY_DOWN: i32 = 108;
const KEY_ENTER: i32 = 28;
const KEY_BACK: i32 = 158;
const BTN_LEFT: i32 = 272;

static LOG_PATH: OnceLock<String> = OnceLock::new();

fn log_path() -> &'static str {
    LOG_PATH.get_or_init(|| {
        if let Ok(exe) = env::current_exe() {
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
    fn presenter_set_key_handler(cb: extern "C" fn(i32, i32));
    fn wakelock_start() -> i32;
    fn presenter_pointer_y() -> i32;
    fn input_set_logger(cb: extern "C" fn(*const core::ffi::c_char));
    fn input_probe() -> i32;
}

extern "C" fn c_log(msg: *const core::ffi::c_char) {
    if !msg.is_null() {
        let s = unsafe { core::ffi::CStr::from_ptr(msg) }.to_string_lossy();
        info!("[egl] {s}");
    }
}

static KEYQ: OnceLock<Mutex<Vec<(i32, bool)>>> = OnceLock::new();

extern "C" fn c_key(code: i32, pressed: i32) {
    KEYQ.get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .map(|mut q| q.push((code, pressed != 0)))
        .ok();
}

#[derive(Clone)]
struct Source {
    name: String,
    url: String,
    addrs: Vec<String>,
}

struct Shared {
    latest: Mutex<Option<(Arc<[u8]>, i32, i32)>>,
    cv: Condvar,
    quit: AtomicBool,
    mode: AtomicU8,
    sources: Mutex<Vec<Source>>,
    selected: Mutex<usize>,
    play: Mutex<Option<Source>>,
    decode_us: AtomicU64,
    decode_frames: AtomicU64,
    ptr_y: Mutex<i32>,
}

impl Shared {
    fn new() -> Self {
        Shared {
            latest: Mutex::new(None),
            cv: Condvar::new(),
            quit: AtomicBool::new(false),
            mode: AtomicU8::new(MODE_PICKER),
            sources: Mutex::new(Vec::new()),
            selected: Mutex::new(0),
            play: Mutex::new(None),
            decode_us: AtomicU64::new(0),
            decode_frames: AtomicU64::new(0),
            ptr_y: Mutex::new(0),
        }
    }

    fn publish(&self, pixels: Arc<[u8]>, w: i32, h: i32) {
        let mut slot = self.latest.lock().unwrap();
        *slot = Some((pixels, w, h));
        self.cv.notify_one();
    }
}

struct Args {
    url: Option<String>,
    list_only: bool,
    /// Index into the discovered source list (from launch params or --index).
    index: Option<usize>,
}

impl Args {
    fn parse() -> Self {
        let mut a = Args {
            url: env::var("OMT_SOURCE").ok(),
            list_only: false,
            index: None,
        };
        let mut it = env::args().skip(1);
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--connect" | "-c" => a.url = it.next(),
                "--list" | "-l" => a.list_only = true,
                "--index" => a.index = it.next().and_then(|v| v.parse().ok()),
                other => {
                    // SAM passes launch params as a JSON blob in argv[1].
                    if let Some(pos) = other.find("\"index\":") {
                        let rest = &other[pos + 8..];
                        let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                        if let Ok(v) = num.parse() {
                            a.index = Some(v);
                        }
                    }
                }
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

fn discover(timeout: Duration) -> Vec<Source> {
    let mut out = Vec::new();
    match Discovery::new() {
        Ok(mut d) => {
            if d.refresh_for(timeout).is_err() {
                return out;
            }
            for src in d.sources() {
                out.push(Source {
                    name: src.instance_name().to_string(),
                    url: src.to_url(),
                    addrs: src.addresses.clone(),
                });
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

fn draw_picker(shared: &Shared, font: &Option<font::Font>, canvas: &mut [u8]) {
    let sources = shared.sources.lock().map(|s| s.clone()).unwrap_or_default();
    let selected = *shared.selected.lock().unwrap_or_else(|e| e.into_inner());

    let mut c = font::Canvas::new(canvas, UI_W, UI_H);
    c.clear(18, 16, 14);

    let Some(f) = font.as_ref() else {
        return;
    };

    c.text(f, 60, 50, 3, "OMT MONITOR", (255, 210, 120));
    c.text(f, 60, 110, 1, "SELECT A SOURCE ON THE LAN", (170, 170, 170));

    if sources.is_empty() {
        c.text(f, 60, LIST_TOP as usize, 2, "SEARCHING FOR OMT SOURCES...", (200, 200, 200));
    }

    for (i, s) in sources.iter().take(8).enumerate() {
        let y = LIST_TOP + i * ROW_H;
        if i == selected {
            c.fill(40, y - 10, UI_W - 80, ROW_H - 8, 70, 120, 60);
        }
        let label = if i == selected {
            format!("> {}", s.name.to_uppercase())
        } else {
            format!("  {}", s.name.to_uppercase())
        };
        c.text(f, 60, y, 2, &label, (240, 240, 240));
        let addr = s.addrs.first().cloned().unwrap_or_default();
        c.text(f, 720, y, 1, &addr, (160, 200, 160));
    }

    let hint_y = UI_H - 70;
    c.text(
        f,
        60,
        hint_y,
        1,
        "UP/DOWN: SELECT    ENTER: PLAY    BACK: RETURN TO LIST",
        (150, 150, 150),
    );
}

fn handle_keys(shared: &Shared) -> () {
    let events: Vec<(i32, bool)> = KEYQ
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .map(|mut q| q.drain(..).collect())
        .unwrap_or_default();

    for (code, pressed) in events {
        if !pressed {
            continue;
        }
        info!("key code={code}");
        let mode = shared.mode.load(Ordering::SeqCst);
        if mode == MODE_PICKER {
            let n = shared.sources.lock().map(|s| s.len()).unwrap_or(0);
            match code {
                KEY_UP => {
                    let mut sel = shared.selected.lock().unwrap();
                    *sel = sel.saturating_sub(1);
                }
                KEY_DOWN => {
                    let mut sel = shared.selected.lock().unwrap();
                    if n > 0 && *sel + 1 < n {
                        *sel += 1;
                    }
                }
                KEY_ENTER => {
                    if n > 0 {
                        let sel = *shared.selected.lock().unwrap();
                        let src = shared.sources.lock().unwrap().get(sel).cloned();
                        if let Some(src) = src {
                            info!("play {}", src.url);
                            *shared.play.lock().unwrap() = Some(src);
                            shared.mode.store(MODE_PLAYING, Ordering::SeqCst);
                        }
                    }
                }
                BTN_LEFT => {
                    // pointer click: pick the row under the cursor
                    let y = unsafe { presenter_pointer_y() };
                    if y as usize >= LIST_TOP {
                        let idx = ((y as usize - LIST_TOP) / ROW_H).min(n.saturating_sub(1));
                        *shared.selected.lock().unwrap() = idx;
                    }
                }
                _ => {}
            }
        } else if code == KEY_BACK {
            info!("back to picker");
            *shared.play.lock().unwrap() = None;
            shared.mode.store(MODE_PICKER, Ordering::SeqCst);
        }
    }
}

fn main() {
    let args = Args::parse();
    info!("omt-monitor starting");

    let nthreads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    vmx::use_shared_thread_pool(Some(nthreads));
    let caps = vmx::SimdCapabilities::detect();
    info!("vmx caps: neon={} threads={nthreads}", caps.neon);

    if args.list_only {
        for s in discover(Duration::from_secs(3)) {
            info!("source: {} {}", s.name, s.url);
        }
        return;
    }

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

    // Fixed source (source.txt / --connect) skips the picker when requested.
    let fixed: Option<Source> = args
        .url
        .clone()
        .or_else(source_from_file)
        .map(|u| Source {
            name: u.clone(),
            url: u,
            addrs: Vec::new(),
        });

    extern "C" fn in_log(msg: *const core::ffi::c_char) {
        if !msg.is_null() {
            let s = unsafe { core::ffi::CStr::from_ptr(msg) }.to_string_lossy();
            info!("[input] {s}");
        }
    }
    std::thread::spawn(|| {
        unsafe { input_set_logger(in_log) };
        let rc = unsafe { input_probe() };
        info!("input probe exited: {rc}");
    });

    std::thread::spawn(|| {
        let rc = unsafe { wakelock_start() };
        info!("wakelock exited: {rc}");
    });

    // ---- presenter thread ----
    let pres = Arc::clone(&shared);
    let presenter = std::thread::spawn(move || {
        unsafe {
            presenter_set_log(c_log);
            presenter_set_key_handler(c_key);
        }
        let rc = unsafe { presenter_init(UI_W as i32, UI_H as i32) };
        if rc != 0 {
            info!("EGL presenter init failed: {rc}");
            pres.quit.store(true, Ordering::SeqCst);
            pres.cv.notify_all();
            return;
        }
        info!("EGL presenter ready");

        let font = font::Font::load();
        info!("font loaded: {}", font.is_some());
        let mut canvas = vec![0u8; UI_W * UI_H * 4];

        let mut frames: u64 = 0;
        let mut window = Instant::now();
        let mut cpu_start = cpu_seconds();
        let mut dec_us = 0u64;
        let mut dec_frames = 0u64;
        let mut pres_us = 0u64;
        let mut last_picker: Option<Instant> = None;

        loop {
            let _ = unsafe { presenter_roundtrip() };
            handle_keys(&pres);

            if pres.quit.load(Ordering::SeqCst) {
                break;
            }

            let mode = pres.mode.load(Ordering::SeqCst);
            if mode == MODE_PICKER {
                let now = Instant::now();
                if last_picker.map(|t| t.elapsed() > Duration::from_millis(150)).unwrap_or(true) {
                    draw_picker(&pres, &font, &mut canvas);
                    let _ = unsafe { presenter_frame(canvas.as_ptr(), UI_W as i32, UI_H as i32) };
                    last_picker = Some(now);
                }
                std::thread::sleep(Duration::from_millis(30));
                continue;
            }

            // playing: present the newest decoded frame
            let mut slot = pres.latest.lock().unwrap();
            while slot.is_none() && !pres.quit.load(Ordering::SeqCst) {
                let (g, _) = pres.cv.wait_timeout(slot, Duration::from_millis(100)).unwrap();
                slot = g;
                let _ = unsafe { presenter_roundtrip() };
                handle_keys(&pres);
                if pres.mode.load(Ordering::SeqCst) == MODE_PICKER {
                    break;
                }
            }
            if pres.mode.load(Ordering::SeqCst) == MODE_PICKER {
                continue;
            }
            let Some((px, w, h)) = slot.take() else {
                continue;
            };
            drop(slot);

            let t0 = Instant::now();
            let _ = unsafe { presenter_frame(px.as_ptr(), w, h) };
            pres_us += t0.elapsed().as_micros() as u64;
            frames += 1;
            dec_us += pres.decode_us.swap(0, Ordering::Relaxed);
            dec_frames += pres.decode_frames.swap(0, Ordering::Relaxed);

            let secs = window.elapsed().as_secs_f64();
            if secs >= 2.0 {
                let n = frames.max(1) as f64;
                let dn = dec_frames.max(1) as f64;
                let cpu = (cpu_seconds() - cpu_start) / secs * 100.0;
                info!(
                    "fps={:.1} decode={:.2}ms present={:.2}ms cpu={:.0}% frames={}",
                    frames as f64 / secs,
                    dec_us as f64 / dn / 1000.0,
                    pres_us as f64 / n / 1000.0,
                    cpu,
                    frames
                );
                frames = 0;
                dec_us = 0;
                dec_frames = 0;
                pres_us = 0;
                window = Instant::now();
                cpu_start = cpu_seconds();
            }
        }
        unsafe { presenter_shutdown() };
        info!("presenter stopped");
    });

    // ---- discovery + playback thread ----
    if let Some(src) = fixed {
        info!("fixed source: {}", src.url);
        *shared.play.lock().unwrap() = Some(src);
        shared.mode.store(MODE_PLAYING, Ordering::SeqCst);
    }

    let pick_index = args.index;
    let mut first_source_at: Option<Instant> = None;
    let mut last_disco = Instant::now() - Duration::from_secs(10);
    let mut session: Option<ReceiverSession> = None;
    let mut current_url = String::new();

    while !shared.quit.load(Ordering::SeqCst) {
        // refresh the source list every 3 s (and always while in the picker)
        if last_disco.elapsed() > Duration::from_secs(3) {
            let found = discover(Duration::from_millis(800));
            if !found.is_empty() {
                let mut names: Vec<String> = found.iter().map(|s| s.url.clone()).collect();
                names.sort();
                names.dedup();
                let mut list = shared.sources.lock().unwrap();
                if list.len() != found.len() {
                    info!("sources: {}", found.iter().map(|s| s.name.clone()).collect::<Vec<_>>().join(", "));
                }
                *list = found;
            }
            last_disco = Instant::now();
        }

        if shared.mode.load(Ordering::SeqCst) == MODE_PICKER {
            // No remote-key input is available to raw wayland clients, so the
            // picker auto-plays: an explicit `index` launch param wins, otherwise
            // the first discovered source is played after a short grace period.
            {
                let n = shared.sources.lock().map(|s| s.len()).unwrap_or(0);
                if n > 0 {
                    let t = *first_source_at.get_or_insert_with(Instant::now);
                    let ready = pick_index.is_some() || t.elapsed() > Duration::from_secs(4);
                    if ready && shared.play.lock().map(|p| p.is_none()).unwrap_or(false) {
                        let idx = pick_index.unwrap_or(0).min(n - 1);
                        let src = shared.sources.lock().unwrap().get(idx).cloned();
                        if let Some(src) = src {
                            info!("select [{idx}] {}", src.url);
                            *shared.play.lock().unwrap() = Some(src);
                            shared.mode.store(MODE_PLAYING, Ordering::SeqCst);
                            continue;
                        }
                    }
                }
            }
            if session.is_some() {
                session = None; // disconnect
                current_url.clear();
                let mut slot = shared.latest.lock().unwrap();
                *slot = None;
            }
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }

        let want = shared.play.lock().unwrap().clone();
        let Some(want) = want else {
            shared.mode.store(MODE_PICKER, Ordering::SeqCst);
            continue;
        };

        if session.is_none() || current_url != want.url {
            session = None;
            match connect(&want.url, &want.addrs) {
                Ok(s) => {
                    info!("connected {}", want.url);
                    session = Some(s);
                    current_url = want.url.clone();
                }
                Err(e) => {
                    info!("{e}; retry in 2s");
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                }
            }
        }

        let Some(s) = session.as_ref() else {
            continue;
        };
        let t0 = Instant::now();
        match s.recv_video_timeout(Duration::from_millis(100)) {
            Some(frame) => {
                shared
                    .decode_us
                    .fetch_add(t0.elapsed().as_micros() as u64, Ordering::Relaxed);
                shared.decode_frames.fetch_add(1, Ordering::Relaxed);
                shared.publish(frame.pixels, frame.width as i32, frame.height as i32);
            }
            None => {
                // nothing decoded in time; keep looping
            }
        }
    }

    shared.quit.store(true, Ordering::SeqCst);
    shared.cv.notify_all();
    let _ = presenter.join();
    info!("bye");
}
