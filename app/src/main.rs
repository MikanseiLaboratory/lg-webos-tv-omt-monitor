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
    fn sdl_presenter_init(w: i32, h: i32, want_gl: i32) -> i32;
    fn sdl_presenter_set_log(cb: extern "C" fn(*const core::ffi::c_char));
    fn sdl_presenter_frame(pixels: *const u8, w: i32, h: i32) -> i32;
    fn sdl_presenter_poll_key(code: *mut i32) -> i32;
    fn audio_open(rate: i32, channels: i32) -> i32;
    fn audio_push(interleaved: *const f32, frames: i32);
    fn audio_close();
    fn audio_level_frames() -> i32;
    fn sdl_presenter_size(w: *mut i32, h: *mut i32);
    fn sdl_presenter_timings(upload_ms: *mut f64, present_ms: *mut f64);
    fn sdl_presenter_shutdown();
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
    audio_frames: AtomicU64,
    audio_drops: AtomicU64,
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
            audio_frames: AtomicU64::new(0),
            audio_drops: AtomicU64::new(0),
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

/// Interleave one planar FPA1 frame and queue it for playback.
struct AudioPacer {
    start: Instant,
    pushed: u64,
    rate: f64, /* frames per second */
}

impl AudioPacer {
    fn new(rate: f64) -> Self {
        AudioPacer { start: Instant::now(), pushed: 0, rate }
    }

    /// Allow realtime rate plus a small jitter buffer.
    fn allow(&self, buffer_frames: f64) -> bool {
        let allowed = self.start.elapsed().as_secs_f64() * self.rate + buffer_frames;
        (self.pushed as f64) < allowed
    }
}

fn pump_audio(
    session: &ReceiverSession,
    audio: &mut Option<(i32, i32)>,
    pacer: &mut Option<AudioPacer>,
    counter: &AtomicU64,
) -> u64 {
    let mut frames_out = 0u64;
    while let Some(a) = session.try_recv_audio() {
        let ch = a.channels as usize;
        let n = a.samples_per_channel as usize;
        if ch == 0 || n == 0 {
            continue;
        }
        let need_open = match *audio {
            Some((r, c)) => r != a.sample_rate || c as usize != ch,
            None => true,
        };
        if need_open {
            *audio = Some((a.sample_rate, ch as i32));
            unsafe { audio_open(a.sample_rate, ch as i32) };
            info!("audio {} Hz, {} ch", a.sample_rate, ch);
        }
        let bytes = &a.pcm_planar_f32;
        if bytes.len() < ch * n * 4 {
            continue;
        }
        let mut out = vec![0f32; n * ch];
        for c in 0..ch {
            let base = c * n * 4;
            for i in 0..n {
                let o = base + i * 4;
                out[i * ch + c] =
                    f32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
            }
        }
        let rate = a.sample_rate as f64 / n as f64;
        let p = pacer.get_or_insert_with(|| AudioPacer::new(rate));
        // Bound the buffered latency: drop frames that would exceed ~250 ms.
        let max_frames = (a.sample_rate / 4).max(1);
        if unsafe { audio_level_frames() } > max_frames {
            continue;
        }
        if !p.allow(rate * 0.12) {
            continue;
        }
        unsafe { audio_push(out.as_ptr(), n as i32) };
        p.pushed += 1;
        frames_out += 1;
        counter.fetch_add(1, Ordering::Relaxed);
    }
    frames_out
}

fn connect(url: &str, addresses: &[String]) -> Result<ReceiverSession, String> {
    let cfg = ReceiverConfig {
        frame_types: FrameType::VIDEO | FrameType::AUDIO,
        connect_timeout: Duration::from_secs(5),
        // webOS decoders are slow enough to need more than the 100 ms default;
        // the SDL queue is capped at the same value in pump_audio().
        audio_max_latency: Duration::from_millis(250),
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

fn draw_picker(shared: &Shared, font: &Option<font::Font>, canvas: &mut [u8], ui_w: usize, ui_h: usize) {
    let sources = shared.sources.lock().map(|s| s.clone()).unwrap_or_default();
    let selected = *shared.selected.lock().unwrap_or_else(|e| e.into_inner());

    let mut c = font::Canvas::new(canvas, ui_w, ui_h);
    c.clear(18, 16, 14);

    let Some(f) = font.as_ref() else {
        return;
    };

    // Layout derived from the real font metrics so it adapts to any PSF size.
    let fh = f.height.max(8);
    let title_scale = (ui_h / (fh * 6)).max(1);
    let title_y = ui_h / 27;
    let sub_scale = (title_scale / 2).max(1);
    let sub_y = title_y + fh * title_scale + ui_h / 90;
    let list_top = sub_y + fh * sub_scale + ui_h / 20;
    let row_h = fh * sub_scale + ui_h / 60;

    c.text(f, 60, title_y, title_scale, "OMT MONITOR", (255, 210, 120));
    c.text(f, 60, sub_y, sub_scale, "SELECT A SOURCE ON THE LAN", (170, 170, 170));

    if sources.is_empty() {
        c.text(f, 60, list_top, sub_scale, "SEARCHING FOR OMT SOURCES...", (200, 200, 200));
    }

    for (i, s) in sources.iter().take(8).enumerate() {
        let y = list_top + i * row_h;
        if i == selected {
            c.fill(40, y - 10, ui_w - 80, row_h - 8, 70, 120, 60);
        }
        let label = if i == selected {
            format!("> {}", s.name.to_uppercase())
        } else {
            format!("  {}", s.name.to_uppercase())
        };
        c.text(f, 60, y, sub_scale, &label, (240, 240, 240));
        let addr = s.addrs.first().cloned().unwrap_or_default();
        c.text(f, (ui_w * 60) / 100, y, 1, &addr, (160, 200, 160));
    }

    let hint_y = ui_h - (ui_h / 15);
    c.text(
        f,
        60,
        hint_y,
        1,
        "UP/DOWN: SELECT    ENTER: PLAY    ANY KEY: BACK TO LIST",
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
        } else if true {
            info!("back to picker (key {code})");
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
        // presenter.txt: "egl" | "sdl" (SDL renderer) | "gl" (SDL window + GLES3 PBO)
        let mode = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|d| d.join("presenter.txt")))
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| t.trim().to_string())
            .unwrap_or_else(|| "sdl".to_string());
        let want_sdl = mode != "egl";
        let want_gl = mode == "gl";

        let mut use_sdl = false;
        if want_sdl {
            unsafe { sdl_presenter_set_log(c_log) };
            let rc = unsafe { sdl_presenter_init(UI_W as i32, UI_H as i32, want_gl as i32) };
            if rc == 0 {
                use_sdl = true;
                info!("SDL presenter ready");
            } else {
                info!("SDL presenter init failed: {rc}");
            }
        }
        if !use_sdl {
            let rc = unsafe { presenter_init(UI_W as i32, UI_H as i32) };
            if rc != 0 {
                info!("EGL presenter init failed: {rc}");
                pres.quit.store(true, Ordering::SeqCst);
                pres.cv.notify_all();
                return;
            }
            info!("EGL presenter ready");
        }

        // pump input: SDL events when available, wayland otherwise
        let pump = move |pres: &Shared| {
            if use_sdl {
                loop {
                    let mut code = 0i32;
                    match unsafe { sdl_presenter_poll_key(&mut code) } {
                        1 => {
                            let mapped = match code {
                                0x4000_0052 => Some(KEY_UP),
                                0x4000_0051 => Some(KEY_DOWN),
                                13 | 0x4000_0058 => Some(KEY_ENTER),
                                27 | 8 | 0x4000_0086 => Some(KEY_BACK),
                                _ => {
                                    info!("sdl key {code:#x}");
                                    None
                                }
                            };
                            if let Some(c) = mapped {
                                KEYQ.get_or_init(|| Mutex::new(Vec::new()))
                                    .lock()
                                    .map(|mut q| q.push((c, true)))
                                    .ok();
                            }
                        }
                        2 => pres.quit.store(true, Ordering::SeqCst),
                        _ => break,
                    }
                }
            } else {
                let _ = unsafe { presenter_roundtrip() };
            }
        };

        let font = font::Font::load();
        info!("font loaded: {} {:?}", font.is_some(), font.as_ref().map(|f| (f.width, f.height)));
        let (mut ui_w, mut ui_h) = (UI_W, UI_H);
        if use_sdl {
            let (mut w32, mut h32) = (0i32, 0i32);
            unsafe { sdl_presenter_size(&mut w32, &mut h32) };
            if w32 > 0 && h32 > 0 {
                ui_w = w32 as usize;
                ui_h = h32 as usize;
            }
        }
        info!("ui canvas {ui_w}x{ui_h}");
        let mut canvas = vec![0u8; ui_w * ui_h * 4];

        let mut frames: u64 = 0;
        let mut window = Instant::now();
        let mut cpu_start = cpu_seconds();
        let mut dec_us = 0u64;
        let mut dec_frames = 0u64;
        let mut pres_us = 0u64;
        let mut last_picker: Option<Instant> = None;

        loop {
            pump(&pres);
            handle_keys(&pres);

            if pres.quit.load(Ordering::SeqCst) {
                break;
            }

            let mode = pres.mode.load(Ordering::SeqCst);
            if mode == MODE_PICKER {
                let now = Instant::now();
                if last_picker.map(|t| t.elapsed() > Duration::from_millis(150)).unwrap_or(true) {
                    draw_picker(&pres, &font, &mut canvas, ui_w, ui_h);
                    if use_sdl {
                        let _ = unsafe { sdl_presenter_frame(canvas.as_ptr(), ui_w as i32, ui_h as i32) };
                    } else {
                        let _ = unsafe { presenter_frame(canvas.as_ptr(), ui_w as i32, ui_h as i32) };
                    }
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
                pump(&pres);
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
            if use_sdl {
                let _ = unsafe { sdl_presenter_frame(px.as_ptr(), w, h) };
            } else {
                let _ = unsafe { presenter_frame(px.as_ptr(), w, h) };
            }
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
                    "fps={:.1} decode={:.2}ms present={:.2}ms (up={:.2} draw={:.2}) cpu={:.0}% frames={} aframes={} adrop={} abuf={}ms",
                    frames as f64 / secs,
                    dec_us as f64 / dn / 1000.0,
                    pres_us as f64 / n / 1000.0,
                    {
                        let (mut u, mut p2) = (0f64, 0f64);
                        unsafe { sdl_presenter_timings(&mut u, &mut p2) };
                        u
                    },
                    {
                        let (mut u, mut p2) = (0f64, 0f64);
                        unsafe { sdl_presenter_timings(&mut u, &mut p2) };
                        p2
                    },
                    cpu,
                    frames,
                    pres.audio_frames.load(Ordering::Relaxed),
                    pres.audio_drops.load(Ordering::Relaxed),
                    unsafe { audio_level_frames() } / 48
                );
                frames = 0;
                dec_us = 0;
                dec_frames = 0;
                pres_us = 0;
                window = Instant::now();
                cpu_start = cpu_seconds();
            }
        }
        if use_sdl {
            unsafe { sdl_presenter_shutdown() };
        } else {
            unsafe { presenter_shutdown() };
        }
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
    let mut audio_state: Option<(i32, i32)> = None;
    let mut audio_pacer: Option<AudioPacer> = None;

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
                    let ready = pick_index.is_some() && t.elapsed() > Duration::from_millis(500);
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
                pump_audio(s, &mut audio_state, &mut audio_pacer, &shared.audio_frames);
                shared
                    .audio_drops
                    .store(s.statistics().frames_dropped_audio, Ordering::Relaxed);
            }
            None => {
                pump_audio(s, &mut audio_state, &mut audio_pacer, &shared.audio_frames);
                shared
                    .audio_drops
                    .store(s.statistics().frames_dropped_audio, Ordering::Relaxed);
            }
        }
    }

    shared.quit.store(true, Ordering::SeqCst);
    shared.cv.notify_all();
    unsafe { audio_close() };
    let _ = presenter.join();
    info!("bye");
}
