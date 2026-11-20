# ビルド手順（aarch64 + 64to32 bridge）

現状のビルドは **aarch64 (64bit) + webos-bridge-64to32** を前提にしている。
32bit (armv7) は vmx-rs の `arm32-neon` feature（nightly）でデコードは可能だが、
glibc 起動時の問題を回避できていないため未採用。

## 1. ツールチェーン（sysroot のみ使用）

```bash
# webOS SDK buildroot（ホストバイナリは Linux 用なので macOS では sysroot だけ使う）
curl -LO https://github.com/cscd98/buildroot-nc4/releases/download/webos-be2ff5f/aarch64-webos-linux-gnu_sdk-buildroot-x86_64.tar.gz
tar xzf aarch64-webos-linux-gnu_sdk-buildroot-x86_64.tar.gz
```

Rust は `rust-lld` をリンカにし、**crt1.o / crti.o / crtn.o を明示的にリンクする**
（これが無いと e_entry が 0 になり、起動直後にセグフォルトする）。

```bash
TC=$PWD/aarch64-webos-linux-gnu_sdk-buildroot
SYS=$TC/aarch64-webos-linux-gnu/sysroot
GCCL=$TC/lib/gcc/aarch64-webos-linux-gnu/16.2.0
BL=/media/developer/apps/usr/palm/applications/org.webosbrew.bridge-64to32/lib

export RUSTFLAGS="\
 -C linker=rust-lld -C link-arg=--disable-new-dtags \
 -C link-arg=$SYS/usr/lib/crt1.o -C link-arg=$SYS/usr/lib/crti.o -C link-arg=$SYS/usr/lib/crtn.o \
 -C link-arg=--sysroot=$SYS -C link-arg=-L$SYS/usr/lib -C link-arg=-L$SYS/lib -C link-arg=-L$GCCL \
 -C link-arg=-L<bridge-lib-copy> -C link-arg=-l:libSDL2-2.0.so.0 \
 -C link-arg=--dynamic-linker=$BL/ld-linux-aarch64.so.1 -C link-arg=-rpath -C link-arg=$BL"
```

- `--disable-new-dtags` で **DT_RPATH**（推移的に継承）にする。RUNPATH だと
  `libSDL2 → libm` のような間接依存が解決できない。
- `<bridge-lib-copy>` は IPK から展開した `lib/`（リンク時のみ使用）。

## 2. ビルド & パッケージ

```bash
cargo +nightly build --release --target aarch64-unknown-linux-gnu   # MSRV 1.97 のため nightly
python3 packaging/build-ipk.py --bin target/aarch64-unknown-linux-gnu/release/omt-monitor
```

## 3. 実機での確認

- `luna://com.webos.appInstallService/dev/install` で IPK を投入（root があれば devmode_enabled で通る）
- 起動: `luna://com.webos.applicationManager/launch {"id":"org.mikansei.omt-monitor"}`
- ログはアプリのディレクトリの `run.log`（アプリは jail 内で /tmp が private のため）
