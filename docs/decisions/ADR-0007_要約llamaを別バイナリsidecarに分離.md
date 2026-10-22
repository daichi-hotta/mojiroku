# 0007. ローカル要約 llama.cpp を別バイナリ sidecar に分離（whisper との ggml 衝突）

- ステータス: 採用（**ADR-0003 の「whisper.cpp と llama.cpp を Rust 単一バイナリに in-process 同居」を一部訂正**）
- 日付: 2026-06-24

## Context

ADR-0003 は STT(whisper.cpp) と要約(llama.cpp) を **同一 Rust バイナリに in-process で同居**させる前提だった。
実装して検証した結果、この前提は誤りと判明した：

- **whisper.cpp と llama.cpp は、それぞれが ggml を vendoring**しており、同名のシンボル（`ggml_*` 等）を持つ。
- 両方を 1 つのバイナリにリンクすると、macOS ではリンクは通る（リンカが一方の ggml を採用）が、
  **実行時に whisper が llama 側の ggml を呼んで壊れる**。実測で、llama-cpp-2 を mojiroku-core に追加した
  途端、同じ音声で whisper が **0 セグメント**（空）になった。llama を外すと即復旧。
- これは ggml の既知問題（ggml ソースが ggml/whisper.cpp/llama.cpp 間で手動 sync されるため重複シンボルになる）。
  参照: llama.cpp#9267 / ggml#1148 / whisper.cpp#1887。

並行して**品質ゲート**を実施: 実会議（Google Meet 録音 15 分）を whisper で文字起こしし、
ローカル `Qwen2.5-7B-Instruct Q4_K_M`（llama.cpp/Metal）で議事録を生成 → **議題・決定事項・担当者付き
アクションアイテム（実名＋期限）を正確に抽出する実用品質**を確認（約 40 秒、完全ローカル）。
→ この llama.cpp 級の品質を維持したい。

## Decision

- **STT(whisper) は従来どおり mojiroku-core / 本体アプリに in-process** で残す。
- **ローカル要約(llama.cpp) は別バイナリ `crates/mojiroku-llm`（sidecar）に分離**する。Tauri の externalBin として
  同梱し、要約時に spawn して **stdin（プロンプト）→ stdout（要約）** で受け渡す。各バイナリは ggml を 1 つしか
  リンクしないため衝突しない。
- **BYOK 要約（OpenAI/Anthropic, ureq）は ggml 非依存**なので mojiroku-core に in-process のまま残す。
- 候補だった **B: candle（純Rust）は不採用**（品質ゲートで local llama 品質が十分 → llama.cpp 級を優先。candle の
  量子化Qwen on Metal 成熟度リスクを取らない）。

## Consequences

- ✅ whisper + llama を**プロセス分離**で共存。**全 Rust・ローカル完結・$0 は維持**（小さな Rust sidecar であり、
  却下した Python sidecar とは別物 — PyInstaller/torch/localhost 無し）。
- ✅ llama.cpp のリファレンス品質を維持。
- ⚠️ 要約ごとに sidecar を spawn しモデルをロードする（会議 1 回なら許容）。常駐デーモン化は将来の最適化、
  必要になるまで作らない。
- ⚠️ sidecar の各 OS 向け同梱/署名（Tauri externalBin が triple 付き命名を処理）。
- **Phase 2 の sherpa-onnx は onnxruntime（ggml ではない）**ので whisper と衝突しない見込みだが、同様に実機検証する。

## 関連して確定した実装上の注意

- **UTF-8 トークン復号**: `token_to_str` をトークン単位で呼ぶと日本語がトークン境界で壊れる。
  `token_to_bytes` でバイト蓄積し、最後に UTF-8 デコードする（sidecar で対応）。
- **VAD 必須**: whisper は無音区間で「ご視聴ありがとうございました」等をハルシネーションする。
  Silero VAD で無音を除去する（Phase 1a の改善として別途。spec の VAD 段）。
- **プロンプトの受け渡しと上限時間**（2026-10-22）: プロンプトは引数でなく一時ファイルのパスで渡す。ファイルは
  ライブ翻訳と同じ `PromptFile`（新規作成のみ・0600・使い終わると削除）で作る。要約とタイトル生成の sidecar は
  60 分（`LOCAL_LLM_TIMEOUT`）で止める。測った値ではなく、固まった sidecar が重い処理の許可（`HEAVY_ML_JOB`）を
  持ち続けないための上限。止めたときもプロセスの終了を待ってから許可を返す。
