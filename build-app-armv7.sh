#!/usr/bin/env bash
# Build the OMT monitor for native 32-bit webOS (armv7), no 64to32 bridge.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
APP="$HERE/app"

TC="${TC:?set TC to the arm-webos-linux-gnueabi buildroot dir}"
SYS="$TC/arm-webos-linux-gnueabi/sysroot"
GCCL="$TC/lib/gcc/arm-webos-linux-gnueabi/16.2.0"

mkdir -p "$HERE/build/nolibgcc"

echo "== compiling C presenter (armv7)"
clang --target=armv7-linux-gnueabi --sysroot="$SYS" -O2 -fPIC \
  -c "$APP/csrc/presenter_egl.c" -o "$HERE/build/presenter_egl_arm.o"
clang --target=armv7-linux-gnueabi --sysroot="$SYS" -O2 -fPIC \
  -c "$HERE/tools/getauxval_stub.c" -o "$HERE/build/getauxval_stub.o"
clang --target=armv7-linux-gnueabi --sysroot="$SYS" -O2 -fPIC \
  $(pkg-config --cflags glib-2.0 2>/dev/null || echo "-I$SYS/usr/include/glib-2.0 -I$SYS/usr/lib/glib-2.0/include") \
  -c "$APP/csrc/wakelock.c" -o "$HERE/build/wakelock.o"
clang --target=armv7-linux-gnueabi --sysroot="$SYS" -O2 -fPIC \
  -I"$SYS/usr/include/glib-2.0" -I"$SYS/usr/lib/glib-2.0/include" \
  -c "$APP/csrc/input.c" -o "$HERE/build/input.o"
clang --target=armv7-linux-gnueabi --sysroot="$SYS" -O2 -fPIC \
  -c "$APP/csrc/presenter_sdl.c" -o "$HERE/build/presenter_sdl.o"
clang --target=armv7-linux-gnueabi --sysroot="$SYS" -O2 -fPIC \
  -c "$APP/csrc/audio_sdl.c" -o "$HERE/build/audio_sdl.o"

echo "== building Rust app (armv7)"
export RUSTC_WRAPPER="$HERE/tools/rustc-vmx-neon.py"
touch "$APP/src/main.rs"
# RUSTC_WRAPPER changes do not invalidate cargo fingerprints; force vmx rebuild.
rm -rf "$APP"/target/armv7-unknown-linux-gnueabi/release/.fingerprint/vmx-*
RUSTFLAGS="\
 -C relocation-model=static -C linker=rust-lld -C link-arg=-no-pie \
 -C link-arg=$SYS/usr/lib/crt1.o -C link-arg=$SYS/usr/lib/crti.o -C link-arg=$SYS/usr/lib/crtn.o \
 -C link-arg=$HERE/build/getauxval_stub.o \
 -C link-arg=$HERE/build/presenter_egl_arm.o -C link-arg=$HERE/build/wakelock.o -C link-arg=$HERE/build/input.o -C link-arg=$HERE/build/presenter_sdl.o -C link-arg=$HERE/build/audio_sdl.o \
 -C link-arg=-L$HERE/build/nolibgcc \
 -C link-arg=--sysroot=$SYS \
 -C link-arg=-L$SYS/usr/lib -C link-arg=-L$SYS/lib -C link-arg=-L$GCCL \
 -C link-arg=-lSDL2 -C link-arg=-lEGL -C link-arg=-lGLESv2 -C link-arg=-lwayland-client \
 -C link-arg=-lwayland-egl -C link-arg=-lwayland-webos-client \
 -C link-arg=-lluna-service2 -C link-arg=-lglib-2.0 -C link-arg=-lgobject-2.0 \
 -C link-arg=--dynamic-linker=/lib/ld-linux.so.3" \
  cargo +nightly-2026-11-01 build --release --no-default-features \
    --target armv7-unknown-linux-gnueabi --manifest-path "$APP/Cargo.toml"

echo "== packaging"
python3 "$HERE/packaging/build-ipk.py" \
  --bin "$APP/target/armv7-unknown-linux-gnueabi/release/omt-monitor" \
  --out "$HERE/build/omt-monitor-arm.ipk"
