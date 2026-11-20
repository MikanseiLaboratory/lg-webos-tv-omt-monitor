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
