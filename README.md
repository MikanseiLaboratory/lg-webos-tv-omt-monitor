# lg-webos-tv-omt-monitor

LG webOS テレビを **OMT (Open Media Transport)** のモニターにする、ネイティブアプリ実装。

[OMT](https://github.com/openmediatransport) の受信ストリーム（VMX1 / TCP / DNS-SD）をテレビ上で直接デコードし、
フルスクリーン表示 + 音声再生 + 遅延・ドロップなどのモニタリング OSD を提供する。

> **Status: 実機動作中** — LG OLED48A2PJA (webOS 10.3.2) で 720p60 = 57〜59fps / 1080p = 30fps。
> 詳細は [docs/STATUS.md](docs/STATUS.md)、設計は [docs/IMPLEMENTATION_PLAN.md](docs/IMPLEMENTATION_PLAN.md)。

---

## なぜ作るか

webOS には NDI / OMT の受信アプリが存在しない（2026-11 時点で公式・主要カスタムリポジトリ・GitHub を調査済み）。
一方、放送・配信現場では「送出機の隣に置いたテレビで OMT をそのまま見たい」需要が強い。
PC で受けて Moonlight で飛ばす構成は可能だが、余計なエンコードが 1 段入る。

本プロジェクトは **テレビ上で直接 OMT を受けて表示する** ことを目指す。

## ターゲット

| 項目 | 実測値（開発機: OLED48A2PJA） |
|---|---|
| 機種 / ボード | OLED48A2PJA / `K8HP_ARIB_JP` |
| webOS | **10.3.2**（firmware 33.31.75） |
| Kernel | 5.4.268 aarch64 |
| Userland | **armv7 (gnueabi, soft-float)** — 32bit |
| CPU | 4 core Cortex-A55 / NEON (`asimd`) |
| RAM | 1.7 GB（空き 400 MB 前後） |
| GPU | GLES2 / EGL |
| 映像出力 | `libNDL` (Native Display Layer), GStreamer 1.0, SDL2 2.0.14 |

32bit userland しかないため、**aarch64 でビルドした本アプリは
[webos-bridge-64to32](https://github.com/webosbrew/webos-bridge-64to32) 経由で実行する**（NEON を使うため必須。
32bit フォールバックでは VMX1 デコードが scalar になり実用速度が出ない）。

## アーキテクチャ（予定）

```
 OMT source (vMix / OBS / TouchDesigner / omt-sender ...)
        │  TCP  (VMX1 video + PCM audio + metadata)
        ▼
 ┌─────────────────────────────────────────────┐
 │ webOS app (Rust, aarch64)                   │
 │                                             │
 │  Discovery (DNS-SD / 手動IP)                │
 │        ↓                                    │
 │  openmediatransport-rs ReceiverSession      │
 │        ↓                                    │
 │  vmx (VMX1 decode, NEON) → BGRA frame       │
 │        ↓                                    │
 │  GLES2 presenter (texture upload)           │
 │        ↓                                    │
 │  EGL / Wayland surface (bridge-64to32)      │
 └─────────────────────────────────────────────┘
```

## 依存

- [MikanseiLaboratory/openmediatransport-rs](https://github.com/MikanseiLaboratory/openmediatransport-rs) — OMT プロトコル / Discovery
- [MikanseiLaboratory/vmx-rs](https://github.com/MikanseiLaboratory/vmx-rs) — VMX1 コーデック（NEON）
- [webosbrew/webos-bridge-64to32](https://github.com/webosbrew/webos-bridge-64to32) — aarch64 実行基盤（IPK）
- [webosbrew/SDL-webOS](https://github.com/webosbrew/SDL-webOS) — SDL（採用可否は M0 で判断）

## 予定機能

- OMT ソースの自動検出（DNS-SD）と手動 IP 指定
- フルスクリーン表示（1080p60 を目標、4K は 1/8 プレビューまたは非対応）
- 音声再生（A/V 同期）
- モニタリング OSD: 解像度 / fps / bitrate / 受信遅延 / ドロップ / 再接続回数
- 切断時の自動再接続、ソース切り替え
- Homebrew Channel からのインストール（IPK + HBC リポジトリ）

## ビルド（予定）

```bash
# webOS SDK (arm-webos-linux-gnueabi buildroot) を用意したうえで
cargo build --release --target aarch64-unknown-linux-gnu   # bridge 経由
# 32bit フォールバック
cargo build --release --target armv7-unknown-linux-gnueabi
```

パッケージングは `packaging/` の IPK ビルドスクリプト（`ares-package`）で行う。詳細は実装計画を参照。

## ロードマップ

| 段階 | 内容 |
|---|---|
| M0 | 実行基盤スパイク（bridge / aarch64 / GLES / VMX デコード速度実測） |
| M1 | OMT 発見・接続・統計表示（映像なし） |
| M2 | 映像デコード + 表示（1080p30） |
| M3 | 音声 + A/V 同期 |
| M4 | UI / OSD / 再接続 |
| M5 | 性能改善（1080p60 判定） |
| M6 | IPK パッケージ / HBC リポジトリ / CI / リリース |

## ライセンス

MIT
