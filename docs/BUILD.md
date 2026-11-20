# ビルド手順

現在の本線は **32bit ネイティブ（armv7、bridge 不要）** である。ビルドは `build-app-armv7.sh` が行う。
初期に使っていた aarch64 + 64to32 bridge の手順は「2. aarch64 + 64to32 bridge（初期の経路）」に残す。

## 1. armv7 ネイティブ（現在の本線）

必要なもの: webOS arm buildroot の sysroot（`arm-webos-linux-gnueabi`）と nightly Rust。

```bash
TC=/path/to/arm-webos-linux-gnueabi_sdk-buildroot ./build-app-armv7.sh
```

スクリプトは C プレゼンタ / wakelock / input をコンパイルし、Rust アプリを
`--no-default-features --target armv7-unknown-linux-gnueabi` でビルドして、
`packaging/build-ipk.py` で `build/omt-monitor-arm.ipk` を生成する。

リンク要点:

| 項目 | 理由 |
|---|---|
| crt1.o / crti.o / crtn.o を明示リンク | 無いと `e_entry=0` になり起動直後に落ちる |
| `-C relocation-model=static -C link-arg=-no-pie` | sysroot の crt1.o が非 PIC のため |
| `getauxval` の自前スタブ | sysroot の glibc に `getauxval` が無いため（/proc/self/auxv を読む） |
| vmx クレートのみ `-C target-feature=-soft-float,+neon` | A32 NEON を有効化するため。`RUSTC_WRAPPER` で付与する |
| `--no-default-features` | Rust 側の `sdl2` feature を無効化する（`sdl_stub.rs` にフォールバック） |

`RUSTC_WRAPPER` の変更は cargo のフィンガープリントに載らないため、スクリプトは
vmx の fingerprint を消してから再ビルドする。詳細は [TARGET-NOTES.md](TARGET-NOTES.md) を参照。

## 2. aarch64 + 64to32 bridge（初期の経路）

初期は aarch64 (64bit) + webos-bridge-64to32 を前提としていた。当時、32bit (armv7) は
vmx-rs の `arm32-neon` feature（nightly）でデコードは可能だったが、glibc 起動時の問題を
回避できていないため未採用だった。

### 2.1 ツールチェーン（sysroot のみ使用）

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

### 2.2 ビルド & パッケージ

```bash
cargo +nightly build --release --target aarch64-unknown-linux-gnu   # MSRV 1.97 のため nightly
python3 packaging/build-ipk.py --bin target/aarch64-unknown-linux-gnu/release/omt-monitor
```

### 2.3 実機での確認

- `luna://com.webos.appInstallService/dev/install` で IPK を投入する（root があれば devmode_enabled で通る）。
- 起動: `luna://com.webos.applicationManager/launch {"id":"org.mikansei.omt-monitor"}`
- ログはアプリのディレクトリの `run.log`（アプリは jail 内で /tmp が private のため）。

## 依存クレート

`openmediatransport` と `vmx` は crates.io の **0.2.2** を使用する（vendoring なし）。

公開は crates.io の Trusted Publishing（GitHub OIDC）で行う。`v*` タグを push すると
`publish.yml` が走る。`Cargo.lock` が未コミットだと `cargo publish` が失敗するため、
バージョン変更時は lock も同時にコミットする。
