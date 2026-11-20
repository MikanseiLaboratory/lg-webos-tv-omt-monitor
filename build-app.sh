#!/usr/bin/env bash
# Build the OMT monitor for webOS (aarch64 + 64to32 bridge) on macOS.
#
# Requirements:
#   - webOS aarch64 buildroot sysroot (cscd98/buildroot-nc4)
#   - a local copy of the bridge IPK's lib/ dir (BRIDGE_LIB)
#   - nightly Rust (openmediatransport / vmx require 1.97)
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
APP="$HERE/app"

TC="${TC:?set TC to the aarch64 webOS buildroot dir}"
SYS="$TC/aarch64-webos-linux-gnu/sysroot"
GCCL="$TC/lib/gcc/aarch64-webos-linux-gnu/16.2.0"
BRIDGE_LIB="${BRIDGE_LIB:?set BRIDGE_LIB to the extracted bridge lib/ dir}"
BL_RUNTIME=/media/developer/apps/usr/palm/applications/org.webosbrew.bridge-64to32/lib

mkdir -p "$HERE/build"

echo "== compiling C presenter"
clang --target=aarch64-linux-gnu --sysroot="$SYS" -O2 -fPIC \
  -c "$APP/csrc/presenter_egl.c" -o "$HERE/build/presenter_egl.o"

echo "== building Rust app"
RUSTFLAGS="\
 -C linker=rust-lld -C link-arg=--disable-new-dtags \
 -C link-arg=$SYS/usr/lib/crt1.o -C link-arg=$SYS/usr/lib/crti.o -C link-arg=$SYS/usr/lib/crtn.o \
 -C link-arg=--sysroot=$SYS \
 -C link-arg=-L$SYS/usr/lib -C link-arg=-L$SYS/lib -C link-arg=-L$GCCL \
 -C link-arg=-L$BRIDGE_LIB -C link-arg=-L$SYS/usr/lib \
 -C link-arg=-l:libwayland-client.so.0 -C link-arg=-l:libwayland-egl.so.1 \
 -C link-arg=-l:libEGL.so.1 -C link-arg=-l:libGLESv2.so.2 \
 -C link-arg=-l:libwayland-webos-client.so.1 -C link-arg=-l:libSDL2-2.0.so.0 \
 -C link-arg=$HERE/build/presenter_egl.o \
 -C link-arg=--dynamic-linker=$BL_RUNTIME/ld-linux-aarch64.so.1 \
 -C link-arg=-rpath -C link-arg=\$ORIGIN/lib -C link-arg=-rpath -C link-arg=$BL_RUNTIME" \
  cargo +nightly build --release --target aarch64-unknown-linux-gnu --manifest-path "$APP/Cargo.toml"

echo "== packaging"
python3 "$HERE/packaging/build-ipk.py" \
  --bin "$APP/target/aarch64-unknown-linux-gnu/release/omt-monitor" \
  --lib-so "$SYS/usr/lib/libwayland-webos-client.so.1.0.0" \
  --out "$HERE/build/omt-monitor.ipk"
