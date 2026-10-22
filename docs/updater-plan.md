# アプリ内アップデート（Tauri v2 updater）

すでにインストール済みのユーザーへ「新バージョンがあります」と通知し、アプリ内のボタンから
自動 DL → 置換 → 再起動できるようにする。roadmap Phase 5「自動アップデート」の実装。

## 実装済み（このコミット）

| 対象 | 内容 |
|---|---|
| `src-tauri/Cargo.toml` | `tauri-plugin-updater = "2"` / `tauri-plugin-process = "2"` |
| `frontend/package.json` | `@tauri-apps/plugin-updater` / `@tauri-apps/plugin-process` |
| `src-tauri/src/lib.rs` | Builder に `.plugin(tauri_plugin_updater::Builder::new().build())` と `.plugin(tauri_plugin_process::init())` |
| `src-tauri/capabilities/default.json` | `updater:default`, `process:allow-restart` を許可 |
| `src-tauri/tauri.conf.json` | `bundle.createUpdaterArtifacts: true` ＋ `plugins.updater`（endpoints / pubkey） |
| `frontend/src/lib/updater.ts` | `checkForUpdate()` / `downloadAndApply()` |
| `frontend/src/features/update/UpdateBanner.tsx` | 起動時チェック＋フロート通知 UI（App.tsx に配線済み） |

検証: `npm --prefix frontend run build`（tsc+vite）クリーン / `cargo check -p mojiroku` クリーン。

## 重要: 誰に通知が届くか（期待値のズレ注意）

**いま配布済みの updater 無しビルドを使っている人には、アプリ内通知は出せない**（その実体に updater コードが無いため）。
- 既存ベータユーザーは **一度だけ手動で `mojiroku.com/download` から入れ直す**必要がある（= updater 入りビルドに乗る）。
- 通知が出るのは「updater 入り版をインストール済み」かつ「より新しい版を公開した」とき。
  → updater 入りを最初のリリースとして撒き、**その次のリリース**で初めてポップアップが出る。

## リリース運用

### 1. updater 専用鍵（設定済み）
- 鍵生成（ユーザー、対話的）:
  ```bash
  npx tauri signer generate -w ~/.tauri/mojiroku.key
  ```
- `tauri.conf.json` の `plugins.updater.pubkey` に貼るのは、**`~/.tauri/mojiroku.key.pub` の中身そのまま**
  （= `cat ~/.tauri/mojiroku.key.pub` の出力）。このファイルの中身が既に base64（minisign 2行を base64 化したもの）なので、
  **`base64` で再エンコードしない**こと（二重エンコードになり検証に失敗する）。手打ちせずコピペ。
  ※ 設定済み: pubkey `dW50cnVzdGVk...`（keyid `1A7458C0EE859094`）。秘密鍵 `~/.tauri/mojiroku.key`。
- 秘密鍵とパスワードはビルド時 env（commit しない）:
  ```bash
  export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/mojiroku.key)"
  export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="********"
  ```
- pubkey が仮のままでも開発実行/コンパイルは通る（`check()` の検証失敗を握りつぶすため）。
  実際に更新を配るには本物の鍵が必須。

### 2. リリース手順（GitHub Actions 自動。`.github/workflows/release.yml`）

リリースは CI で自動化済み。**`src-tauri/tauri.conf.json` の `version` を上げて main にマージ**すると、
`release.yml` が以下を自動実行する（手動実行は Actions の `workflow_dispatch`）:

1. **gate（ubuntu・無料）**: `tauri.conf.json` の version が `HEAD^` から変わったか判定し、公開
   `mojiroku-releases` に同 version が既存なら skip（冪等）。満たすときだけ macOS ジョブを起動。
2. **build-publish（macos-26 / Apple Silicon）**:
   - `scripts/build-sidecar.sh` → `npm run tauri build`（署名 env は GitHub Secrets で注入）。
   - 出力は**ワークスペース直下** `target/release/bundle/`（`dmg/*.dmg` と `macos/*.app.tar.gz` + `.sig`）。
     ※ `src-tauri/target/...` ではない（ワークスペース構成のため）。
   - 安定名へリネーム: `mojiroku-macos-aarch64.dmg` / `mojiroku-macos-aarch64.app.tar.gz`。
   - `scripts/make-updater-manifest.sh` で `latest.json` を生成（signature は `.sig` 中身そのまま）。
   - **署名検証**: 生成 `.sig` が `tauri.conf.json` の pubkey で `minisign -V` を通ることを確認
     （不一致なら全 0.2.0 ユーザが**サイレントに**更新失敗するため、publish 前に決定的に落とす）。
   - 公開 `mojiroku-releases` に **draft** で 3 アセット（.dmg / .app.tar.gz / latest.json）を添付
     → 揃ったら `--draft=false` に flip（原子的切替。draft 中は `releases/latest` が前版に解決し続ける）。
3. **配信**: `latest.json` は Release アセット。`mojiroku.com/updater/latest.json` は landing Worker の
   プロキシ route（`landing/worker/index.ts`）が `releases/latest/download/latest.json` を 200 で返し、
   latest 解決で常時追従する。**version 上げ→main マージだけで既存 0.2.0+ ユーザに通知が届く**
   （landing の再デプロイは不要）。

必要 Secrets（リポジトリ Settings → Secrets）:
- `TAURI_SIGNING_PRIVATE_KEY` = `cat ~/.tauri/mojiroku.key` の中身
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`
- `RELEASES_REPO_TOKEN` = `daichi-hotta/mojiroku-releases` のみに **Contents: Read and write** の fine-grained PAT
  （デフォルト `GITHUB_TOKEN` は別 repo に書けない）。**read-only だと気づきにくい**: releases repo が public なので
  `gh release view` は通るが、`gh release create`（POST）で初めて `HTTP 403: Resource not accessible by personal access token`。
  必ず write を付与すること（未付与時のリカバリは下記「初回切替」末尾参照）。

注意:
- **macOS ランナーは public repo の無料枠で動く**（[ADR-0027](./decisions/ADR-0027_ライセンスをAGPL-3.0とCLAに決定.md) で OSS 化）。
  private 時代は分課金10倍で spending limit/有料プランが必須だった（$5/Stop usage を設定済み。public 化後は消費されない）。
- **ランナーは `macos-26`**。macos-14 は macOS 26 SDK 不足で依存 `apple-metal`（`screencapturekit 8.0` 経由）がコンパイル不可（実地で判明）。
- `url` は latest 解決（**非 draft・非 prerelease** の公開リリースにしか解決しない）。
- Intel 対応時のみ `darwin-x86_64` も足す（当面 aarch64 のみ）。
- **Cloudflare Static Assets は dist の一致ファイルを Worker route より優先**する（`run_worker_first` 未設定）。
  `mojiroku.com/updater/latest.json` は Worker プロキシで動的配信するため、静的 `landing/public/updater/latest.json` は**置かない**
  （置くと route がバイパスされ旧版が固定で返り続ける＝初回切替で実際に踏んだ）。詳細は [ADR-0020](./decisions/ADR-0020_自動リリースパイプライン.md)。

#### 初回切替（実施済み・2026-06-30）

`latest.json` は当初 Release アセットに無く静的ファイル配信だったため、以下の順で投入して成立（履歴・再現用）:
1. `workflow_dispatch` の **dry_run** で高価なビルド+署名検証を1回確認（公開しない／成果物は artifact）。
2. version を **0.3.0** に上げて初回リリースを公開 → `latest.json` が Release アセットになる。
3. `curl -IL .../releases/latest/download/latest.json` が **200** を確認。
4. **そのあと** Worker に `/updater/latest.json` route 追加 → `npx wrangler deploy` → 静的 `landing/public/updater/latest.json` を撤去して再 deploy。
5. インストール済み 0.2.0 → 0.3.0 のアプリ内 E2E（通知→DL→relaunch）。→ **全段グリーンで成立**。

> 未付与 PAT で publish 段（`gh release create`）が 403 で落ちても、成果物は `if: success()` で artifact 化されるため、
> ダウンロードしてオーナーの `gh`（releases repo admin）で `gh release create -R daichi-hotta/mojiroku-releases ...` すれば
> 再ビルド無しで手動公開できる（v0.3.0 はこの手動リカバリで公開）。`RELEASES_REPO_TOKEN` に write 付与後は自動で完結。

## 重要な前提（落とし穴）

1. **updater は Apple コード署名とは独立の minisign 鍵で検証**する。当初は未署名配布のまま動くことが要件だったが、
   **Developer ID 署名+notarization 導入後（ADR-0022）も検証チェーンは独立のまま不変**（CI は minisign 検証と
   Apple 署名検証を別ステップで両方 fail-closed 実行）。
2. **更新アーティファクトは `.dmg` ではなく `.app.tar.gz`（+ `.sig`）**。updater がこれを DL して `.app` を直接置換 → 再起動。
   署名導入後は notarize+staple 済みの `.app` がそのまま tar.gz 化される（再署名不要・順序問題なし）。
3. **Tauri v2 は `bundle.createUpdaterArtifacts: true` が必須**（無いと `.app.tar.gz`/`.sig` が生成されない）。
4. **quarantine（"damaged"）はアップデート時には回避される — 実機検証済み（2026-06-30）**。
   初回DLの「壊れているため開けません」はブラウザDLの quarantine 属性が原因。updater はアプリ自身が
   HTTP 取得・展開・置換するため quarantine が付かず、**0.2.0→0.3.0 の更新後に "damaged" が再発しないことを実機で確認**。
5. **未署名(0.3.x) → 署名版へのアプリ内更新では、署名 identity の変化（ad-hoc → Developer ID）で TCC
   （マイク・画面収録）が一度だけ再プロンプトされる**。updater の動作自体（DL→検証→置換→再起動）は不変。
   以後の署名版どうしの更新は安定 DR により TCC が永続する（ADR-0022）。

## 検証（実機・リリース後）— **実証済み（2026-06-30, 0.2.0→0.3.0）**
- ✅ 旧版（updater 入り）→ 新版リリース → 起動で通知 → 更新 → 再起動でバージョンが 0.3.0 に上がる。
- ✅ 未署名のまま updater 適用後に Gatekeeper "damaged" が**再発しない**（前提4）。
- オフライン時にチェックが静かに失敗し UI を邪魔しないこと（Worker 503 時も同挙動）。
- ✅ `curl -IL https://github.com/daichi-hotta/mojiroku-releases/releases/latest/download/mojiroku-macos-aarch64.app.tar.gz` が最後まで 200、
  `curl -s https://mojiroku.com/updater/latest.json | jq .version` が `0.3.0`。


## Release verification (2026-09-06, v0.5.7 to v0.6.0)

[v0.6.0](https://github.com/daichi-hotta/mojiroku-releases/releases/tag/v0.6.0) was
published from source commit `9338e8bbcc56d51bf3517c2be580cfae1cbfa64e` by
[release run 34030466995](https://github.com/daichi-hotta/mojiroku/actions/runs/34030466995).
The main merge tree matched the locally validated and signed dry-run source tree.

| Check | Result |
|---|---|
| Public app archive and DMG | Digests match GitHub assets; Apple signatures, notarization and Gatekeeper checks pass |
| Public updater endpoint | Serves 0.6.0; manifest matches the release asset; updater signature verifies against the embedded public key |
| In-app update | Published v0.5.7 copy detected 0.6.0, downloaded, installed and relaunched; About shows 0.6.0 |
| Fresh DMG installation | Copied to a separate location and launched on the same Mac with the existing profile |
| Installed executables | App and both sidecars match the published updater archive through both installation paths |
| Existing data | Recordings, transcript segments and jobs unchanged against a pre-launch SQLite backup |

![About window after the public v0.5.7 to v0.6.0 update](images/v0.6.0-update-confirmation.png)

Application copies were used for validation; this was not a new macOS user profile.
The original missing-text report in #87 remains open for confirmation through real
meeting use after release. It is not claimed fully resolved by these installation checks.
