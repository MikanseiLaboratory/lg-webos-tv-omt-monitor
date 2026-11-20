//! OMT (Open Media Transport) monitor for LG webOS TVs.
//!
//! Receives an OMT video stream, decodes VMX1 in software and presents BGRA
//! frames through SDL2 (which, on webOS, goes through the 64-to-32 bridge).

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
        // Apps run in a jail with a private /tmp, so log next to the executable.
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
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
    {
        let _ = f.write_all(line.as_bytes());
    }
}

macro_rules! info {
    ($($arg:tt)*) => { crate::log(&format!($($arg)*)) };
}

struct Args {
    url: Option<String>,
    list_only: bool,
    windowed: bool,
    width: i32,
    height: i32,
}

impl Args {
    fn parse() -> Self {
        let mut a = Args {
            url: env::var("OMT_SOURCE").ok(),
            list_only: false,
            windowed: false,
            width: 1920,
            height: 1080,
        };
        let mut it = env::args().skip(1);
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--connect" | "-c" => a.url = it.next(),
                "--list" | "-l" => a.list_only = true,
                "--windowed" | "-w" => a.windowed = true,
                "--width" => a.width = it.next().and_then(|v| v.parse().ok()).unwrap_or(1920),
                "--height" => a.height = it.next().and_then(|v| v.parse().ok()).unwrap_or(1080),
                "--help" | "-h" => {
                    eprintln!(
                        "omt-monitor [--connect omt://HOST:PORT/NAME] [--list] [--windowed] [--width N] [--height N]"
                    );
                    std::process::exit(0);
                }
                other => info!("ignoring unknown argument: {other}"),
            }
        }
        a
    }
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
                let url = src.to_url();
                info!(
                    "source: {} port={} url={} addrs={:?}",
                    src.instance_name(),
                    src.port,
                    url,
                    src.addresses
                );
                out.push((url, src.addresses.clone()));
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

fn main() {
    let args = Args::parse();
    info!(
        "omt-monitor starting (windowed={}, {}x{})",
        args.windowed, args.width, args.height
    );

    let mut target: Option<(String, Vec<String>)> =
        args.url.clone().map(|u| (u, Vec::new()));

    if args.list_only {
        discover(Duration::from_secs(3));
        return;
    }

    // Packaged installs have no argv, so allow a URL in source.txt next to the binary.
    if target.is_none() {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let cfg = dir.join("source.txt");
                if let Ok(text) = std::fs::read_to_string(&cfg) {
                    let url = text.trim().to_string();
                    if !url.is_empty() {
                        info!("using {} from {}", url, cfg.display());
                        target = Some((url, Vec::new()));
                    }
                }
            }
        }
    }

    if target.is_none() {
        info!("no --connect given; discovering OMT sources for 3s");
        let found = discover(Duration::from_secs(3));
        target = found.into_iter().next();
    }

    let Some((url, addresses)) = target else {
        info!("no OMT source found; pass --connect omt://HOST:PORT/NAME");
        std::process::exit(2);
    };
    info!("target: {url}");

    // The bridge's SDL2 defaults to a driver that is not usable on webOS 10;
    // force the wayland path (the bridge provides libwayland + libEGL stubs).
    if std::env::var_os("SDL_VIDEODRIVER").is_none() {
        unsafe { std::env::set_var("SDL_VIDEODRIVER", "wayland") };
    }
    info!("SDL video driver = {}", std::env::var("SDL_VIDEODRIVER").unwrap_or_default());

    if let Err(e) = sdl::init() {
        info!("SDL init failed: {e}");
        std::process::exit(3);
    }
    let window = match sdl::create_window("OMT Monitor", args.width, args.height, !args.windowed) {
        Ok(w) => w,
        Err(e) => {
            info!("window creation failed: {e}");
            sdl::quit();
            std::process::exit(3);
        }
    };
    let renderer = match sdl::create_renderer(&window) {
        Ok(r) => r,
        Err(e) => {
            info!("renderer creation failed: {e}");
            sdl::quit();
            std::process::exit(3);
        }
    };

    // Clear to black once so the TV does not show garbage while connecting.
    sdl::delay(50);

    let mut texture: Option<sdl::Texture> = None;
    let mut tex_size = (0i32, 0i32);
    let mut event_buf = [0u8; sdl::SDL_EVENT_SIZE];
    let mut frames: u64 = 0;
    let mut last_report = Instant::now();
    let mut report_frames: u64 = 0;
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
                sdl::delay(2000);
                continue 'outer;
            }
        };
        last_frame_at = Instant::now();

        while running {
            while let Some((ty, sym)) = sdl::poll_event(&mut event_buf) {
                if ty == sdl::SDL_QUIT
                    || (ty == sdl::SDL_KEYDOWN
                        && (sym == sdl::SDLK_ESCAPE || sym == sdl::SDLK_q))
                {
                    info!("quit requested");
                    running = false;
                }
            }
            if !running {
                break;
            }

            match session.recv_video_timeout(Duration::from_millis(100)) {
                Some(frame) => {
                    let (w, h) = (frame.width as i32, frame.height as i32);
                    if texture.is_none() || tex_size != (w, h) {
                        info!("video {}x{}", w, h);
                        texture = sdl::create_texture(&renderer, w, h).ok();
                        tex_size = (w, h);
                    }
                    if let Some(tex) = texture.as_ref() {
                        if sdl::update_texture(tex, &frame.pixels, w, h).is_ok() {
                            sdl::present(&renderer, tex, &window);
                        }
                    }
                    frames += 1;
                    report_frames += 1;
                    last_frame_at = Instant::now();
                }
                None => {
                    if last_frame_at.elapsed() > Duration::from_secs(3) {
                        info!("no frames for 3s; reconnecting");
                        break;
                    }
                }
            }

            if last_report.elapsed() >= Duration::from_secs(2) {
                let stats = session.statistics();
                info!(
                    "fps={:.1} total={} bytes={} decoded={} drop_wire={} drop_decode={}",
                    report_frames as f64 / last_report.elapsed().as_secs_f64(),
                    frames,
                    stats.bytes_received,
                    stats.frames_decoded,
                    stats.frames_dropped_wire,
                    stats.frames_dropped_decode
                );
                report_frames = 0;
                last_report = Instant::now();
            }
        }
        session.disconnect();
    }

    if let Some(tex) = texture.as_ref() {
        sdl::destroy(&window, &renderer, tex);
    }
    sdl::quit();
    info!("bye (frames={frames})");
}
