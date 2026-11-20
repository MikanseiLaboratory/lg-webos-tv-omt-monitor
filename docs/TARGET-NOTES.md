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
