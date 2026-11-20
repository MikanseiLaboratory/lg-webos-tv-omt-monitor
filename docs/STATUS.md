# 実機ステータス（2026-11-20）

## 動作している構成

- **armv7 32bit ネイティブ**（bridge 不要）。TV 自身の EGL/GLESv2 + wayland で描画
- HBC からインストールした `org.mikansei.omt-monitor` を SAM 経由で起動
- OMT ソースは DNS-SD で自動検出（`source.txt` があれば固定 URL を優先）
- 受信スレッドと描画スレッドを分離し、最新フレーム優先でドロップ
- vmx は `-C target-feature=-soft-float,+neon` でビルドして A32 NEON を有効化

## 実測（LG OLED48A2PJA / webOS 10.3.2 / Cortex-A55 ×4）

| 入力 | decode | present | fps |
|---|---|---|---|
| 1280x720 @60 | 16.4〜17.1 ms | 5.8〜6.4 ms | **59〜60** |
| 1920x1080 @60 | 33 ms | 11 ms | **30〜31** |

参考: vmx の `simd_report`（path=neon）を実機で実行

```
1920x1080 load_from+decode_uyvy: 29.4 ms/frame (34.0 fps)
1920x1080 load_from+decode_bgra: 36.2 ms/frame (27.6 fps)
```

**1080p60 はこの SoC のソフトウェアデコード上限（34fps）を超えるため不可能。**
720p60 で 60fps 相当、1080p なら 30fps で安定動作する。

## スリープ対策（wakelock）

webOS は入力無操作で画面を落とす（Screen Saver / Screen Off）。EGL で描画しているだけのネイティブアプリは
「映像再生中」と認識されないため、アプリ側から tvpowerd に働きかける。

- `app/csrc/wakelock.c`（libluna-service2 + glib）
  - 起動直後に `luna://com.webos.service.tvpower/power/turnOnScreen` を呼んで画面を起こす
  - 30 秒ごとに同メソッドを再送（画面が落ちても最大 30 秒で復帰）
  - `registerScreenSaverRequest` に自アプリ名で登録
- 実機検証: `turnOffScreen` で画面を落とした状態からアプリ起動 → 直後に `state: Active` へ復帰。
  再生を 3 分以上継続しても `state: Active` のまま（fps 59.4〜60.1 を維持）

## ハードウェアデコードの可否（調査結果）

- OMT の映像は **VMX1 のみ**（`omtrs/PROTOCOL.md` に Codec FourCC はあるが、実装は VMX1 パススルーのみ）
- TV のハードウェアデコーダ（NDL / GStreamer）は H.264 / HEVC / VP9 / AV1 用で **VMX1 非対応**
- vmx には `wgpu` feature（GPU デコード）があるが、webOS 10 のアプリ向け Vulkan は無く、
  8x8 ブロック単位のデコードは GPU の起動オーバーヘッドに負ける可能性が高い
- 結論: **1080p60 は不可**（NEON CPU デコードで 34fps が上限）。720p60 で 60fps 達成

## 操作

```bash
# ビルド（macOS + webOS arm ツールチェーン + nightly）
TC=/path/to/arm-webos-linux-gnueabi_sdk-buildroot ./build-app-armv7.sh

# 実機へ（root があれば devmode_enabled で通る）
luna-send -n 1 -f luna://com.webos.appInstallService/dev/install \
  '{"id":"org.mikansei.omt-monitor","ipkUrl":"/tmp/omt.ipk","subscribe":true}'
```

- 起動: `luna-send -n 1 -f luna://com.webos.applicationManager/launch '{"id":"org.mikansei.omt-monitor"}'`
- ログ: アプリディレクトリの `run.log`
- 固定ソースにしたい場合は同ディレクトリに `source.txt`（例 `omt://192.168.3.53:6400/Colorbars`）

## 既知の制限

- 1080p60 は不可（上記）。720p60 か 1080p30 を推奨
- 音声は未実装（映像のみ）
- 入力（リモコン）処理は未実装。終了はアプリ切替で
