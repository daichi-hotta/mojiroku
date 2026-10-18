# mojiroku 仕様書（技術仕様）

> ステータス: ドラフト（初版） / 最終更新: 2026-06-24
> 関連: [要件定義書](./requirements.md) ・ [アーキテクチャ](./architecture.md) ・ [ADR](./decisions/)
> アーキテクチャは **案B（Rust 単一ランタイム）**。Python / サイドカー / localhost HTTP は一切持たない。

---

## 1. システム構成図

```
┌──────────────────────────────────────────────────────────┐
│  Tauri v2 デスクトップアプリ (mojiroku)                     │
│  ┌───────────────┐   invoke / event   ┌─────────────────┐ │
│  │ Vite + React  │◄──────────────────►│ Rust コア        │ │
│  │ UI (静的出力)  │  (IPC, HTTP なし)   │ ・Tauri commands │ │
│  └───────────────┘                    │ ・音声取り込み    │ │
│                                       │ ・ML パイプライン │ │
│   全 ML を in-process / Rust で実行 ──►│  whisper-rs      │ │
│   (サイドカーなし・localhost なし)      │  sherpa-onnx-rs  │ │
│                                       │  llama-cpp-2     │ │
│                                       └────────┬─────────┘ │
└────────────────────────────────────────────────┼──────────┘
                                                 ▼ (BYOK 選択時のみ)
                              OpenAI・Anthropic API (任意, reqwest)
```

- UI（`frontend`）は Tauri の `invoke`（要求）/ `event`（進捗）でコアと通信。HTTP サーバーは存在しない。
- ML（`crates/mojiroku-core`）はアプリプロセス内で実行。STT=ggml/Metal、diarization=ONNX/CoreML、要約=ggml/Metal。
- 外部送信は BYOK 選択時のみ（`reqwest` で OpenAI/Anthropic）。

## 2. 技術スタック一覧

| 層 | 採用 | 備考 |
|---|---|---|
| フロント | Vite, React, TypeScript, Tailwind CSS v4 | Next.js 不採用（[ADR-0006](./decisions/ADR-0006_フロントはViteでNextは不採用.md)）。shadcn/zustand は未導入（素の React state） |
| シェル | Tauri v2（Rust） | ウィンドウ/権限/配布/音声取り込み |
| ML コア | Rust: `whisper-rs`, `sherpa-onnx`(-rs), `llama-cpp-2` | すべて in-process |
| VAD | whisper.cpp 内蔵 Silero VAD（ggml, `WhisperVadContext`） | 無音区間除去。whisper-rs の `state.full()` は内蔵VADをバイパスするため独立適用（[ADR-0008](./decisions/ADR-0008_VADはwhisper内蔵Sileroを独立適用.md)） |
| 永続化 | SQLite（`rusqlite` or `sqlx`） | 履歴・設定 |
| BYOK 通信 | `reqwest` | OpenAI/Anthropic |
| キー保管 | `keyring`（OS キーチェーン） | BYOK API キー |
| 型共有 | `ts-rs` 等を検討 | Rust→TS の型生成 |
| モデル | whisper ggml / silero VAD ggml / pyannote seg-3.0 ONNX / 要約 GGUF | バージョンは models で管理 |

> バージョンは次フェーズの scaffold 時に固定し、本表に追記する。

## 3. プロセス構成・ライフサイクル

- **単一プロセス**。重い ML はワーカースレッド / 非同期タスクで実行し、UI スレッドをブロックしない。
- ジョブは**キャンセル可能**。長時間処理（large-v3 の文字起こし等）は中断・再開を考慮。
- 旧案のサイドカー spawn / 監視 / 再起動 / localhost ポート確保は**存在しない**（攻撃面・配管の削減）。

## 4. UI ↔ コア IF（IPC）

Tauri コマンド（`#[tauri::command]`）と `event` で接続する。代表例：

| 種別 | 名前 | 概要 |
|---|---|---|
| command | `start_job(input, options)` | 文字起こし/要約ジョブを起動。`job_id` を返す |
| command | `cancel_job(job_id)` | 実行中ジョブの中断 |
| command | `get_history()` / `get_recording(id)` | 履歴一覧 / 詳細取得 |
| command | `export(recording_id, format)` | md/txt/docx 出力 |
| command | `set_provider(config)` / `set_api_key(provider, key)` | 要約プロバイダ・BYOK 設定 |
| event | `job://progress` | `{job_id, stage, percent}` の進捗ストリーム |
| event | `job://done` / `job://error` | 完了 / エラー通知 |

- リクエスト/レスポンスの型は Rust 側を正とし、`ts-rs` 等で TS 型を生成して齟齬を防ぐ。

## 5. データモデル

履歴・エクスポート・要約出力まで含めたルートエンティティを定義する（話者分離対応を**最初から**内包）。

```
Recording (=Session, ルート)
  ├─ id, source_type(file|mic|live), audio_meta(duration, sample_rate, ...), created_at
  ├─ Transcript
  │    └─ Segment[]  { start, end, text, speaker_id }   ← 話者分離対応の中核
  ├─ Speaker[]       { id, label, display_name }
  ├─ Summary (=Minutes)  { template_id, content, action_items[] }
  │    └─ ActionItem[]   { text, assignee?, due? }
  └─ Job             { id, recording_id, stage, status, progress }

SummaryTemplate      { id, name, prompt, kind(minutes|summary|action_items) }
```

- `Segment.speaker_id` を Phase 1 から持たせることで、Phase 2 の話者分離導入時に**スキーマの retrofit が不要**。
- Phase 1 では `speaker_id` を単一話者/未割当で埋め、Phase 2 で実値を付与する。

## 6. 処理パイプライン

```
音声取り込み → VAD(whisper.cpp 内蔵 Silero/ggml) → STT(whisper-rs) → [Phase2] diarization(sherpa-onnx)
            → 話者マージ(セグメント↔話者ターンの整合) → 要約(llama-cpp-2 or BYOK)
```

- ジョブキューで逐次/並行を制御し、各段で `job://progress` を発行。
- **モデル load/unload ライフサイクル**: STT 実行 → モデル解放 → 要約モデルをロード、と順に常駐を切り替え、8GB Mac でも同時常駐メモリを抑える（§10）。
- **話者マージ**（受容したトレードオフ）: whisper のセグメント/word タイムスタンプと sherpa の話者ターンを時間軸で突き合わせる。境界ズレの扱いを実装で詰める（[ADR-0005](./decisions/ADR-0005_STTエンジンにwhisper-cppを採用.md)）。

## 7. 音声取り込み設計

- **ファイル**（Phase 1）: Tauri のファイルダイアログ→デコード（ffmpeg 同梱 or symphonia）。
- **マイク**（Phase 3）: OS マイク権限、録音 UI、PCM 取得。
- **ライブ取り込み**（Phase 4）: OS 別のシステム音声キャプチャ。macOS は ScreenCaptureKit / Core Audio（仮想オーディオデバイス不要の方向を優先）、Windows は WASAPI ループバック。**各 OS で単独の設計課題**として扱う。

## 8. 要約プロバイダ抽象化

- 既定 = 同梱 `llama-cpp-2`（GGUF, in-process, 完全ローカル）。
- 共通トレイト `SummarizeProvider` で実装を切替: `Local(llama)`, `OpenAI(BYOK)`, `Anthropic(BYOK)`, `Ollama(検出時のみ)`。
- BYOK キーは **OS キーチェーン（keyring）** に保管（平文設定ファイルに置かない）。
- 📌 **宿題: 既定モデル選定**。小型ローカル GGUF の**日本語議事録品質**を実機検証して決定する。size ↔ quality ↔「ただ動く（DL サイズ/速度）」の三すくみ。これがプロダクト体験の下限を決める。

## 9. 話者分離設計（Phase 2）

- sherpa-onnx を使用し、**pyannote segmentation-3.0 の ONNX 重みのみ**を採用する（**full pyannote.audio パイプラインではない**）。これに話者埋め込みモデル＋クラスタリングを onnxruntime だけで組み合わせ、オフライン・torch なしで話者分離する。
- 📌 **宿題（B/C 分岐の真の肝）: 会議は話者数が未知**。sherpa-onnx のオフライン diarization は「話者数既知」または「クラスタリングしきい値」を要求する → **しきい値ベースのクラスタリングが必須**で、**このしきい値が日本語会議での実用可否を決める**（生 DER の数字より重要）。実機チューニング項目として明記。
- **案C フォールバックの切替点**: 日本語品質がしきい値調整でも不足する場合、torch pyannote(full pyannote.audio) を**薄い Python サイドカーで Phase 2 のみ遅延起動**する。whisper/llama は Rust ネイティブのまま、Phase 1 は Python ゼロを維持。切替は diarization トレイトの実装差し替えで吸収する（[ADR-0004](./decisions/ADR-0004_話者分離はsherpa-onnxで実現.md)）。

### 9.1 発言単位の話者訂正（Issue #19 増分1）

話者を変える手段は 2 つあり、**粒度が違う**。

| 操作 | 対象 | コマンド |
|---|---|---|
| 改名 | **クラスタ全体**（S1 の表示名を「田中」に） | `rename_speaker` |
| **訂正** | **発言 1 件**（この発言だけ S2 → S1） | `set_segment_speaker` |

訂正は `segments.speaker_id` を 1 行だけ更新する。対象は `(recording_id, idx)` で指す。
`idx` は `insert_segments` が `enumerate()` で採番した連番で、`Segment.idx` として API 境界に出る。

**戻り値は「実際に変えたか」**（`Result<bool>` → コマンド → `invoke<boolean>`）。
同じ話者を選び直したときは `false` を返し、発言も要約も変えない（下の「手で確かめた発言」の
行だけは消す）。UI はこれを見て「要約が古い」の表示を
出し分ける — 真偽を知っているのはコアだけなので、**UI 側で現在値と比較させない**
（UI の値が DB と一致している前提に依存してしまう）。

制約:

- **当該録音の `speakers` に無い id は拒否する。** 許すと `speakers` の id 集合と
  `segments.speaker_id` の集合がズレ、改名 UI に出ない話者が発言側だけに生まれる。
- **移動元の話者行は消さない。** 最後の 1 発言を移して発言ゼロになっても、
  `speakers` 行・声紋・ライブラリ紐づけを残す（訂正を戻せるようにするため）。
  書き出しヘッダーの話者一覧は `speakingSpeakerNames`（`frontend/src/lib/types.ts`）で
  実際に発言している人だけに絞る。**訂正モーダルと SpeakerPanel は絞らない**
  （発言ゼロになった話者を選び直せないと訂正が戻せない）。
- **実際に変えたときだけ要約を stale にする。** 要約本文に話者名が出るため。
  同値なら立てない — ローカル要約の作り直しは分単位かかるので、内容が
  変わっていないのに促すのは害。
- `rec_fts` は触らない。body は `segment.text` のみで話者を含まない。
- **エラーは `error.` 始まりのキーで返し、コマンド層の `core_err` が `CoreError` の
  Display 接頭辞を外す。** キーをコア側の文字列に詰めただけでは `db error: ` が前置され、
  フロントの `translateError`（最初の `": "` でキーを切り出す）に掛からず、
  日本語 UI に英語がそのまま出る。

#### 訂正の記録と、話者分離のやり直し（ADR-0048）

訂正は `segments.speaker_id` の更新と同時に `speaker_corrections` へ 1 発言 1 行で残す
（スキーマ v8）。

| 列 | 中身 |
|---|---|
| `predicted` | 話者分離が付けた話者。同じ発言を何度直しても最初の値を残す |
| `corrected` | 利用者が選んだ話者（`NULL` は話者不明に戻した） |

`predicted` と同じ話者を選び直すと行を消す（訂正の取り消し）。文字起こしを入れ直すと
`idx` が別の発言を指しうるので、`replace_transcript` が全行を消す。

**話者分離をやり直しても訂正は引き継ぐ。** 本文は変わらないので `idx` はそのまま使える。
変わるのは話者 id だけなので、選んだ話者 X の新しい id を次の順で決める
（`correction::carry_corrections`）。

1. X の**直していない**発言が、やり直しで付いた話者の多数決（直した発言は票に入れない。同数なら id の小さい方）
2. 1 で決まらなければ、改名の引き継ぎと同じ声紋の対応（`match_speaker_ids`）
3. 会議の自分（`self`）は再分離しないので、そのまま

どれでも決まらない発言と、選んだ話者と元の予測（利用者が否定した話者）が同じ新しい話者に
まとまった発言は**未割当（「?」）に戻し**、件数をジョブ完了の通知
（`JobUpdate.unmapped_corrections`）で画面に出す。利用者が一度否定した話者分離の結果に
黙って戻さないためである。残った行は新しい id で書き直し、`predicted` はやり直しの結果になる。
やり直しの結果が利用者の選択と一致した行も消さない（手で確かめた発言として残す。
取り消しで行を消すのは、利用者が自分で `predicted` を選び直したときだけ）。手で確かめた発言では
`predicted` が今の話者なので、今の話者を選び直すと行だけが消える（発言と要約は変えない）。
否定した話者が無いので、次のやり直しで「否定した話者とまとまった」として未割当にもしない。

`segments` を書き換える経路を新しく足すときは、`speaker_corrections` をどうするかを決めること。

## 10. モデル管理

- 初回 DL・キャッシュ（ユーザーデータディレクトリ）・モデル選択 UI。
- 各 OS 向け ONNX Runtime / ggml バイナリの同梱方針。
- **メモリ予算**: モデルごとのフットプリントを把握し load/unload を制御。
- **総フットプリントと初回 DL 体験**を数値で規定（whisper ≈1〜3GB / 要約 GGUF 数GB / sherpa ≈数百MB）。同梱 vs 初回 DL の選択は §14 と連動。

## 11. フロントエンド設計

- 画面: ホーム/取り込み → 処理中（進捗）→ 結果（文字起こし＋議事録の二面）→ 履歴 → 設定。
- 状態管理は当面素の React state（zustand は未導入。必要になれば追加）。Tauri `invoke`/`event` クライアントを `lib/` に集約。
- 型は Rust から生成（§4）。

## 12. 永続化

- SQLite（`rusqlite` 想定）。`Recording` をルートに履歴・`Summary`・`Segment` を保存。
- 保存先は OS のアプリデータディレクトリ。音声原本の保持/破棄はプライバシー設定で選択可能に。

## 13. セキュリティ・権限

- Tauri v2 capabilities で最小権限を宣言（ファイル/録音/画面収録）。
- BYOK キーは keyring。**localhost HTTP を持たないため、旧案のサイドカー認証（localhost 攻撃面）が不要**＝攻撃面が縮小。

## 14. ビルド・配布

- 単一 Tauri バンドル（各 OS）＋モデルファイル。署名/公証（macOS notarization 等）。
- **PyInstaller / torch なしで「凍結地獄」が消える**（[ADR-0003](./decisions/ADR-0003_MLをRust単一ランタイムに集約.md)）。
- 残課題: モデルを**同梱 vs 初回 DL** のどちらにするか、各 OS の ONNX/ggml バイナリ同梱。

## 15. エラーハンドリング・ロギング

- ジョブ失敗（モデル未 DL / メモリ不足 / 音声デコード失敗 / BYOK 認証失敗）をユーザーに分かる形で提示。
- ローカルログ（回転）。クラッシュ時の安全な復帰（履歴は破損させない）。

## 16. テスト戦略

- frontend: Vitest（純ロジック・ストア）。
- `mojiroku-core`: `cargo test`。短いモック音声で STT/話者マージ/パイプラインを単体テスト。
- `src-tauri`: コマンドの結合テスト。
- diarization しきい値・既定 GGUF 品質は、テストとは別に**実機評価スパイク**で測る。
