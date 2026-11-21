# lg-webos-tv-omt-monitor

Native LG webOS app that receives an [OMT (Open Media Transport)](https://github.com/openmediatransport)
stream and displays it full-screen on the TV, without a PC in the signal path.

**Status: working on hardware.** LG OLED48A2PJA (webOS 10.3.2): 720p60 at 59-60 fps, 1080p at 30-31 fps.
Details: [docs/STATUS.md](docs/STATUS.md).

---

## What it does

- Receives the OMT video stream over TCP from a sender (vMix, OBS, TouchDesigner, `omt-sender`, ...).
- Decodes VMX1 in software on the CPU (NEON). There is no hardware codec path.
- Presents BGRA frames with EGL/GLES2 on a Wayland surface.
- Plays the OMT audio stream (FPA1) through SDL2.
- Keeps the TV awake while streaming (`tvpowerd` screen-on request, re-sent every 30 s).
- Finds OMT sources by DNS-SD and shows a source list on the TV.

## Network requirement

Use a **wired LAN connection** between the sender and the TV. OMT over Wi-Fi is
usable but jittery: expect occasional audio dropouts and dropped video frames on
a congested or weak link. Wired Ethernet removes most of that jitter.

## Target device (measured)

| Item | Value |
|---|---|
| Model / board | OLED48A2PJA / `K8HP_ARIB_JP` |
| webOS | 10.3.2 (firmware 33.31.75) |
| Kernel | 5.4.268 aarch64 |
| Userland | armv7, 32-bit, GNU EABI soft-float (`/lib/ld-linux.so.3`) |
| CPU | 4x Cortex-A55 with NEON (`asimd`) |
| RAM | 1.7 GB (about 400 MB free) |
| GPU | GLES2 / EGL |

The kernel is aarch64 and runs 32-bit ELF binaries. The userland is 32-bit only, so the 64-bit
bridge is not used.

## Architecture

```
 OMT source (vMix / OBS / omt-sender ...)
        |  TCP  (VMX1 video + PCM audio + metadata)
        v
 webOS app (Rust, armv7 32-bit native)
   discovery (DNS-SD) -> source picker
        v
   receive/decode thread: ReceiverSession -> vmx (NEON) -> BGRA
        v
   present thread: GLES2 texture upload -> EGL / Wayland surface
```

- **32-bit native.** The app is built for `armv7-unknown-linux-gnueabi` and runs directly on the TV
  against its own `libEGL.so` / `libGLESv2.so` / Wayland. No 64-to-32 bridge.
- **Threads.** One receive/decode thread and one present thread. The present thread keeps the newest
  frame and drops stale ones; `eglSwapInterval(0)` removes the vsync wait.
- **Discovery.** DNS-SD enumerates OMT sources at start-up and the list is drawn on screen
  (PSF console font `Agafari-16.psfu.gz`, rendered into a BGRA canvas).
- **Source picker.** See [Source selection](#source-selection).

## Build

Requirements: the webOS arm buildroot sysroot (`arm-webos-linux-gnueabi`) and the pinned
nightly in `rust-toolchain.toml` (the `vmx` crate needs Rust 1.97, and its AArch32 NEON
kernels use unstable intrinsics).

```bash
TC=/path/to/arm-webos-linux-gnueabi_sdk-buildroot ./build-app-armv7.sh
```

The script compiles the C sources (EGL presenter, wakelock, input, `getauxval` stub), then builds the
Rust app with `--no-default-features --target armv7-unknown-linux-gnueabi`, and produces
`build/omt-monitor-arm.ipk` via `packaging/build-ipk.py`.

Link notes:

| Item | Reason |
|---|---|
| `crt1.o` / `crti.o` / `crtn.o` linked explicitly | Without them `e_entry` is 0 and the process dies immediately. |
| `-C relocation-model=static -C link-arg=-no-pie` | The sysroot `crt1.o` is not PIC. |
| `getauxval` stub | The sysroot glibc has no `getauxval`; the stub reads `/proc/self/auxv`. |
| `-C target-feature=-soft-float,+neon` on the `vmx` crate only | Enables A32 NEON codegen. Applied through `RUSTC_WRAPPER` (`build/rustc-vmx-neon.py`). Other crates stay soft-float. |
| Rebuild of `vmx` after the wrapper change | `RUSTC_WRAPPER` is not part of the cargo fingerprint, so `fingerprint/vmx-*` is deleted first. |

## Install and run on the TV

Requires Homebrew Channel and root. The IPK can then be installed from the CLI without dev mode.

```bash
luna-send -n 1 -f luna://com.webos.appInstallService/dev/install \
  '{"id":"org.mikansei.omt-monitor","ipkUrl":"/tmp/omt.ipk","subscribe":true}'

luna-send -n 1 -f luna://com.webos.applicationManager/launch \
  '{"id":"org.mikansei.omt-monitor"}'
```

Logs are written to `run.log` in the app directory.

## Source selection

The picker is driven by the remote control (SDL2 presenter). A fixed source can also be selected
without any input, in two ways:

1. `source.txt` in the app directory pins a fixed URL, for example
   `omt://192.168.3.53:6400/Colorbars`. Playback starts without UI.
2. The `index` launch parameter plays entry *n* of the discovered list:

   ```bash
   luna-send -n 1 -f luna://com.webos.applicationManager/launch \
     '{"id":"org.mikansei.omt-monitor","params":{"index":1}}'
   ```

If neither is set, the picker stays on screen until the remote selects a source.

## Performance and constraints

Measured on the target device (LG OLED48A2PJA, 4x Cortex-A55):

| Input | decode | present | fps |
|---|---|---|---|
| 1280x720 @ 60 | 16.4-17.1 ms | 5.8-6.4 ms | 59-60 |
| 1920x1080 @ 60 | 29-33 ms | 11-12 ms | 30-31 |

`vmx` benchmark (`simd_report`, path = neon) on the device:

```
1920x1080 load_from+decode_uyvy: 29.4 ms/frame (34.0 fps)
1920x1080 load_from+decode_bgra: 36.2 ms/frame (27.6 fps)
```

**1080p59.94 and 1080p60 are not achievable on this SoC.** The codec is decoded in software and the
measured ceiling is about 34 fps for 1080p, so the stream is displayed at 30-31 fps. Do not claim
1080p60 support.

Hardware decoding is not an option. OMT carries VMX1 only, and the TV hardware decoders
(NDL / GStreamer) support H.264, HEVC, VP9 and AV1, not VMX1.

Remote-control input is delivered only through the SDL2 presenter. A raw Wayland client receives
keyboard focus but no key events, and the networkinput pointer socket yields nothing, because the
compositor does not advertise a `wl_webos_shell` role to it. SDL2 is therefore the default presenter;
writing `egl` to `presenter.txt` switches back to the raw EGL path, which has no input.

## Requirements

- Homebrew Channel installed and root access (needed to install the IPK without dev mode).
- `webos-bridge-64to32` is **not** required.

## Audio path

OMT carries audio as FPA1 (planar float32). The receiver interleaves it, bounds the
buffered latency to 250 ms, and pushes it into an SDL2 device opened at the source
sample rate. Frames that would exceed the latency budget are dropped, so playback
stays close to live. The receive loop drains the audio queue on every iteration.

`openmediatransport` 0.3.0 replaces its fixed 64-frame audio queue with a
media-time budget (`ReceiverConfig::audio_max_latency`). This app asks for 250 ms,
matching the SDL queue cap below, which still absorbs webOS decode stalls. Both
crates come from crates.io; nothing is vendored.

## Limitations

- Audio plays through SDL2. The playback buffer is capped at about 250 ms; frames past that are dropped, and a dry buffer plays silence. Wi-Fi jitter can still cause short dropouts.
- Remote-control input works only in the SDL2 presenter.
- 1080p is capped at about 30-31 fps; 1080p60 is impossible with software VMX1 decoding.
- The app must be installed as an IPK and launched through `applicationManager` (SAM).

## Documentation

- [docs/STATUS.md](docs/STATUS.md) — measured state on the device, wakelock, input findings
- [docs/BUILD.md](docs/BUILD.md) — build steps and link details
- [docs/IMPLEMENTATION_PLAN.md](docs/IMPLEMENTATION_PLAN.md) — goals, constraints, milestones
- [docs/TARGET-NOTES.md](docs/TARGET-NOTES.md) — device probe results and measurement log

## Related projects

- [openmediatransport-rs](https://github.com/MikanseiLaboratory/openmediatransport-rs) — OMT protocol and discovery
- [vmx-rs](https://github.com/MikanseiLaboratory/vmx-rs) — VMX1 codec (NEON)
- [webos-bridge-64to32](https://github.com/webosbrew/webos-bridge-64to32) — 64-bit runtime for webOS (not used here)
- [SDL-webOS](https://github.com/webosbrew/SDL-webOS) — SDL port evaluated during bring-up

## License

MIT
