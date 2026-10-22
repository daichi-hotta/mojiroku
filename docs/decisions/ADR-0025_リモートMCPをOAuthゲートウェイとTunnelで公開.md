# 0025. リモート MCP を OAuth 2.1 ゲートウェイ + Cloudflare Tunnel で公開

- ステータス: 採用（claude.ai からのエンドツーエンド実疎通を 2026-07-18 に確認済み）
- 日付: 2026-07-13（実疎通確認: 2026-07-18）
- 関連: [[ADR-0010_ローカルMCPサーバーをstdio別バイナリで提供]]（同じ `mojiroku-mcp`・読み取り専用・3 ツール。本 ADR はそれを stdio から HTTP へ延長）/ [[ADR-0011_配布は未署名dmgでCloudflareとReleases]]（Cloudflare を配布に既に使用＝Tunnel も同じ $0 枠）/ [[ADR-0019_連携のOAuthワンクリック化]]（mojiroku.com Worker ブローカーの OAuth 運用ノウハウ）

## Context

[[ADR-0010_ローカルMCPサーバーをstdio別バイナリで提供]] の `mojiroku-mcp` は **stdio 専用**で、
MCP クライアント（Claude Desktop / Claude Code）が**同じ Mac 上でプロセスを spawn** して初めて
使える。つまり議事録を引けるのは「その Mac で動く Claude」だけだった。

引きたいのは **claude.ai（Web / モバイルの Claude）**からも自分の議事録を検索・参照すること。
claude.ai の「カスタムコネクタ」はローカルプロセスを spawn できず、**HTTP で到達できる MCP
エンドポイント + OAuth 2.1** を要求する。よって次の 2 つが要る:

1. `mojiroku-mcp` を **HTTP（streamable HTTP）でも喋れる**ようにする。
2. ローカルの HTTP エンドポイントを**インターネットから安全に到達可能**にし、かつ
   claude.ai が求める **OAuth 2.1 の認可フロー**を満たす。

制約は北極星と同じ **サーバー費 $0 の維持**（[[ADR-0011_配布は未署名dmgでCloudflareとReleases]] で
Cloudflare は既に使用）。議事録 DB は**単一ユーザーのローカルデータ**なので、多人数向けの ID 基盤は不要。
守るべきは「**自分だけが、自分の Mac の議事録に、読み取り専用で到達できる**」こと。

## Decision

**4 段のチェーンで claude.ai → ローカル議事録をつなぐ。** ローカル側は読み取り専用のまま、
外向き公開は「認可済みリクエストだけを内側へ通す」ゲートウェイに閉じ込める。

```
claude.ai カスタムコネクタ
  │  OAuth 2.1（DCR + S256 PKCE）+ アクセストークン
  ▼
mcp.mojiroku.com（Cloudflare Worker = OAuth ゲートウェイ）   ← workers/mcp-gateway/
  │  トークン検証 → ORIGIN_TOKEN(Bearer) に差し替えてプロキシ
  ▼
mcp-origin.mojiroku.com（Cloudflare Tunnel / cloudflared）
  │  127.0.0.1:8970 へ転送（LAN にもインターネットにも直接は出さない）
  ▼
mojiroku-mcp --http 8970（ローカル・Bearer 検証・読み取り専用 DB）  ← ADR-0010 のバイナリ
```

### 1. `mojiroku-mcp` に HTTP モードを追加（既定は stdio のまま）

`--http <port>` を渡したときだけ `rmcp` の `transport-streamable-http-server` + `axum` で
`127.0.0.1:<port>/mcp` を配信する。**引数なしは従来通り stdio**（既存の Claude Code / Desktop
登録は無改修で動く）。bind は常に loopback＝**LAN にもインターネットにも自前では出さない**
（外部公開は Tunnel の責務に一元化）。

- **Bearer 認証必須**: env `MOJIROKU_MCP_TOKEN`（`ps` に映る CLI 引数ではなく環境変数で受ける）。
  **32 文字未満は起動拒否の fail-closed**（認証なしで議事録が公開される事故を構成ミスの段階で止める）。
  比較は**定数時間**（トークンを先頭から 1 文字ずつ確定されない）。失敗理由は区別せず一律 401。
- **DNS rebinding 対策**: rmcp は Host ヘッダを検証し既定で loopback のみ許可。Tunnel 経由は
  `Host: mcp-origin.mojiroku.com` で来るので `--allowed-host mcp-origin.mojiroku.com` で明示追加する。
- **未知引数はエラー終了**（タイポで意図せず stdio モードに落ち、HTTP を待つ launchd がハングする
  事故を防ぐ）。

### 2. OAuth ゲートウェイ（`workers/mcp-gateway/` = Cloudflare Worker）

claude.ai が要求する OAuth 2.1 の口（`/authorize` `/token` `/register`(DCR)
`/.well-known/*`）は **`@cloudflare/workers-oauth-provider`** に任せる。トークン類は
`OAUTH_KV` に**ハッシュのみ保存**される（ライブラリの設計）。アクセストークン検証済みの
**`/mcp` リクエストだけ**が `apiHandler` に届き、Tunnel origin へプロキシされる。

- **「ユーザー認証」は単一ユーザー用パスフレーズ**（`GATEWAY_PASSPHRASE`）。`/authorize` の同意画面で
  入力し、定数時間比較。ID 基盤は持たない（議事録は単一ユーザーのローカルデータ）。
- **総当たりは全体の試行回数で止める**: 失敗時の 1 秒待ちはリクエストごとにしか効かず、並列に
  投げられると意味が無い。そこで Durable Object `AuthorizeGuard`（インスタンス 1 つ）が 1 時間の窓で
  POST の試行を数え、**照合の前に**1 回分を予約する。10 回に達すると窓が明けるまで正しい
  パスフレーズでも 429。成功すると 0 に戻す。攻撃者が自分を締め出せるが、承認は 30 日に 1 回程度
  なので許容する。
- **クライアント登録と送り先は claude.ai のコールバックに限る**: DCR は誰でも叩けるため、絞らないと
  攻撃者の URL へ認可コードを送るクライアントを登録され、正規ドメインの同意画面でパスフレーズを
  入れた時点でトークンを持ち去られる。`clientRegistrationCallback` で `redirect_uris` を
  `https://claude.ai/api/mcp/auth_callback` と `https://claude.com/api/mcp/auth_callback`（公式が
  移行予定と明記）だけに制限し、`/authorize` でも同じ許可リストを確かめる（制限前に登録された
  クライアント向け）。同意画面には送り先のホストを表示する。
- **同意画面は埋め込ませない**: `frame-ancestors 'none'` と `X-Frame-Options: DENY`。`form-action` は
  付けない（承認後の claude.ai への 302 まで制限され、フローが壊れる）。
- **クライアントトークンを origin に見せない**: `/mcp` プロキシ時に `Authorization` を
  `ORIGIN_TOKEN`（＝ローカル `MOJIROKU_MCP_TOKEN` と同値）に差し替え、`cookie` は落とす。
  Response をそのまま返すことで **SSE ストリーミングと `Mcp-Session-Id` を素通し**する。
- **DCR のクライアント名は外部入力**なので同意画面表示前に HTML エスケープする。
- **OAuth 2.1: S256 PKCE のみ**（`allowPlainPKCE: false`）。
- **Secrets はコード非同梱**（`wrangler secret put`）: `GATEWAY_PASSPHRASE` / `ORIGIN_TOKEN`、
  任意で `CF_ACCESS_CLIENT_ID` / `CF_ACCESS_CLIENT_SECRET`（下の 3）。
  `wrangler.jsonc` / `package.json` / `index.ts` にキー名だけ記す。

### 3. Cloudflare Tunnel（`cloudflared`）で origin を公開

`mcp-origin.mojiroku.com` を `cloudflared` で `127.0.0.1:8970` に向ける。ポート開放も固定 IP も
不要で **$0**。Tunnel より先（Mac 側）は `mojiroku-mcp` 自身の Bearer 検証が守る（多層防御）。

origin のホスト名は公開 DNS にあり、そのままでは誰でも `ORIGIN_TOKEN` の推測を試せる。
Cloudflare Access のアプリを `mcp-origin.mojiroku.com` に張り、サービストークンだけを通すポリシーに
すると、ゲートウェイ以外は Tunnel に届く前に落ちる。ゲートウェイは `CF_ACCESS_CLIENT_ID` /
`CF_ACCESS_CLIENT_SECRET` がそろっているときだけ `CF-Access-Client-Id` / `CF-Access-Client-Secret`
を付けて転送する（Access の設定はダッシュボード側の個人環境の構成）。

### 4. ローカル常駐は launchd

`mojiroku-mcp --http 8970 --allowed-host mcp-origin.mojiroku.com` を launchd で常駐
（`RunAtLoad`）。`MOJIROKU_MCP_TOKEN` は plist の `EnvironmentVariables` で渡す。`cloudflared` も
同様に launchd 常駐。**アプリ本体の起動有無に関係なく動く**（履歴 DB は永続・WAL で並行読み取り可）。

### なぜゲートウェイと origin でトークンを分けるか

claude.ai が持つのは**ゲートウェイが発行した OAuth アクセストークン**で、ローカルの
`MOJIROKU_MCP_TOKEN` は知らない。ゲートウェイが境界でトークンを差し替えることで、
**外に出る資格情報（失効・ローテート可能な OAuth トークン）と、内側の固定 Bearer を分離**できる。
claude.ai 側の資格情報が漏れても、それはゲートウェイで失効させれば足り、ローカルトークンは無傷。

## デプロイ / 運用（リリース CI には載せない）

- **手動デプロイ**: `cd workers/mcp-gateway && npx wrangler deploy`。リリースパイプライン
  （[[ADR-0020_自動リリースパイプライン]]）はアプリ配布物専用なので**ゲートウェイは分離**する
  （アプリのバージョンバンプと Worker のデプロイは無関係）。
- ゲートウェイのソースは repo にコミット済み（`node_modules` / `.wrangler` は gitignore）。
  **Worker とローカル（Mac）の常駐設定＝ launchd plist・`cloudflared` 設定・各 secret は個人環境の
  構成**で、repo には含めない（手順は [mcp.md](../mcp.md) のリモート節）。

## 検証

- **配管（OAuth チェーン）**: 実プローブで確認済み — DCR `/register`→201、正規 client での
  `/authorize`→パスフレーズ同意画面が表示、`/mcp` は無トークンで 401、`/.well-known/oauth-authorization-server`→200。
  `cloudflared`（tunnel `mojiroku-mcp`）常駐・ローカル `--http 8970` は launchd 起動でログ正常。
- ✅ **claude.ai カスタムコネクタからのエンドツーエンド実疎通（2026-07-18 確認済み）**: claude.ai に
  カスタムコネクタ `https://mcp.mojiroku.com/mcp` を追加 → DCR → 同意画面でパスフレーズ入力で OAuth 承認 →
  `list_recent_meetings` が実行され、ローカル Mac の議事録 DB から実データ（実会議 3 件）が返ることを確認。
  全チェーン（claude.ai → ゲートウェイ → Tunnel → ローカル `--http` → 読み取り専用 DB）が実データで成立。

## 影響・制約

- **攻撃面が stdio 版から増える**（インターネット公開）。緩和は多層: Tunnel（ポート非開放）→
  ゲートウェイ（OAuth + パスフレーズ）→ ローカル Bearer（32 文字以上 fail-closed・定数時間）→
  読み取り専用 DB（そもそも書けない）。単一ユーザー・パスフレーズは**自分専用ゲート**の割り切り。
- **常時オン前提**: claude.ai から引くには Mac と `cloudflared` と `mojiroku-mcp --http` が
  起動している必要がある（launchd 常駐でカバー。スリープ中は不達）。
- **スキーマ整合は stdio 版と同条件**（[[ADR-0024_バックグラウンドジョブ基盤]]）: HTTP モードでも
  同じ `open_readonly` を使うので、本体を v5 化したら MCP も v5 core から再ビルドして出す。
- **ローカル MCP（stdio）は無改修で残る**。リモートは stdio の置き換えではなく**追加の到達経路**。
