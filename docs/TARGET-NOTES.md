# 開発機の実測メモ

対象: LG OLED48A2PJA（2022 / A2）

## OS / ハードウェア

| 項目 | 値 | 取得方法 |
|---|---|---|
| モデル | OLED48A2PJA | `ssap://system/getSystemInfo` |
| ボード | K8HP_ARIB_JP | `luna://com.webos.service.tv.systemproperty/getSystemInfo` |
| webOS | 10.3.2 | 同上 `sdkVersion` |
| firmware | 33.31.75 | 同上 `firmwareVersion` |
| kernel | 5.4.268-294.24.papikonda.2 / **aarch64** | `uname -a` |
| userland | **armv7 32bit soft-float** | `od -An -tu1 -j4 -N1 /bin/busybox` → 1 (ELFCLASS32), `-j18` → 40 (EM_ARM) |
| loader | `/lib/ld-linux.so.3` | `ls /lib` |
| CPU | 4 core / NEON (`asimd`) | `nproc`, `/proc/cpuinfo` |
| RAM | 1.7 GB（空き ~400 MB） | `/proc/meminfo` |
| GPU | GLES2 / EGL (`libEGL.so`, `libGLESv2.so`) | `ls /usr/lib` |
| 映像 | `libNDL.so`, GStreamer 1.0 (`gst-launch-1.0`) | `ls /usr/lib`, `which` |
| SDL2 | 2.0.14（システム） | `ls /usr/lib/libSDL2*` |
| 空き容量 | `/media/developer` ~400 MB, `/tmp` 725 MB | `df -h` |

## ルート / HBC

- 取得: [SlopBro](https://github.com/throwaway96/slopbro)（jsserver 脆弱性、webOS 7 で有効）
- HBC `org.webosbrew.hbchannel` v0.7.3 導入済み、`/var/lib/webosbrew/{startup.sh,init.d}` で永続化
- root シェル: HBC の telnetd（23/tcp, パスワードなし）。不要時は
  `touch /var/luna/preferences/webosbrew_telnet_disabled` で無効化
- ファームウェア更新ブロック: `/var/luna/preferences/webosbrew_block_updates` + HBC startup が
  `/etc/hosts` を bind mount して `su.lge.com` 系 4 ホストを 127.0.0.1 に落とす

## 64bit 実行（bridge）

- [webos-bridge-64to32](https://github.com/webosbrew/webos-bridge-64to32) v0.0.8 を導入済み
- 静的リンクした aarch64 バイナリは bridge なしでも動く（実測）
- GLES / Wayland を使う aarch64 アプリは bridge の共有メモリ IPC 経由（セルフテスト起動確認済み）

## Rust ツールチェーン

- ホスト: rustc 1.91.1 + nightly 1.102.0
- クロスビルド実績: `armv7-unknown-linux-musleabi`, `aarch64-unknown-linux-musl`
  （`-C linker=rust-lld -C link-self-contained=yes` で外部リンカ不要）
- 実機へは HTTP で配布 → `curl` → 実行（root telnet 経由）
- **注意**: `vmx` は `rust-version = 1.97` のため 1.91 では `--ignore-rust-version` が必要

## M0 実測（2026-11-20）

- `webos-bridge-64to32` v0.0.8 を `luna://com.webos.appInstallService/dev/install` で導入成功
  （root + `devmode_enabled` があれば HBC を介さず CLI から入れられる）
- 起動すると `external_test_64`（appinfo の `main`）が `service/gles_proxy` を子プロセスとして
  spawn する（fds 3–6 を渡す）。GLES 描画（回転キューブ）まで確認
- 同梱 lib（64bit）: `libEGL.so.1` / `libGLESv2.so.2` / `libwayland-client` / `libwayland-egl` /
  **`libSDL2-2.0.so.0.3000.12` (2.30.12-webos)** / **`libSDL3.so.0.4.16`** / alsa-lib / aarch64 glibc 一式
- 外部 64bit アプリの要件:
  1. `patchelf --set-interpreter <bridge>/lib/ld-linux-aarch64.so.1 <binary>`
  2. `patchelf --add-rpath <bridge>/lib <binary>`
  3. IPK 化は `ares-package -A arm` または `scripts/patch-aarch64-in-ipk.sh`
- 既知の無害な警告: `/etc/ld.so.preload` の `/lib/libSegFault.so` が 32bit のため
  `wrong ELF class: ELFCLASS32 ... ignored` が出る
- 計測メモ: `load average` は常時 30 前後（`md_check_thread` 等のカーネルスレッドが D 状態で
  常駐するため）。CPU 使用率の指標には使わない

## M0 追加実測（2026-11-20 続き）

### 動いたこと
- `aarch64-unknown-linux-gnu` の Rust バイナリが **bridge 経由で起動**（crt1.o を明示リンクするのが必須）
- IPK の手組み（`packaging/build-ipk.py`）でインストール成功
- OMT の **DNS-SD ディスカバリと TCP 接続**が実機で動作（`omt://LOCALHOST:6400/Colorbars` を検出）
- armv7 の A32 NEON は vmx-rs の `arm32-neon` feature（nightly）で実機テスト 26/26 パス

### 詰まっている点
- アプリの SDL2 初期化（`SDL_Init`）で即クラッシュ。C で組んだ `external_test_64`（EGL/GLES 直叩き）は
  動作するため、bridge の SDL2 を外部アプリから使う経路に問題がありそう。
- ログ: `target:` の直後、`SDL video driver = wayland` を出した直後にプロセスが消える（SIGSEGV）。
- 次の候補:
  1. `external_test_64` と同じ **EGL/GLES2 + wayland 直叩き**に置き換える（実績あり）
  2. bridge 同梱の **SDL3 (0.4.16)** を使う
  3. SDL2 の webOS ドライバが要求する env（`APPID`, `DISPLAY_ID`, `XDG_RUNTIME_DIR`）を確認

## M0 続き2（2026-11-20）

### SDL2/SDL3 が使えない理由（確定）
- bridge 同梱の **libxkbcommon 0.9.2** が webOS のキーマップを解釈できない:
  `Keycode too big: must be < 0xfff, got 0x5030` → `Failed to compile keymap`
- SDL の wayland ドライバはその直後に NULL 参照で落ちる（SIGSEGV）
- SDL3 も同じ xkbcommon を使うため、B 案（SDL3）はこの環境では成立しない見込み

### EGL/GLES2 直叩き（A案）は動作
`csrc/presenter_egl.c` + `build-app.sh` で実装。**root シェルからは動作確認済み**（1080p60 入力）:

```
fps=42.8 decode=5.89ms present=16.11ms cpu=117%
fps=38.3 decode=5.92ms present=16.77ms cpu=128%
fps=42.4 decode=5.68ms present=16.42ms cpu=119%
```

- `present` ≈ 16.4ms は eglSwapBuffers の vsync 待ち（60Hz）。decode と直列化しているため 42fps で頭打ち。
- 改善: swap interval 0 にして最新フレーム優先でドロップする / decode と present を別スレッド化。

### 残る課題
- **SAM から起動した場合のみ** `wl_display_connect` 直前でプロセスが消える（root シェルからは同じバイナリが動作）。
  jail/seccomp かコンポジタ側の制約と思われる。要調査（次は bridge アプリと同じ起動条件を突き合わせる）。

## 32bit ネイティブ経路（2026-11-20 続き3）— これが本線

bridge を使わず、**TV 自身の 32bit EGL/GLESv2 + wayland** で描画する経路に切替。SAM 起動でも動作する。

### ビルド要点（`build-app-armv7.sh`）
- target: `armv7-unknown-linux-gnueabi`（soft-float）
- `-C relocation-model=static -C link-arg=-no-pie`（sysroot の crt1.o が非PICのため）
- crt1.o / crti.o / crtn.o を明示リンク（**これが無いと e_entry=0 で即死**）
- `getauxval` が sysroot の古い glibc に無いので自前スタブ（/proc/self/auxv を読む）をリンク
- SDL2 は使わない（`--no-default-features`、sdl_stub.rs にフォールバック）

### 実測（1080p60 カラーバー、SAM 起動）
```
fps=18.8 decode=30.1ms present=12.9ms cpu=218%
```
- 表示は `LSM NL_VSC {"app_id":"org.mikansei.omt-monitor","visible":true}`
- decode が scalar VMX1 のため 30ms → 19fps 頭打ち（present は vsync 待ちで 13〜16ms）

### armv7 NEON の壁
- `vmx` の `arm32-neon` feature は **LLVM の soft-float 制約**で `rustc-LLVM ERROR: Do not know how to soften this operator's operand!` になる
- `lto = "thin"` を有効にすると vmx 自体はコンパイルできるが、最終バイナリの LTO で同じエラー
- `+vfp3,+neon` のグローバル付与も不可（他クレートの float が壊れる）
- 対策候補: (a) vmx を cdylib として別ビルドして C ABI で呼ぶ、(b) ホットループだけ C/asm で書く、(c) 64bit bridge 経路の proxy spawn を解決する
