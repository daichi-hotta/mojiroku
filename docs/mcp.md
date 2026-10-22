# mojiroku MCP サーバー（ローカル議事録を Claude から検索・参照）

`mojiroku-mcp` は、mojiroku が保存したローカル議事録（録音→文字起こし→要約／話者分離）を
**MCP（Model Context Protocol）** 経由で Claude Desktop / Claude Code に公開する stdio サーバーです。
履歴 DB を**読み取り専用**で開くだけで、ローカル完結・$0・ネットワーク不要。設計は [ADR-0010](./decisions/ADR-0010_ローカルMCPサーバーをstdio別バイナリで提供.md)。

## 公開ツール

| ツール | 引数 | 返すもの |
|---|---|---|
| `search_meetings` | `query`（文字列） | 全文検索のヒット（タイトル / 日時 / スニペット / `recording_id`） |
| `list_recent_meetings` | `limit`（省略時 20） | 最近の会議メタ（`recording_id` / タイトル / 日時 / 長さ） |
| `get_meeting` | `recording_id`, `include_transcript`（省略時 false） | 要約・メタ・話者。`include_transcript=true` で逐語全文も |

## 使うバイナリ

**`mojiroku-mcp` は `.app` に同梱されている。**別途ビルドは要らない。

```
/Applications/mojiroku.app/Contents/MacOS/mojiroku-mcp
```

署名・公証はアプリ本体と同じ経路で済んでいるので、Gatekeeper の `xattr` 回避も不要。
アプリを更新すればこのバイナリも一緒に更新される。パスは固定なので設定を書き直す必要はない。

⚠️ **v0.5.4 以前の `.app` には入っていない。**古い版を使っている場合はアプリを更新する。

DB パスは既定で `~/Library/Application Support/com.daichi0812.mojiroku/mojiroku.db` を見るため、
通常は指定不要。別パスを使う場合だけ `--db <path>` 引数か環境変数 `MOJIROKU_DB` で渡す。

### 開発者向け: ソースからビルドする

repo を clone して開発する場合は `scripts/build-sidecar.sh`（`just dev` / `just build` から自動で呼ばれる）が
`target/release/mojiroku-mcp` を作る。同梱版の代わりにこちらを指定してもよい。

```bash
cargo build --release -p mojiroku-mcp
echo "$(pwd)/target/release/mojiroku-mcp"
```

`mojiroku-mcp` は `mojiroku-core` に依存するため、ソースからのビルドには whisper.cpp の C++ ビルド環境
（cmake / Xcode）と sherpa-onnx の prebuilt ライブラリ取得（初回は要ネット）が要る。
**議事録を読むためだけに他のマシンでこれを揃えるのは重い**ので、同梱版を使うこと。

## Claude Code に登録

```bash
# 既定 DB パスを使う場合（推奨）
claude mcp add mojiroku /Applications/mojiroku.app/Contents/MacOS/mojiroku-mcp

# DB パスを明示する場合
claude mcp add mojiroku -- /Applications/mojiroku.app/Contents/MacOS/mojiroku-mcp \
  --db "$HOME/Library/Application Support/com.daichi0812.mojiroku/mojiroku.db"
```

登録後、`/mcp` でサーバーが connected になり、3 ツールが見えれば成功。

## Claude Desktop に登録

設定ファイル `~/Library/Application Support/Claude/claude_desktop_config.json` に追記:

```json
{
  "mcpServers": {
    "mojiroku": {
      "command": "/Applications/mojiroku.app/Contents/MacOS/mojiroku-mcp"
    }
  }
}
```

DB パスを明示するなら `"args": ["--db", "/Users/<you>/Library/Application Support/com.daichi0812.mojiroku/mojiroku.db"]`
を足す。保存して Claude Desktop を再起動すると、ツール一覧に mojiroku の 3 ツールが現れる。

## 使い方の例

- 「先週の MVP ミーティングで決まったことは？」→ `search_meetings` で探し、`get_meeting` で要約・決定事項を引く。
- 「あの会議の◯◯さんの発言を確認したい」→ `get_meeting` を `include_transcript=true` で全文取得。

## リモート MCP（claude.ai から引く）

同じ `mojiroku-mcp` を **HTTP モード**で常駐させ、OAuth ゲートウェイ（Cloudflare Worker）と
Cloudflare Tunnel を挟むと、**claude.ai（Web / モバイルの Claude）のカスタムコネクタ**からも
自分の議事録を検索・参照できます。ローカルは読み取り専用のまま、認可済みリクエストだけを
内側へ通します。設計は [ADR-0025](./decisions/ADR-0025_リモートMCPをOAuthゲートウェイとTunnelで公開.md)。

```
claude.ai カスタムコネクタ
  → mcp.mojiroku.com（Cloudflare Worker = OAuth ゲートウェイ）
  → mcp-origin.mojiroku.com（Cloudflare Tunnel / cloudflared）
  → 127.0.0.1:8970（ローカル mojiroku-mcp --http・Bearer 検証・読み取り専用 DB）
```

> ⚠️ 単一ユーザー（自分専用）の構成です。同意はパスフレーズ 1 本で、多人数の ID 基盤は持ちません。
> インターネットに口が開くので、下記のトークン・パスフレーズは他人に渡さないこと。

### 1. HTTP モードのトークンを用意

```bash
openssl rand -hex 32   # これを MOJIROKU_MCP_TOKEN として使う（32 文字未満は起動拒否）
```

### 2. ローカルを HTTP モードで常駐（launchd）

`~/Library/LaunchAgents/com.daichi0812.mojiroku-mcp.plist` を作り、次を常駐させる
（`RunAtLoad`・`MOJIROKU_MCP_TOKEN` は `EnvironmentVariables` で渡す）:

```
/Applications/mojiroku.app/Contents/MacOS/mojiroku-mcp --http 8970 \
  --allowed-host mcp-origin.mojiroku.com
```

- bind は常に `127.0.0.1`。LAN・インターネットへは自前で出さない（公開は Tunnel の仕事）。
- `--allowed-host` は必須。付けないと rmcp の Host 検証（DNS rebinding 対策）が Tunnel 経由の
  `Host: mcp-origin.mojiroku.com` を弾く。
- ロードは `launchctl load ~/Library/LaunchAgents/com.daichi0812.mojiroku-mcp.plist`。
  ログは plist の `StandardErrorPath`（例 `~/Library/Logs/mojiroku-mcp.log`）で確認。

### 3. Cloudflare Tunnel で origin を公開（cloudflared）

`mcp-origin.mojiroku.com` を `cloudflared` で `http://127.0.0.1:8970` に向ける（`cloudflared tunnel`）。
これも launchd 常駐にしておく。ポート開放・固定 IP は不要で $0。

推奨: Cloudflare Zero Trust で `mcp-origin.mojiroku.com` に Access アプリを作り、
サービストークン（Service Auth）だけを許すポリシーにする。ゲートウェイ以外は Tunnel に届く前に落ちる。
発行したトークンの Client ID / Secret は手順 4 でゲートウェイの secret に入れる。

### 4. OAuth ゲートウェイ（Cloudflare Worker）をデプロイ

ソースは `workers/mcp-gateway/`。**リリース CI には載せない手動デプロイ**:

```bash
cd workers/mcp-gateway
npm install
# secrets（コード非同梱。値はローカルとゲートウェイで揃える）
npx wrangler secret put GATEWAY_PASSPHRASE   # /authorize 同意画面のパスフレーズ
npx wrangler secret put ORIGIN_TOKEN         # ← 手順1の MOJIROKU_MCP_TOKEN と同じ値
# 任意（手順 3 で Access を張った場合）
npx wrangler secret put CF_ACCESS_CLIENT_ID
npx wrangler secret put CF_ACCESS_CLIENT_SECRET
npx wrangler deploy                          # mcp.mojiroku.com に配信
```

- 登録できるクライアントは claude.ai のコールバック（`https://claude.ai/api/mcp/auth_callback` と
  `https://claude.com/api/mcp/auth_callback`）を送り先に持つものだけ。
- パスフレーズの試行は全体で 1 時間 10 回まで。超えると 1 時間は正しいパスフレーズでも 429 になる。

疎通の目安（配管が生きているか）:

```bash
curl -o /dev/null -w "%{http_code}\n" https://mcp.mojiroku.com/.well-known/oauth-authorization-server  # 200
curl -o /dev/null -w "%{http_code}\n" -X POST https://mcp.mojiroku.com/mcp -d '{}'                       # 401（無トークン）
```

### 5. claude.ai にカスタムコネクタとして登録

claude.ai の設定 → コネクタ → カスタムコネクタで **`https://mcp.mojiroku.com/mcp`** を追加。
OAuth のフロー（DCR → 同意画面でパスフレーズ入力 → 認可）を通すと、claude.ai から
`search_meetings` / `get_meeting` / `list_recent_meetings` の 3 ツールが使える。

> **前提**: claude.ai から引ける間は、Mac・`cloudflared`・`mojiroku-mcp --http` が起動している
> 必要がある（launchd 常駐でカバー。スリープ中は不達）。

## 注意

- stdio では **stdout が JSON-RPC チャネル**。サーバーは診断・ログを **stderr** にのみ出す（stdout を汚さない）。
- 読み取り専用。議事録の作成・編集はアプリ本体で行う（MCP からは書き込めない）。
- アプリ起動の有無に関係なく動く（履歴 DB は永続。WAL で並行読み取り可）。
