# 実装計画 — lg-webos-tv-omt-monitor

- 対象: LG webOS テレビ（開発機 OLED48A2PJA / webOS 10.3.2）を OMT モニターにする
- 前提: Homebrew Channel (HBC) 導入済み・root 取得済み
- 状態: 計画（コードなし）

---

## 1. ゴール / 非ゴール

### ゴール
1. テレビ単体で OMT ストリームを受信し、フルスクリーン表示する（PC を介さない）
2. 音声を同期再生する
3. 「モニター」として必要な情報（fps / 遅延 / ドロップ / 再接続）を OSD 表示する
4. HBC から 1 タップでインストールできる（IPK + HBC リポジトリ）

### 非ゴール
- OMT **送信**（テレビを送出元にする）— 別プロジェクト
- 4K60 のリアルタイム表示（SoC 能力上、初期版では対象外）
- マルチビュー / 複数ソース同時表示
- NDI 対応（→ [lg-webos-tv-ndi-monitor](https://github.com/MikanseiLaboratory/lg-webos-tv-ndi-monitor) で別実装）

---

## 2. 制約（実測）

| 制約 | 値 | 影響 |
|---|---|---|
| webOS | 10.3.2 (33.31.75) | 2026-10 以降の jsserver 修正の影響を受けない世代。root 維持のため更新はブロックする |
| Kernel | aarch64 5.4.268 | 64bit バイナリは動くが userland lib は 32bit |
| Userland | armv7 gnueabi (soft-float) | 32bit ネイティブは system SDL2 が使えるが VMX が scalar |
| CPU | 4× A55, NEON | VMX1 デコードは NEON 必須。60p は要実測 |
| RAM | 1.7 GB / 空き ~400 MB | フレームバッファは 2〜3 枚まで。BGRA 1080p = 8.3 MB/枚 |
| GPU | GLES2 / EGL (PVR) | テクスチャアップロードで表示。シェーダは GLES2 縛り |
| 出力 | `libNDL` / GStreamer 1.0 / SDL2 2.0.14 | SDL2 は古い。SDL-webOS (2.30 backport) の利用を検討 |
| ストレージ | /media/developer ~400 MB 空き | IPK は数 MB に抑える |

補足: 32bit の `ld-linux.so.3` のみ存在（`ld-linux-armhf.so.3` なし）＝ **soft-float ABI**。
Rust の `armv7-unknown-linux-gnueabi` がこれに一致する。

---

## 3. アーキテクチャ

```
┌────────────── TV (webOS) ──────────────┐
│ HBC ランチャ → IPK app                 │
│                                        │
│  ┌─ omt-monitor (aarch64) ───────────┐ │
│  │ net thread: ReceiverSession       │ │
│  │   ├ video: vmx decode (NEON)      │ │
│  │   ├ audio: PCM ring buffer        │ │
│  │   └ stats: fps/latency/drops      │ │
│  │ decode thread pool (rayon)        │ │
│  │ present thread: GLES2 + EGL       │ │
│  │ UI: キー入力 / OSD                │ │
│  └───────────────────────────────────┘ │
│        │ bridge IPC (shared mem)       │
│  ┌─────▼───── 32bit side ───────────┐  │
│  │ libEGL.so / libGLESv2.so (PVR)   │  │
│  │ Wayland surface                  │  │
│  └──────────────────────────────────┘  │
└────────────────────────────────────────┘
```

### スレッドモデル
- **recv thread**: TCP 受信 → フレーム組み立て。ロックフリーリングで decode へ
- **decode**: `vmx::Codec` をスレッドローカルに持つ（rayon でスライス並列）
- **present**: EGL コンテキストを持つ単一スレッド。vsync 同期、フレーム落ちは最新フレーム優先
- **audio**: SDL audio callback / ALSA。映像 PTS との差分でリングバッファ長を調整

### 表示経路の選択肢（M0 で決定）

| 案 | 内容 | 長所 | 短所 |
|---|---|---|---|
| A. aarch64 + bridge-64to32 + raw EGL/Wayland | 64bit で NEON、EGL は bridge 経由 | VMX 最速 | SDL が使えない可能性、入力処理を自前で |
| B. aarch64 + bridge + SDL3(aarch64 ビルド) | SDL-webOS を 64bit でビルド | 入力/音声/ウィンドウが楽 | SDL の 64bit webOS ビルドが未検証 |
| C. armv7 + system SDL2 | 32bit そのまま | 最短で動く | VMX が scalar → 1080p30 も危うい |

**推奨**: A または B。C は「動くプロトタイプ」用のフォールバック。

---

## 4. 技術選定

| 領域 | 採用 | 理由 |
|---|---|---|
| 言語 | Rust | `openmediatransport-rs` / `vmx-rs` がそのまま使える。NEON 実装が既にある |
| OMT | `openmediatransport-rs` (crates.io) | 純 Rust・DNS-SD・VMX1 BGRA 受信に対応 |
| コーデック | `vmx` crate | libvmx とバイト互換。aarch64 で NEON |
| 表示 | GLES2 (EGL) + GL_TEXTURE_2D | webOS の実績経路。BGRA → swizzle 不要 |
| 音声 | SDL audio or ALSA (16bit PCM) | OMT は PCM。ALSA 直叩きが軽い |
| 入力 | Magic Remote (wayland/uinput) | M0 で確認。最悪 IR リモコンキーイベントを HBC 経由で |
| パッケージ | `ares-package` (IPK) | HBC 互換 |
| CI | GitHub Actions + webOS SDK buildroot コンテナ | 再現性 |

---

## 5. マイルストーン

### M0: 実行基盤スパイク（1〜2 日）
- [ ] `webos-bridge-64to32` IPK を導入し、aarch64 の hello-world が動く
- [ ] GLES2 で三角形が出る（EGL サーフェス取得まで）
- [ ] `vmx` の 1080p デコード速度を実測（NEON / scalar、rayon 有無）
- [ ] 1080p60 / 1080p30 / 720p60 の decode fps 表を作る
- [ ] OMT ソース（vMix or `omt-sender`）から 1 ストリーム受信し、フレーム取得を確認

**Exit criteria**: 1080p30 で decode ≥ 45 fps、1080p60 の可否を数値で判断できる。
NG なら C 案（armv7）にフォールバックし、720p60 / 1080p30 を上限とする。

### M1: 発見・接続・統計（2 日）
- [ ] DNS-SD でソース一覧取得（`_omt._tcp`）
- [ ] 手動 IP 入力での接続
- [ ] 受信統計（フレーム数 / bitrate / 最終受信時刻）をテキスト表示
- [ ] 切断検知と自動再接続

### M2: 映像表示（3 日）
- [ ] BGRA フレーム → GL テクスチャ → フルスクリーン描画
- [ ] アスペクト比保持、レターボックス
- [ ] 1080p30 で 30 分連続再生（メモリリークなし）

### M3: 音声・同期（2 日）
- [ ] PCM 出力、映像 PTS との同期（±40 ms 以内）
- [ ] 無音時のアンダーラン処理

### M4: UI / OSD（2 日）
- [ ] ソース選択画面（Magic Remote の上下左右 + OK）
- [ ] OSD: 解像度 / fps / bitrate / 遅延 / ドロップ / 再接続回数（トグル）
- [ ] 情報オーバーレイの焼き込みなし（スクリーンショット用途を考慮）

### M5: 性能改善（2〜3 日）
- [ ] フレームバッファ 2 枚 + 最新フレームドロップ方式
- [ ] `RAYON_NUM_THREADS` 調整（A55 ×4）
- [ ] root 時のみ: CPU governor を performance に固定するオプション
- [ ] 1080p60 の可否判定とデフォルト設定への反映

### M6: パッケージ・配布（2 日）
- [ ] `packaging/` に appinfo.json / icon / IPK ビルド
- [ ] HBC リポジトリ用 `apps.json` + manifest.json
- [ ] GitHub Actions で IPK 自動ビルド & リリース添付
- [ ] README に導入手順（bridge-64to32 → 本アプリ）

---

## 6. リスク

| リスク | 影響 | 対策 |
|---|---|---|
| VMX1 デコードが A55 で遅い | 60p 断念 | M0 で実測。720p60 / 1080p30 に落とす。プレビュー(1/8)モード併用 |
| bridge-64to32 が webOS 10.3.2 で不安定 | 実行基盤が崩れる | M0 で最初に検証。ダメなら 32bit + scalar で機能限定版 |
| SDL の aarch64 ビルドが通らない | 入力/音声を自前実装 | raw wayland + ALSA に切替（B→A） |
| 32bit soft-float ABI の地雷 | 32bit 側がリンクできない | `armv7-unknown-linux-gnueabi` を使用、`-C target-feature=+neon` は付けない |
| メモリ不足 (400MB) | OOM / カクつき | バッファプール上限を設定、4K は受けない |
| Magic Remote のキー取得 | UI 操作不能 | uinput / wayland seat を M0 で確認。最悪 HBC のリモコン API |
| OMT 音声 API の完成度 | 音が出ない | 映像先行（M2）で切離し、M3 で対応 |

---

## 7. 未解決の問い（スパイク対象）

1. `webos-bridge-64to32` 上で EGL コンテキストを自前生成できるか（RetroArch はどうやっているか）
2. `openmediatransport-rs` の audio receive API は現状どこまで実装済みか
3. VMX1 の 1080p60 デコードに必要な実効スループット（NEON 1 コア vs 4 コア）
4. 1080p BGRA を GL に上げる際の帯域（8.3MB × 60 = 500MB/s）が A55 で耐えるか
5. HBC アプリとして root なしで動くか（root 前提にしない）

---

## 8. テスト計画

- **レイテンシ測定**: タイムコード焼き込みソース（vMix / omt-sender）を送出し、TV 画面を 240fps カメラで撮って差分を測る
- **ソーク**: 8 時間連続受信でメモリ増加・再接続回数を確認
- **回線変動**: `tc netem` で 1% ロス / 20ms ジッタを注入し、復帰挙動を確認
- **解像度マトリクス**: 720p60 / 1080p30 / 1080p60 / 1080p59.94
- **A/V 同期**: クリック音 + フラッシュの同時提示で ±40ms 判定

---

## 9. リポジトリ構成（予定）

```
app/                    Rust アプリ本体
  src/main.rs           エントリ / 状態機械
  src/omt.rs            Discovery + ReceiverSession
  src/video.rs          vmx decode + フレームプール
  src/present_egl.rs    EGL/GLES2 プレゼンタ
  src/audio.rs          PCM 出力
  src/ui.rs             ソース選択 / OSD
packaging/
  appinfo.json
  icon.png
  build-ipk.sh
  webosbrew/apps.json
docs/
  IMPLEMENTATION_PLAN.md
.github/workflows/build.yml
```

---

## 10. 参考

- OMT 公式: https://github.com/openmediatransport
- OMT Rust 実装: https://github.com/MikanseiLaboratory/openmediatransport-rs
- VMX1 Rust: https://github.com/MikanseiLaboratory/vmx-rs
- HBC: https://github.com/webosbrew/webos-homebrew-channel
- bridge: https://github.com/webosbrew/webos-bridge-64to32
- SDL for webOS: https://github.com/webosbrew/SDL-webOS
- 参考実装（ネイティブ webOS アプリ）: https://github.com/mariotaku/moonlight-tv
