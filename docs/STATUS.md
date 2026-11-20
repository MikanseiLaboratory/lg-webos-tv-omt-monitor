# 実機ステータス（2026-11-20）

## 動作している構成

- **armv7 32bit ネイティブ**（bridge 不要）。TV 自身の EGL/GLESv2 + wayland で描画する。
- HBC からインストールした `org.mikansei.omt-monitor` を SAM 経由で起動する。
- OMT ソースは DNS-SD で自動検出する。`source.txt` があれば固定 URL を優先する。
- 受信スレッドと描画スレッドを分離する。最新フレーム優先でドロップする。
- vmx は `-C target-feature=-soft-float,+neon` でビルドし、A32 NEON を有効化する。
- 無操作時の画面オフを防ぐため、tvpowerd へ画面復帰要求を送る（後述）。

## 実測（LG OLED48A2PJA / webOS 10.3.2 / Cortex-A55 ×4）

| 入力 | decode | present | fps |
|---|---|---|---|
| 1280x720 @60 | 16.4〜17.1 ms | 5.8〜6.4 ms | **59〜60** |
| 1920x1080 @60 | 33 ms | 11 ms | **30〜31** |

参考: vmx の `simd_report`（path=neon）を実機で実行した結果。

```
1920x1080 load_from+decode_uyvy: 29.4 ms/frame (34.0 fps)
1920x1080 load_from+decode_bgra: 36.2 ms/frame (27.6 fps)
```

**1080p60 はこの SoC のソフトウェアデコード上限（34fps）を超えるため不可能である。**
720p60 なら 60fps 相当、1080p なら 30fps で安定動作する。

## スリープ対策（wakelock）

webOS は入力無操作で画面を落とす（Screen Saver / Screen Off）。EGL で描画しているだけのネイティブアプリは
「映像再生中」と認識されない。そのためアプリ側から tvpowerd を操作する。

- `app/csrc/wakelock.c`（libluna-service2 + glib）
  - 起動直後に `luna://com.webos.service.tvpower/power/turnOnScreen` を呼び、画面を起こす。
  - 30 秒ごとに同メソッドを再送する（画面が落ちても最大 30 秒で復帰する）。
  - `registerScreenSaverRequest` に自アプリ名で登録する。
- 実機検証: `turnOffScreen` で画面を落とした状態からアプリを起動すると、直後に `state: Active` へ復帰する。
  再生を 3 分以上続けても `state: Active` を維持する（fps 59.4〜60.1）。

## ハードウェアデコードの可否（調査結果）

- OMT の映像は **VMX1 のみ**である（`omtrs/PROTOCOL.md` に Codec FourCC はあるが、実装は VMX1 パススルーのみ）。
- TV のハードウェアデコーダ（NDL / GStreamer）は H.264 / HEVC / VP9 / AV1 用であり、**VMX1 に対応しない**。
- vmx には `wgpu` feature（GPU デコード）がある。ただし webOS 10 のアプリ向け Vulkan は無く、
  8x8 ブロック単位のデコードは GPU の起動オーバーヘッドに負ける可能性が高い。
- 結論: **1080p60 は不可**（NEON CPU デコードで 34fps が上限）。720p60 で 60fps を達成する。

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
- 固定ソースにしたい場合は同ディレクトリに `source.txt` を置く（例 `omt://192.168.3.53:6400/Colorbars`）

## 既知の制限

- 1080p60 は不可（上記）。720p60 か 1080p30 を推奨する。
- 音声は未実装（映像のみ）。
- 入力（リモコン）処理は未実装。終了はアプリ切替で行う。

## ソース選択 UI と入力の現状（2026-11-20 追記）

### 実装したもの

- 起動時に DNS-SD で OMT ソースを列挙し、**TV 上に一覧を描画する**（PSF コンソールフォントを
  `/usr/share/consolefonts/Agafari-16.psfu.gz` から読み込み、BGRA キャンバスに描いて提示する）。
- 選択中の行をハイライト表示し、アドレスも併記する。
- 選択方法（現状）:
  1. `source.txt` があれば固定 URL を再生する（ヘッドレス）。
  2. 起動パラメータ `params.index` で一覧の n 番目を再生する。
     `luna-send -n 1 -f luna://com.webos.applicationManager/launch '{"id":"org.mikansei.omt-monitor","params":{"index":1}}'`
  3. 何も指定が無ければ **4 秒後に一覧の先頭を自動再生**する。

### リモコン入力が届かない件（実測）

- `wl_seat` / `wl_keyboard` はバインドでき、`kbd enter (focus)` も受信するが、
  **キーイベントが 1 件も来ない**。
- `/tmp/netinput.pointer.sock`（`luna://com.webos.service.networkinput/getPointerInputSocket` で取得）は
  接続できるが、やはりイベントが来ない（ソケットは root:root 770 のため `chmod 777` が必要）。
- 原因の推定: コンポジタが `wl_webos_shell` / `wl_shell` を公開しておらず（実測でグローバル一覧に無い）、
  アプリ用のシェルロールを持たないサーフェスにはリモコン入力をルーティングしない。
- 対策: **SDL2 プレゼンタ**（`app/csrc/presenter_sdl.c`）を実装し、既定の描画経路にした。SDL 経由ではリモコンの上下・OK・戻るが届く（実測: `key code=103/108/28`）。
  `presenter.txt` に `egl` を書くと従来の EGL 経路に戻る。
- SDL の描画は `SDL_RenderSetLogicalSize` でキャンバス全体を出力に合わせる（サーフェスサイズ差による欠けを防ぐ）Moonlight（SDL）はリモコン入力が動作しており、
  arm sysroot には SDL2 のヘッダと `libSDL2.so` が揃っている（32bit ネイティブのまま移行可能）。

## 音声（2026-11-20 追記）

- OMT の FPA1（planar float32）を受信し、インターリーブして SDL2 オーディオで再生する
- 受信ループのたびに `try_recv_audio` をすべて排出し、音声キューを詰まらせない
- `AUDIO_Q` を 10 → 64 フレームに拡張した crate を `deps/openmediatransport-rs` に vendored 化
- 再生バッファは最大 250 ms に制限（超過分は破棄）、SDL のコールバックバッファは 2048 サンプル
- 実測: `adrop` は 25/s 程度 → 4/s 程度まで減少、`abuf` は 60〜220 ms で推移
-  Wi-Fi 経由ではジッタで微小な途切れが残る（有線なら改善見込み）

## ネットワーク要件

送信機と TV は **有線 LAN** で接続することを推奨する。Wi-Fi でも動作するが、ジッタにより
音声の途切れや映像フレームの欠落が発生する。有線接続で大半は解消する。

## 最適化の試行（2026-11-20 追記2）

### 1. GLES3 + PBO による非同期アップロード
- `presenter.txt` に `gl` を書くと SDL ウィンドウ + GLES3 コンテキスト + ダブル PBO 経路で描画する
- 実機の GL は `OpenGL ES 3.2 build 24.1@6525005`
- 実測（720p）: `up=5.0〜5.3ms` / `draw=1.1〜1.5ms`（SDL レンダラ経路は `up=5.3〜5.6ms`）
- **改善なし**。ボトルネックは API ではなく、CPU→GPU のコピー帯域（約 700MB/s）そのもの

### 2. NDL（Native Display Layer）での直接表示
- NDL のヘッダ（`libndl-media/NDL_directmedia*.h`）を確認
- `NDL_DirectVideoPlay(void *buffer, unsigned int size)` は **圧縮ビットストリーム**（H.264/H.265/VP9/AV1）を渡す API
- VMX1 の生フレームを渡す経路は無く、**この最適化は不可**

### 結論
720p では present 6.3ms のうち 5.3ms がアップロードだが、decode 17ms が支配的なため
全体への影響は限定的。抜本的に削るには UYVY（2byte/px）で受けてシェーダ変換する必要があり、
デコーダ側（crate）の UYVY 出力対応が前提になる。
