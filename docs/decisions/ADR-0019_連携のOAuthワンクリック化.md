# 0019. 連携の OAuth ワンクリック化（Slack / Google / Notion・loopback + Worker ブローカー・$0 維持）

- ステータス: 採用
- 日付: 2026-06-29
- 一部置換: [[ADR-0013_Notion書き出し]]（認証＝内部トークン）/ [[ADR-0014_Slack送信]]（認証＝Webhook URL 貼付）/ [[ADR-0016_カレンダー取り込み]]（認証/取得＝限定公開 iCal）。**各 ADR の exporter / 整形 / ブロック構築・iCal パーサは引き続き有効**で、本 ADR が更新するのは**認証・取得の層のみ**。

## Context

Phase 6 の3連携（[[ADR-0013_Notion書き出し]] / [[ADR-0014_Slack送信]] /
[[ADR-0016_カレンダー取り込み]]）は、いずれも **ユーザーが自分で秘密情報を発行して貼り付ける**方式だった:

- **Notion**: 内部インテグレーションを作成 → トークンを貼る → 対象ページを手動共有 → 親ページ ID/URL を貼る
- **Slack**: Incoming Webhook を作成 → URL を貼る
- **Google**: 「限定公開 iCal URL」を設定画面から探して貼る

実際にベータで触ると**摩擦が大きく非技術者に不向き**。これら 3 ADR は当時いずれも「**$0・OAuth 不要・審査不要**」を理由に OAuth を回避し、特に 0016 は OAuth Desktop を「重い別プロジェクト（双方向が必要になったら再検討）」と却下していた。

その前提を再検証した（[[verify-load-bearing-assumptions]]・WebFetch で各社公式確認）:

- **OAuth は $0 で実現できる**。受け口を **ephemeral loopback（`http://127.0.0.1:{port}` にワンショット）** にすれば、
  `gcloud`/`gh` と同方式で**常駐サーバーなし**で code を受領できる（spec.md の「常駐 HTTP を持たない」原則とは別物 ＝
  数十秒・loopback bind・1 回限り）。ホスティング費は文字通り 0。
- **Slack だけは loopback で不成立**。Slack は `127.0.0.1` を「**非 web リダイレクト**」とみなし bot スコープ
  （`incoming-webhook` 含む）を拒否する（実エラー: *"Bot scopes are not allowed when redirecting to a non-web URI."*）。
  bot/webhook 名義を保つには **https の web リダイレクト = サーバー**が要る。
- **Google** は Desktop App + PKCE + loopback で成立。client_secret は Google 仕様上「秘密扱いしない」。
  Calendar は sensitive scope → **未確認アプリ警告 + 100 ユーザー上限**（ベータ <100 人で警告許容の方針）。
- **Notion** の REST OAuth は token 交換に client_secret（confidential client）が要る（PKCE 非対応）。
  当初は別の認証面である **Remote MCP `mcp.notion.com` の OAuth 2.1 + DCR + PKCE**（secret/サーバー/アプリ登録すべて不要）
  を検討したが、**Slack でサーバー（Worker）が既に存在**することで「Worker 回避」という MCP 採用の唯一の動機が消滅した（下記 Decision）。

## Decision

**3連携すべてを「◯◯と連携」ボタン → ブラウザで同意 → 完了 の OAuth ワンクリックに移行**する。共通の OAuth/PKCE 基盤
（`src-tauri/src/oauth.rs`）を 1 つ持ち、provider 差分（エンドポイント・scope・受け口）だけ切り替える。維持費は **$0 のまま**。

- **共通基盤 `src-tauri/src/oauth.rs`**: ephemeral/固定どちらも対応の **loopback ワンショット listener**、
  **PKCE(S256)**（verifier/challenge/state の乱数は uuid v4 を流用＝getrandom を足さない）、token 交換（ureq blocking・
  `spawn_blocking` の中で呼ぶ）。RFC 7636 ベクタ含む単体テスト。

- **Slack = Worker ブローカー方式**（[[mojiroku-distribution-architecture]] の Cloudflare Worker を拡張）。
  `mojiroku.com` の Worker が `/oauth/slack/{start,callback}` を担い、**client_id/secret を Worker secret として保持**して
  Slack 同意 → トークン交換まで行い、得た **Incoming Webhook URL を loopback へ 302** で返す。アプリは client_id も secret も
  PKCE も持たない。得た URL を既存スロット `slack_webhook_url` に保存 → **`slack.rs`（exporter・mrkdwn 変換）は完全に不変**。
  マルチテナント（各ユーザーが自分のワークスペースへインストール = Slack の Public Distribution を有効化）。

- **Google = loopback + PKCE 直接フロー（Worker 不要）**。Desktop 型 OAuth クライアント。`access_type=offline` /
  `prompt=consent` で refresh token を取得し、`google_oauth_access` / `google_oauth_refresh` / `google_token_expiry` を
  Keychain 保存。失効 60 秒前で自動 refresh。**データ経路を iCal → Calendar API `events.list` に差し替え**
  （`singleEvents=true&orderBy=startTime` で**サーバ側が繰り返しを展開** → 0016 の RRULE 壁時計展開が不要に・全日予定は除外）。
  client_id/secret は oauth.rs 定数（Desktop 型は非機密＝同梱可・トークンは端末↔Google 直結）。

- **Notion = Worker ブローカー REST 方式**（当初の MCP+DCR 案から転換）。Worker が `/oauth/notion/{start,callback}` を担い、
  `owner=user` で authorize、token 交換は **Basic 認証(client_id:secret) + JSON + `Notion-Version` 必須**、得た
  **アクセストークンを loopback へ 302** で返す。既存スロット `notion_token` に保存 → **`NotionExporter`（ブロック構築・
  page 親・バージョンピン）は完全に不変**。**書き出し先ページ**は OAuth 同意でユーザーが共有を許可したページから選ぶ
  （core `export::accessible_pages`＝search API で列挙 → ドロップダウン・候補 1 件なら自動選択）。
  - **MCP+DCR ではなく REST にした理由**: ①MCP+DCR を選ぶ唯一の動機は「Worker 回避」だったが Slack で Worker が既に存在
    ②既存 `NotionExporter`（509 行・実績あり）を**不変で再利用**でき、rmcp/reqwest を src-tauri に足さずに済む
    ③**Notion 標準 REST OAuth はアクセストークン無期限が既定**（~1h 失効 + リフレッシュトークン・ローテーションは
    **MCP エンドポイント/オプトイン時のみ**）→ **refresh ロジック不要**で Worker は接続時だけ・書き出しホットパスに一切出ない
    ④rmcp client は「自前 bearer を注入する公開 API が無く OAuth 全体を rmcp に任せる前提」で oauth.rs 基盤と綺麗に接合できない。

- **Worker ブローカーの共通則**: loopback の受け口ポートは固定 `BROKER_REDIRECT_PORTS = [8765,8766,8767]`（Worker の
  許可リストと一致・open-redirect 防止のホワイトリスト）。`state` に `port` を載せて callback で復元。アプリは資格情報
  （Webhook URL / アクセストークン）を loopback で受け取り Keychain に保存するだけ。**外部サービスへの redirect 登録は
  Worker の callback URL（https）だけ**で、loopback ポートは各社に登録しない。

## Consequences / リスク

- **維持費は $0 のまま**。Cloudflare Worker は無料枠・接続時のみ稼働。トークンは各端末の Keychain に留まり、
  Google は端末↔Google 直結（Worker は Google 経路に出ない）。
- **保守の所在が移動**（ゼロ保守ではない）。旧: ユーザー各自がトークン管理。新: **Daichi が OAuth アプリ 3 つを保有**し、
  Slack Public Distribution / Google の同意画面（未確認 → 警告許容・ベータ <100 人・将来 verify）/ Notion public integration を維持。
  - ⚠️ **Notion は integration の「トークン有効期限（token expiration）」を OFF のまま**にすること。ON にすると無期限前提が崩れ、
    Worker に refresh ルートを足す必要がある（REST 方式のままで対応可・A への作り直しは不要）。
  - **Worker secret**: `SLACK_CLIENT_ID/SECRET` + `NOTION_CLIENT_ID/SECRET` の 4 つ（コード非同梱）。
- **後方互換**: Google は OAuth 未連携なら**従来の限定公開 iCal にフォールバック**（既存ユーザーを壊さない・0016 の経路を残す）。
  Notion は旧内部トークン（`notion_token` スロット共用）でも引き続き動作。
- ⚠️ **資格情報が loopback リダイレクト URL に乗る**（Slack=Webhook URL / Notion=アクセストークン）。**loopback 経由のみ・
  外部送信なし**だがブラウザ履歴には残る。Slack Webhook と同じ扱いでベータでは許容。
- **実機で潰した loopback バグ 2 つ（基盤共通）**: ①**macOS は accept したソケットがリスナの nonblocking を継承**する
  （BSD 特有・Linux は継承せず）→ accept 後 `set_nonblocking(false)`。②`wait_for_redirect` の完了判定が `code` しか見ておらず
  Worker 方式の `webhook` / `notion` を拾えず白画面で無限待機 → 完了判定に `webhook` / `notion` を追加。
- **`state` の照合は listener の中で行う**（2026-10-22）。`state` が一致しないリクエストには中立ページを返して
  `REDIRECT_TIMEOUT` まで待ち続け、`error` も `state` が一致したときだけ受け付ける。Worker の `bounce` と
  Google のエラー応答はどちらも `state` を返すため、利用者が拒否した場合は従来どおりすぐに終わる。
- **3 連携とも実機 E2E 成功**（Slack=実投稿 / Google=予定リスト表示 / Notion=指定ページに議事録作成）。
- **デプロイ機構の注意**: 本番 Worker は**ローカル `cd landing && npx wrangler deploy`** が更新する。Cloudflare の
  **Workers Builds（Git 連携 CI）は `develop` をビルド**するため、worker が develop に乗るまで（feat ブランチのマージ前）は
  CI が失敗するが**無害**（失敗ビルドは本番を上書きしない）。
- **却下した代替**: Notion MCP+DCR（rmcp 大書き換え + 実行時 mcp.notion.com 依存 + ~1h トークン保守。Worker が無い前提でのみ
  優位だったが前提が消滅）。Slack user スコープ（本人名義投稿・アプリ内チャンネル選択が必要で UX 劣化）。

関連: [[ADR-0013_Notion書き出し]], [[ADR-0014_Slack送信]], [[ADR-0016_カレンダー取り込み]],
[[ADR-0012_設定永続化とBYOKキーチェーンと要約分岐]], [[ADR-0011_配布は未署名dmgでCloudflareとReleases]]（Worker の母体）,
`src-tauri/src/oauth.rs`, `landing/worker/index.ts`, `docs/roadmap.md` Phase 6。
