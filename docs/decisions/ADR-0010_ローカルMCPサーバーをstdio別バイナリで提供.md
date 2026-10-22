# 0010. ローカル MCP サーバーを stdio 別バイナリで提供（履歴 DB を読み取り専用公開）

- ステータス: 採用 (the "Tauri externalBin には登録しない" part alone was withdrawn on 2026-09-03; see the addendum under Consequences)
- 日付: 2026-06-26

## Context

北極星の差別化の核は「**MCP で Claude 等から自分のローカル議事録を検索・参照**できる」こと（[roadmap](../roadmap.md)）。
beta-1（録音→文字起こし→要約→履歴/検索）と Phase 2b（話者分離）で、履歴は `SqliteStore`
（`mojiroku.db`, WAL, FTS5 trigram）に永続化され、読み取り API
（`list_recordings` / `get_recording_detail` / `search_recordings`）が揃った。これを MCP で外部に出す。

設計上の論点は「**誰がサーバを起動し、どこで動くか**」と「**履歴 DB をどう安全に読むか**」。

## Decision

**`crates/mojiroku-mcp` = stdio MCP サーバの別バイナリ**を新設し、`rmcp`（公式 Rust SDK, 1.8.0）で実装する。
履歴 DB を**読み取り専用**で開き、3 ツール（`search_meetings` / `get_meeting` / `list_recent_meetings`）を公開する。

- **stdio・別バイナリ**: MCP クライアント（Claude Desktop / Claude Code）が stdio で spawn する。
  HTTP を持たない（案B「localhost HTTP は無い」を維持・$0）。アプリ非起動でも履歴 DB を読める。
  - ローカル要約 sidecar（[ADR-0007](./ADR-0007_要約llamaを別バイナリsidecarに分離.md)）と違い、**起動者は MCP クライアント**で
    アプリではない。The original rule here was "do not register it as a Tauri `externalBin`". **Withdrawn on
    2026-09-03**: it is now registered as an `externalBin` purely for signing and notarization, while the app
    still never spawns it (see the addendum under Consequences). `scripts/build-sidecar.sh` builds it and
    places it at `src-tauri/binaries/mojiroku-mcp-<triple>`; the bundled copy is `Contents/MacOS/mojiroku-mcp`.
  - `rmcp` は純 Rust（onnxruntime/ggml 非依存）なので [ADR-0007](./ADR-0007_要約llamaを別バイナリsidecarに分離.md) の
    ggml シンボル衝突とは無関係。要約 sidecar とも別プロセスで干渉しない。

- **読み取り専用 open（`SqliteStore::open_readonly`）**: アプリ本体（writer）と MCP（reader）は
  別々に更新され得る（**バージョン skew**）。リーダーが `open()`（= `migrate()` + `journal_mode` 書き込み）を
  呼ぶと、アプリ所有 DB を勝手にマイグレートしてしまう。これを避けるため:
  - `migrate()` も `journal_mode` 設定も**呼ばない**（スキーマを一切触らない）。
  - `PRAGMA user_version` を検査し、DB が**バイナリの理解より新しい**ときは誤読を避けて `Err`。古い分
    （v2 等）は読める範囲で許容（`speakers` 表が無い場合は存在確認して空 Vec フォールバック）。
  - OS レベルでは **read-write ハンドル**で開く（書き込みクエリは発行しない）。**理由**: 厳格な
    `SQLITE_OPEN_READ_ONLY` は WAL の `-shm`/`-wal` を生成できず、アプリ非起動時に開けないことがあるため。
    実測（`-wal` 1.5MB が未チェックポイントの実 DB）でアプリ閉状態でも読めることを確認。

- **ツール出力は LLM コンテキスト向けに整形**（UI 向けではない）:
  - `search_meetings(query)` → ヒット配列（title / created_at / **snippet** / recording_id）。全文は返さない。
  - `get_meeting(recording_id, include_transcript=false)` → **既定は要約＋メタ＋話者（表示名）**。
    逐語全文（実会議で 18,000 字超）は `include_transcript=true` の時だけ（`summarize::transcript_to_text`
    の話者ラベル付き整形を再利用）。
  - `list_recent_meetings(limit=20)` → 最近の会議メタ。
  - いずれも **裸の UUID でなく title/date/話者表示名**を含め、モデルが会議を引用できるようにする。

- **stdout 純度**: stdio では stdout が JSON-RPC チャネル。ログ（tracing）と panic hook は **stderr** に固定。
  raw `initialize`→`tools/list`→`tools/call` ハンドシェイクで stdout が純 JSON-RPC のみであることを実測確認。

## Consequences

- 利用者は MCP クライアント設定にバイナリ絶対パスと DB パスを書く（[docs/mcp.md](../mcp.md)）。
  beta は**文書化のみ**（設定ジェネレータや UI トグルは将来）。
- `.app` への同梱（bundle resources）と署名付き配布は将来。当面は dev/release バイナリを直接指定する。
  - **2026-09-03 追記: 同梱した（Issue #63）。**ただし bundle resources ではなく **externalBin**。理由は署名——
    externalBin は llm sidecar と同じ経路で hardened runtime 署名＋公証され、release.yml が
    `Contents/MacOS/` の各バイナリの runtime フラグを検証する（v0.4.0 から実績あり）。
    bundle resources に置いた Mach-O が同様に署名される保証は取れなかった。
    「アプリは spawn しない」は変わらない: webview には shell の許可を一切与えておらず
    （`capabilities/default.json`）、`mojiroku-llm` を起動するのは Rust 側のコードだけなので、
    externalBin に足しても起動経路は増えない。
    利用者は `/Applications/mojiroku.app/Contents/MacOS/mojiroku-mcp` を指定する。
- 書き込み系ツール・認証・複数 DB は対象外。読み取り専用に限定することで安全側に倒す。

## 検証

- `cargo test -p mojiroku-core`（`open_readonly` の no-migrate / 未来スキーマ Err / v2 互換読み取り）。
- 実 DB（54 分会議含む 5 録音）に対し stdio ハンドシェイクで 3 ツールを実行 → 検索ヒット・
  要約/話者/全文取得・不存在 id のエラー応答・stdout 純度をすべて確認。
- 実 MCP クライアント（Claude Code）登録での E2E。
