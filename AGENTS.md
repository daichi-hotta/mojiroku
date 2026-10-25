# AGENTS

このリポジトリで作業する AI エージェント（Claude Code / Cursor / Codex など）向けの、
ツール非依存の共通入口。**このファイルが唯一の正本。**
2026-09-19 に `CLAUDE.md` を統合して廃止した。詳細設計は `docs/` を見ること。

## これは何

**ローカル完結・基本無料の「Notion AI ミーティングノート」代替デスクトップアプリ。**
録音/音声ファイル → 文字起こし → 要約・議事録。すべてローカル推論（サーバー維持費 $0）。
北極星と全体計画は [docs/roadmap.md](./docs/roadmap.md)。

## 最優先ルール：文書とデータの置き場

正本は **repo / GitHub Issues・Projects / ローカル＆配布物** の3層に分かれる。
repo は「全部の家」ではない。

- **repo（ここ）= コード＋確定文書**：`crates/` `frontend/` `src-tauri/` `landing/`
  `workers/` のコード、`docs/`（要件・仕様・設計・運用・ADR）、`LICENSE` / `NOTICE` / `CLA.md`。
- **GitHub Issues / Projects = タスクと進行中の議論**：これからやること、バグ、機能要望、
  設計の検討中スレッド。**確定したら ADR か `docs/` に落とし、Issue からはリンクする。**
- **大容量バイナリは repo に置かない**：モデルは**実行時 DL**
  （`~/Library/Application Support/com.daichi0812.mojiroku/models/`）、sidecar / `.app` /
  `.dmg` は**ビルド成果物**、配布は公開 `mojiroku-releases` の Releases。
  `*.gguf` `*.bin` `*.onnx` `src-tauri/binaries/` は gitignore。

→ 迷ったときの決定表：[`docs/README.md`](./docs/README.md)

## アーキテクチャ（案B: Rust 単一ランタイム）

- **デスクトップ**: Tauri v2（`src-tauri/`）
- **フロント**: Vite + React + TS + Tailwind v4（`frontend/`、状態は素の React state。zustand/shadcn は未導入）。Next.js は使わない（ADR-0006）。UI ↔ コアは Tauri の `invoke`/`event`（localhost HTTP は無い）。
- **ML コア**: `crates/mojiroku-core`（UI 非依存・単体テスト可能）
  - STT: `whisper-rs`（whisper.cpp / Metal）
  - 要約: **別バイナリ sidecar** `crates/mojiroku-llm`（llama.cpp / Metal）＋ BYOK（OpenAI/Anthropic, ureq）
  - 話者分離: sherpa-onnx（pyannote seg-3.0 ONNX, torch なし。ADR-0004/0009）
  - VAD: whisper.cpp 内蔵 Silero を `WhisperVadContext` で独立適用
  - 音声デコード: symphonia + rubato（→ 16kHz mono f32）
  - 永続化/検索: `SqliteStore`（rusqlite, WAL, FTS5 trigram。録音/文字起こし/要約/話者/**ジョブ**（v5・ADR-0024））; v6 adds `recordings.mic_offset_ms`, the meeting mic-vs-system start offset (Issue #65)
- **MCP サーバー**: 別バイナリ `crates/mojiroku-mcp`（rmcp stdio）。履歴 DB を read-only で公開し Claude 等から議事録を検索・参照（ADR-0010）。**Since 2026-09-03 it is registered as a Tauri `externalBin` and shipped inside the .app as `Contents/MacOS/mojiroku-mcp`** (Issue #63, PR #64), purely so it gets the same hardened-runtime signing and notarization as the LLM sidecar. The app itself never spawns it: the launcher is the MCP client (Claude Desktop / Claude Code), and `src-tauri/capabilities/default.json` allows `shell:allow-execute` for `mojiroku-llm` only. Do not remove the `binaries/mojiroku-mcp` entry from `tauri.conf.json`; `docs/mcp.md` points users at the bundled path.
- **配布**: `mojiroku.com`（Astro → Cloudflare Workers 静的アセット, `landing/`）から **Developer ID 署名+notarization 済み .dmg**（公開 `mojiroku-releases` repo の Releases。ADR-0011/0022）。署名は CI のみ（env 駆動）でローカルビルドは無署名のまま。サーバー費 $0。

## ⚠️ 重要な制約・落とし穴（先に読む）

- **whisper.cpp と llama.cpp は同一バイナリに同居できない**（ggml シンボル衝突。リンクは通るが実行時に whisper が壊れて 0 セグメントになる）。だから要約 llama は `crates/mojiroku-llm` の**別バイナリ sidecar**に分離している（[ADR-0007](./docs/decisions/ADR-0007_要約llamaを別バイナリsidecarに分離.md)）。`mojiroku-core`（whisper を含む）に llama-cpp-2 を**足さないこと**。
- **whisper-rs の `state.full()` は whisper.cpp 内蔵 VAD をバイパスする**（VAD は `whisper_full` 側にあり `whisper_full_with_state` には無い）。よって VAD は `WhisperVadContext` で speech 区間を抽出 → 無音除去 PCM を whisper に渡し、タイムスタンプを元時刻へ再マッピングする（[ADR-0008](./docs/decisions/ADR-0008_VADはwhisper内蔵Sileroを独立適用.md)）。
- **sidecar バイナリはビルド成果物**。`src-tauri/binaries/mojiroku-llm-<triple>` は gitignore。`scripts/build-sidecar.sh`（= `just dev`/`just build` が自動実行）で生成する。Tauri externalBin で `.app` に同梱。The same applies to `src-tauri/binaries/mojiroku-mcp-<triple>` since 2026-09-03: the script builds and places both binaries, and both must exist before `cargo build --workspace` or `tauri build`.
- **モデルは実行時 DL**（`*.gguf`/`*.bin` は gitignore）。保存先は `~/Library/Application Support/com.daichi0812.mojiroku/models/`。whisper large-v3-turbo(547MB) / 要約 Qwen3.5-9B Q4_K_M(5.7GB, 16GB 以上) / Qwen3.5-4B Q4_K_M(2.7GB, それ未満。ADR-0044。Qwen2.5-7B は設定から選べ、手元にある人はそのまま使い続ける) / Silero VAD(864KB)。
  Full Whisper large-v3 q5_0 (1.08 GB) is an explicit offline option in Settings (ADR-0034). Turbo remains the default and is always used by live transcription. `jobs.params.transcription_model` captures the choice at enqueue time; pass it through every offline route, including both meeting tracks. Old settings/jobs default to turbo without a schema migration.
- whisper の**無音ハルシネーション**（「ご視聴ありがとうございました」反復）は VAD で対処済み。Since 2026-09-05 the padded VAD spans are separated by 1 s of silence before whisper sees them (gap-free concatenation made whisper merge utterances and drop short replies), and a VAD result with no speech returns an empty transcript instead of falling back to the raw PCM ([ADR-0031](./docs/decisions/ADR-0031_VAD区間の間に無音を挟み無音入力は空の文字起こしにする.md)). Do not lower the Silero thresholds without re-running the silence fixtures in that ADR.
- Very quiet audio receives bounded gain (up to 16x) for **VAD analysis only**; Whisper and diarization still use the original samples. Keep the no-speech result empty. The level estimate ignores digital-silence blocks and resists isolated loud sounds (ADR-0035); `vad_spans_cli` defaults to this preparation and accepts a final `raw` argument for baseline comparisons.
  The live worker skips only all-zero tails when a VAD model is present so quiet speech can reach that preparation; without VAD it retains the RMS 0.001 guard.
  A configured live VAD is mandatory for each inference call (`with_required_vad`): a failed or removed model skips that preview attempt instead of decoding raw audio. Recording continues.
- Transcription language `"mixed"` opts into language re-detection at speech pauses (ADR-0036). It is consumed by the core before Whisper parameters are built; never send this application marker directly to Whisper. The existing `stt_lang` job snapshot carries it through all offline routes, and the meeting-start snapshot carries it to live preview. Auto, turbo, and greedy remain defaults. Mixed mode requires successful VAD and offsets both timestamps and progress across windows.
  Auto and mixed windows now choose between Japanese and English language probabilities
  before decoding (ADR-0039). Explicit language choices still bypass detection.
- Speaker cleanup uses duration to seed anchor groups, then requires at least one second
  of speech and clear voice separation to retain additional brief speakers (ADR-0041).
  Weak individual turns inherit their aggregate cluster's anchor rather than resurrecting
  raw labels. Check both within-speaker consistency and the public brief-speaker suite;
  speaker count alone missed the v0.6.1 fragmentation regression.
  A new brief speaker also needs one turn of at least 1 s, and a cluster of only sub-2 s
  turns must score below the anchor's own short-turn 5th percentile (ADR-0042). A
  one-on-one meeting still produced eight remote speakers under ADR-0041 alone.
  Speaker separation can be re-run on any transcribed recording (Issue #102). For meetings
  only the system track is re-diarized with the saved start offset; `self` segments are
  never reassigned (`merge::reassign_meeting_speakers`).
- Live translation is an opt-in meeting feature (ADR-0037, updated by ADR-0038), using the separate
  `mojiroku-llm --translate` path with Qwen3.5-9B and `--no-think`. It requires at least 16 GiB
  of detected RAM and its own 5.68 GB cache file; downloading it must not change summary
  selection. Downloads now belong to the app and resume verified partial files (ADR-0038).
  Completed source/translation pairs survive navigation and are saved atomically with the
  recording in SQLite schema v7; final STT does not overwrite them. Keep session/epoch/request
  checks, bounded pending work, and inference cancellation on stop/disable. Live Whisper now reserves the shared heavy-job semaphore atomically;
  release translation's permit only after its child has terminated and been reaped.
- For Japanese output the summary sidecar blocks every token containing a CJK ideograph outside Windows-31J (simplified Chinese such as 进・报), via a logit bias built from the vocabulary at start-up (ADR-0043). English output and live translation are unchanged. Qwen3.5-4B wrote simplified Chinese in 3/28 gate outputs without it.
- The UI follows the macOS light/dark appearance through one token set in `frontend/src/index.css` (ADR-0045). `designTokens.test.ts` fails on font sizes outside 11/12/13/14/15/18/22 px, ad-hoc `rounded-[Npx]` and hard-coded colours in components. Tailwind v4 inlines `--shadow-*` values at build time, so an appearance-dependent shadow colour must live in a `--mj-shadow-*` variable, not in the shadow token.
- Running jobs stop cooperatively (ADR-0046): a per-job flag bound to the worker thread (`mojiroku_core::cancel`) is checked at every `report_stage` and by the Whisper abort callback. A new heavy stage must go through `report_stage` or call `cancel::check()`, and a job must check the flag again before writing results; otherwise Stop waits for the next boundary or is ignored.
- After a mic or meeting transcription, a background `title` job replaces a default name (「録音」「会議」) using the local summary model only if it is already on disk (ADR-0047). It never sends the transcript to the cloud or starts a download. Title jobs are excluded from recording state (`TITLE_JOB_KIND` in the history row, detail active job and `list_jobs`); keep that exclusion in any new job-state query.
- LLM プロンプトは **n_batch(2048) ごとに分割して decode** する（長尺会議で `GGML_ASSERT(n_tokens_all <= n_batch)` を踏まないため）。
- whisper の**逐トークンログ flood** がタイムスタンプ的に長尺会議を停滞させる → `WhisperStt::load()` 先頭で `whisper_rs::install_logging_hooks()` を呼んで抑制（ADR-0009）。話者分離のスケーリングは線形（~0.5xRT）。
- **C++ 例外は Rust を素通りしてプロセス abort する**（tokio の catch_unwind に届いた時点で "Rust cannot catch foreign exceptions"。v0.3.0 実機クラッシュ 3 件の根本原因＝高負荷時の bad_alloc 等）。whisper / sherpa-onnx を呼ぶ新経路は**必ず `mojiroku_core::ffi_guard::guard` を通す**こと（C++ 側 try/catch で Err 化。ADR-0021）。あわせて重い ML ジョブ（STT/話者分離/ローカル要約 sidecar）は `commands::acquire_heavy_job` で**アプリ全体 1 本に直列化**（16GB 機のメモリ枯渇→クラッシュ/スワップフリーズ対策）。ライブ文字起こしは重いジョブ中 tick をスキップして譲る。
- **配布は Developer ID 署名+notarization**（v0.4.0〜, ADR-0022）。署名は**リリース CI の env 駆動のみ**（`APPLE_*` secrets → Tauri が一時 keychain 自動作成。conf に signingIdentity は書かない＝ローカル `just build` は無署名で通る）。entitlements は `src-tauri/entitlements.plist`（audio-input のみ。hardened runtime 下でこれが無いとマイク TCC 要求自体が拒否される）。**.dmg は Tauri が notarize しない**ため CI が手動 notarytool+staple。Tauri は env 不足だと**警告のみで未署名ビルドを完走する**ため CI 冒頭に secrets 存在チェックあり。旧 v0.3.x（未署名）は「damaged」→ `xattr -dr com.apple.quarantine` が必要だった（ADR-0011、install-macos.md の旧バージョン節）。

## ディレクトリ

```
frontend/                Vite+React UI（features/transcription, summary, history, recording, lib/, stores/）
src-tauri/               Tauri v2 シェル。commands（health/transcribe_file/summarize/録音/履歴）、capabilities/、binaries/(gitignore)
crates/mojiroku-core/    ML コア。audio/ stt/ summarize/(byok) diarization/ vad/ store/(SQLite) models/ pipeline/ merge.rs schemas.rs
crates/mojiroku-llm/     ローカル要約 sidecar（llama.cpp）。stdin=プロンプトファイル, stdout=要約
crates/mojiroku-mcp/     ローカル MCP サーバ（rmcp stdio）。履歴 DB を read-only 公開。MCP クライアントが spawn; bundled as externalBin since 2026-09-03
eval/diarization/        話者分離の品質ゲート用ハーネス（GT + 再現スクリプト。音声・モデルは含まない。ADR-0028）
eval/stt/                Public FLEURS CER/WER harness and greedy/beam-5 comparison (ADR-0033; audio and raw results are ignored)
landing/                 配布ランディング（Astro→Cloudflare Workers 静的アセット）。public/_redirects で /download→Releases 302
scripts/build-sidecar.sh mojiroku-llm（triple 名で配置）+ mojiroku-mcp をビルド (both placed as src-tauri/binaries/<name>-<triple>)
docs/                    フラット構成。roadmap/requirements/spec/architecture/CONTRIBUTING/install-macos/mcp/updater-plan + decisions/(ADR-0001〜0024)。索引は docs/README.md
```

## ビルド・実行

前提: Rust 1.88+, Node 20+, cmake, Xcode（Apple Silicon / Metal）。`just` 推奨（`brew install just`）。

```bash
# 依存
npm install && npm --prefix frontend install

# 開発起動（sidecar ビルド込み）
just dev                 # = bash scripts/build-sidecar.sh && npm run tauri dev
# just が無ければ:
bash scripts/build-sidecar.sh && npm run dev

# 配布バンドル（ローカルは無署名 .app/.dmg。配布用の署名+公証は CI）
just build               # = build-sidecar + npm run tauri build

# 個別
npm --prefix frontend run build   # tsc + vite → frontend/dist
bash scripts/build-sidecar.sh     # ★ cargo build の前に必要（下記）
cargo build --workspace
cargo test --workspace
cargo run --release -p mojiroku-core --example transcribe_cli -- <audio> <models_dir>
```

**clone 直後にいきなり `cargo build --workspace` を叩くと失敗する。** sidecar バイナリ
`src-tauri/binaries/mojiroku-llm-<triple>` はビルド成果物で gitignore されているため、
`resource path binaries/mojiroku-llm-aarch64-apple-darwin doesn't exist` になる。
先に `bash scripts/build-sidecar.sh` を実行する（`just dev` / `just build` は自動実行）。
The same check covers `binaries/mojiroku-mcp-<triple>` since 2026-09-03; the script produces both.

**「ビルドが通る」は作業ツリーではなくコミット対象ツリーで確認する。**
過去に `models/` の gitignore で同名ソースが漏れた前例がある:
`git add -A && git checkout-index -a -f --prefix=/tmp/clean/ && (cd /tmp/clean && cargo build --workspace)`

## 開発フロー・ブランチ戦略

- 機能は `feat/<説明>` → `develop`（`--no-ff`）→ マイルストーンで `main`。詳細は [docs/CONTRIBUTING.md](./docs/CONTRIBUTING.md)。
- `main` は安定版のみ。`develop` が最新の動く状態。**`main` へは必ず PR 経由**（ruleset で強制されている）。直コミットしない。
- コミットは Conventional Commits（`feat(core): ...` / `fix(mojiroku-llm): ...`）。
- **確定した技術判断は `docs/decisions/ADR-NNNN_*.md` に残す**
  （→ [ADR の流儀](./docs/decisions/README.md)）。背景・比較案・判断理由・影響が追える形で書く。
- 設計・運用・再現に関わる確定文書は `docs/` の該当ファイルに置く。**新しいカテゴリを勝手に増やさない。**

## Issue and Project synchronization

- PR creation is not complete until each implementation issue has a native
  **Development** link and its Project status has been checked. Follow
  [the issue/PR workflow](docs/CONTRIBUTING.md#issue-and-project-synchronization).
- Use the browser for native links on `develop` PRs; a `Refs`/`Closes` mention alone
  is insufficient. Search by the exact PR URL, preserve existing links, and verify
  with `python3 scripts/sync_issue_project.py --issue N --pr P`.
- After authorized linking, run the same command with `--apply` and explicit
  `--open-status 'In Progress'` for active work, or `--open-status Todo` for deferred
  work. Closed issues must remain **Done**, including historical link backfills.
- Run the checker again after a merge or an authorized issue closure. A merged PR
  does not establish that acceptance criteria are complete. Never close an issue
  or set open work to Done merely to make its Project status match a merged PR.
- Keep public issue progress/checklists current when reporting work. If the browser
  or Project access is unavailable, report the missing link/status as unfinished.

## Code Review Rules

PR の自動レビュー（Codex のコードレビュー）が読む規則。**見出し名は Codex 側の仕様で
固定**なので英語のまま置く（→ [Review GitHub pull requests with Codex](https://learn.chatgpt.com/docs/third-party/github)）。

ここに書くのは「**壊れ方が静かで、知らないと踏む**」制約だけにする。書式・lint のような
機械的な検査は CI に任せる。内容は上の「重要な制約・落とし穴」と重なるが、
レビューで働かせたいものだけを再掲している。**正本はこのファイルと ADR。**

### ML ランタイムの分離

- `crates/mojiroku-core`（whisper.cpp を含む）に **llama.cpp 系の依存を足さない**。
  ggml のシンボルが衝突し、**リンクは通るのに実行時に whisper が壊れて 0 セグメントになる**。
  安全な道は、要約 LLM を別バイナリ sidecar `crates/mojiroku-llm` の側で動かすこと
  （[ADR-0007](./docs/decisions/ADR-0007_要約llamaを別バイナリsidecarに分離.md)）。

### C++ FFI の呼び出し

- whisper / sherpa-onnx を呼ぶ経路を新しく足すときは、必ず
  `mojiroku_core::ffi_guard::guard` を通す。C++ 例外は Rust を素通りして**プロセスごと
  abort する**（v0.3.0 の実機クラッシュ 3 件の根本原因）。例外を `Err` に変えられるのは
  C++ 側の try/catch だけで、Rust の `catch_unwind` では捕まらない
  （[ADR-0021](./docs/decisions/ADR-0021_FFI例外シールドと重処理直列化.md)）。
- 重い ML ジョブ（文字起こし・話者分離・ローカル要約）を新しく足すときは、`HEAVY_ML_JOB`
  セマフォでアプリ全体 1 本に直列化する。並走させると 16GB 機でメモリが枯れ、
  クラッシュやスワップ固着になる。**取り方は経路で 2 つに分かれる。**
  Tauri コマンドから直接呼ぶなら `commands::acquire_heavy_job`（待ちを `stage="queued"` の
  進捗イベントで UI に通知する）。バックグラウンドワーカー（`src-tauri/src/jobs.rs`）からなら
  `commands::acquire_heavy_job_permit`（通知は呼び出し側が `job://update` で行うため、
  ヘルパは permit を返すだけ）。**経路に合わない方を使うと、通知が二重になるか消える。**

### 文字起こしの時刻

- whisper-rs の `state.full()` は whisper.cpp 内蔵の VAD を**バイパスする**。
  VAD を働かせるには `WhisperVadContext` で speech 区間を抜き、無音を除いた PCM を
  whisper に渡したうえで、**タイムスタンプを元の時刻へ再マッピングする**
  （[ADR-0008](./docs/decisions/ADR-0008_VADはwhisper内蔵Sileroを独立適用.md)）。
  再マッピングを省くと、除いた無音の長さぶん字幕がずれる。

### GitHub Actions（このリポジトリは public）

- `pull_request_target` を使わない。fork のコードを base の secrets 付きで動かすことになり、
  「fork PR から secrets に到達する経路が無い」という前提が壊れる。
- レビュー用ワークフロー（`claude-code-review.yml`）を変更したら、**同じ内容をデフォルト
  ブランチ（`main`）にも届ける**。`anthropics/claude-code-action` は、実行しようとした
  ワークフローファイルが default branch 上の版と**内容まで一致**していないと、警告を出して
  自ら終了する。`develop` 側だけ直すと、以後の全 PR が「job は success なのにレビュー 0 件」
  という**静かな無効化**に入る（2026-08-30 に実際に起きた）。
- **PR の required status check になりうるワークフロー**でジョブを条件付きに飛ばすときは、
  ワークフローレベルの `paths:` / `branches:` ではなく **job レベルの `if:`** で書く。
  前者でスキップされた check はそもそも報告されないため、PR が
  `Waiting for status to be reported` で永久にブロックされる。job レベルの `if:` で
  飛ばした job は success 扱いになる。
  なお push トリガーのデプロイ用ワークフロー（`deploy-landing.yml` など）はこの制約の
  対象外で、ワークフローレベルのフィルタを使ってよい。

## 検証の心構え

- **性能・互換性の前提は断定前に裏取りする。**「動いた」は実音声・実データで確認する。
  過去に「faster-whisper は Apple Metal 対応」という誤った前提で設計しかけた例がある。
- 品質ゲートは実会議で評価する。

## 現在地と次

- 完了: scaffold → 1a 文字起こし → 1b 要約（sidecar+BYOK）→ VAD → **1c マイク録音+履歴/FTS5検索（beta-1）** → **Phase 2 話者分離（ADR-0009）** → **Phase 3 MCP サーバー（ADR-0010）** → **Phase 4 配布: `mojiroku.com` 公開（ADR-0011）** → **Phase 5 UI 刷新 ＋ アプリ内アップデート（Tauri v2 updater）＋ 自動リリースCI（ADR-0020）** → **Apple Developer ID 署名+notarization 導入（2026-07, ADR-0022）** → **日英2言語対応（2026-07-04, landing `/en/` + アプリ UI 辞書 `frontend/src/i18n`（ja が形の正）+ コンテンツ言語 settings.language/transcribe_language + エラーキー化。⚠️ EN landing の公開は英語対応アプリのリリースと同時）**。
- 配布ゲート実機（macOS 26）: 署名版は quarantine 付き DL を**警告なしダブルクリック起動できることを v0.4.0 で実機確認済み**（spctl「Notarized Developer ID」accepted）。旧未署名版は「damaged」→ `xattr` で起動だった。
- **自動リリース稼働中（[ADR-0020](./docs/decisions/ADR-0020_自動リリースパイプライン.md)）**: `src-tauri/tauri.conf.json` の `version` を上げて main にマージ（PR 経由）すると `.github/workflows/release.yml`（gate=ubuntu → build-publish=**macos-26**）が .app/.dmg をビルド→Apple 署名+notarization→minisign/Apple 両署名の fail-closed 検証→公開 `mojiroku-releases` に Release→`mojiroku.com/updater/latest.json`（landing Worker プロキシ）が latest を動的配信。**v0.4.0（2026-07-03）で publish まで含む完全自動リリースを実証済み**（PAT write 付与済み。0.2.0→0.3.0 のアプリ内更新 E2E は 2026-06-30 実証）。手順は [docs/updater-plan.md](./docs/updater-plan.md)。
- 次: ベータを友人・研究室に**配って実会議で品質ゲート評価**（話者分離/要約の質）→ フィードバック反復。署名+公証は導入済み（ADR-0022）なので非技術ユーザーにもそのまま配れる。
- 進捗の正は [docs/roadmap.md](./docs/roadmap.md)。

## ライセンス

本プロジェクトは **AGPL-3.0-or-later**。コードを提供する場合は
[`CLA.md`](./CLA.md) への同意が必要になる。判断の経緯は
[ADR-0027](./docs/decisions/ADR-0027_ライセンスをAGPL-3.0とCLAに決定.md)。

生成したコードに第三者のコードを含める場合は、そのライセンスを [`NOTICE`](./NOTICE) に
追記できる形で明示すること。**ライセンス不明のコードを持ち込まない。**

## 詳細ルール

- [`docs/CONTRIBUTING.md`](./docs/CONTRIBUTING.md) — ブランチ / コミット / タグ / PR の規約
- [`docs/README.md`](./docs/README.md) — 文書の置き場の決定表
- [`docs/decisions/README.md`](./docs/decisions/README.md) — ADR の流儀と一覧
