// mojiroku.com の Cloudflare Worker。
//
// 主目的: Slack / Notion OAuth のブローカー。どちらも token 交換に client_secret（confidential client）が
// 要る（Slack は加えて loopback を「非 web リダイレクト」とみなし bot スコープを拒否する）ため、
// web(https) リダイレクトが必要。この Worker が client_id/secret を保持して同意 → トークン交換まで行い、
// 得た資格情報（Slack=Incoming Webhook URL / Notion=アクセストークン）をデスクトップアプリの loopback
// ポートへ 302 で返す。アプリ側は client_id も secret も持たない。
//
// それ以外のパスは従来どおり静的アセット（Astro の ./dist、_redirects/_headers 込み）へ委譲する。
//
// 必要な Secret（`npx wrangler secret put …`）:
//   SLACK_CLIENT_ID / SLACK_CLIENT_SECRET     … Slack アプリの Client ID / Secret
//   NOTION_CLIENT_ID / NOTION_CLIENT_SECRET   … Notion public integration の OAuth client id / secret
// 各サービス側の Redirect URL には `https://mojiroku.com/oauth/<service>/callback` を登録する。

interface Env {
  ASSETS: { fetch: (request: Request) => Promise<Response> };
  SLACK_CLIENT_ID: string;
  SLACK_CLIENT_SECRET: string;
  NOTION_CLIENT_ID: string;
  NOTION_CLIENT_SECRET: string;
}

// loopback の許可ポート（アプリ src-tauri/src/oauth.rs の BROKER_REDIRECT_PORTS と一致させる）。
// 任意ポートへの 302 を防ぐためホワイトリスト化する（open-redirect 対策）。Slack/Notion で共用。
const ALLOWED_PORTS = new Set(["8765", "8766", "8767"]);
const SLACK_REDIRECT = "https://mojiroku.com/oauth/slack/callback";
const SLACK_AUTHORIZE = "https://slack.com/oauth/v2/authorize";
const SLACK_TOKEN = "https://slack.com/api/oauth.v2.access";
const SLACK_SCOPE = "incoming-webhook";
const NOTION_REDIRECT = "https://mojiroku.com/oauth/notion/callback";
const NOTION_AUTHORIZE = "https://api.notion.com/v1/oauth/authorize";
const NOTION_TOKEN = "https://api.notion.com/v1/oauth/token";

/** Google Search Console の所有権確認ファイルのパス（Search Console が発行した名前）。 */
const GSC_VERIFICATION_PATH = "/google6705194712d0362d.html";

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    if (url.pathname === "/oauth/slack/start") return slackStart(url, env);
    if (url.pathname === "/oauth/slack/callback") return slackCallback(url, env);
    if (url.pathname === "/oauth/notion/start") return notionStart(url, env);
    if (url.pathname === "/oauth/notion/callback") return notionCallback(url, env);
    // アプリ内アップデートのマニフェスト。インストール済みアプリの endpoint は
    // mojiroku.com/updater/latest.json に焼き付いているため、ここを公開 Releases の
    // 最新マニフェストへ常時追従させる（CI が latest.json を Release アセットとして発行）。
    if (url.pathname === "/updater/latest.json") return updaterManifest();
    // Google Search Console の所有権確認ファイル。
    // Static Assets の既定（html_handling）は `.html` を落として拡張子なしの URL へ
    // 307 で飛ばすが、Google は `.html` のパスをそのまま見に来る。サイト全体の URL 形は
    // 変えたくないので、このパスだけ中身を直接返す（2026-08-30。実測で 307 を確認）。
    // ⚠️ 確認後もこの応答を消さないこと（"don't remove the file, even after verification succeeds"）。
    if (url.pathname === GSC_VERIFICATION_PATH) {
      return new Response(`google-site-verification: ${GSC_VERIFICATION_PATH.slice(1)}\n`, {
        headers: { "content-type": "text/html; charset=utf-8" },
      });
    }
    // それ以外は静的アセット（_redirects の /download 302 等もここで効く）。
    return env.ASSETS.fetch(request);
  },
};

/**
 * Tauri v2 updater のマニフェスト配信。公開 mojiroku-releases の latest リリースに
 * 添付された latest.json をプロキシして 200 で返す（302 ではなく本文を返すので、
 * updater 側のリダイレクト追従に依存しない）。
 *
 * fail-closed: 上流が非 200 のときは古い内容を返さず 503。アプリの checkForUpdate() は
 * 例外/取得失敗を握り潰して「更新なし」扱いにするため、503 は UI を壊さない。
 * 毎起動で叩かれるので 5 分キャッシュで GitHub を保護（伝播は最大 5 分遅れるが無害）。
 */
async function updaterManifest(): Promise<Response> {
  const upstream =
    "https://github.com/daichi-hotta/mojiroku-releases/releases/latest/download/latest.json";
  let r: Response;
  try {
    // cf.cacheTtl/cacheEverything は Cloudflare Workers のサブリクエストキャッシュ制御。
    // @cloudflare/workers-types を入れていないため RequestInit には無い → 型だけ拡張する。
    const init = {
      cf: { cacheTtl: 300, cacheEverything: true },
    } as RequestInit & { cf: { cacheTtl: number; cacheEverything: boolean } };
    r = await fetch(upstream, init);
  } catch {
    return new Response("manifest fetch failed", { status: 503 });
  }
  if (!r.ok) {
    // latest が無い/アセット未添付など。古い 200 を返さない（fail-closed）。
    return new Response("manifest unavailable", { status: 503 });
  }
  return new Response(r.body, {
    status: 200,
    headers: {
      "content-type": "application/json",
      "cache-control": "public, max-age=300",
    },
  });
}

/** アプリ → ここ。loopback の port と state を受け、Slack の同意画面へ 302。 */
function slackStart(url: URL, env: Env): Response {
  const port = url.searchParams.get("port") ?? "";
  const state = url.searchParams.get("state") ?? "";
  if (!ALLOWED_PORTS.has(port) || !state) {
    return new Response("bad request", { status: 400 });
  }
  if (!env.SLACK_CLIENT_ID) {
    return new Response("SLACK_CLIENT_ID not configured", { status: 500 });
  }
  // callback で loopback ポートを復元できるよう state に port を載せる（state は base64url で "." を含まない）。
  const packed = `${port}.${state}`;
  const authorize = new URL(SLACK_AUTHORIZE);
  authorize.searchParams.set("client_id", env.SLACK_CLIENT_ID);
  authorize.searchParams.set("scope", SLACK_SCOPE);
  authorize.searchParams.set("redirect_uri", SLACK_REDIRECT);
  authorize.searchParams.set("state", packed);
  return Response.redirect(authorize.toString(), 302);
}

/** Slack → ここ。code を secret 付きで交換し、Webhook URL を loopback へ 302 で返す。 */
async function slackCallback(url: URL, env: Env): Promise<Response> {
  const packed = url.searchParams.get("state") ?? "";
  const dot = packed.indexOf(".");
  const port = dot >= 0 ? packed.slice(0, dot) : "";
  const state = dot >= 0 ? packed.slice(dot + 1) : "";
  if (!ALLOWED_PORTS.has(port)) {
    return new Response("bad state", { status: 400 });
  }

  const oauthErr = url.searchParams.get("error");
  if (oauthErr) return bounce(port, { error: oauthErr, state });

  const code = url.searchParams.get("code");
  if (!code) return bounce(port, { error: "no_code", state });

  // confidential client: client_secret を使ってトークン交換（web リダイレクトなので bot スコープ可）。
  const body = new URLSearchParams({
    client_id: env.SLACK_CLIENT_ID,
    client_secret: env.SLACK_CLIENT_SECRET,
    code,
    redirect_uri: SLACK_REDIRECT,
  });
  let data: { ok?: boolean; error?: string; incoming_webhook?: { url?: string } };
  try {
    const resp = await fetch(SLACK_TOKEN, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body,
    });
    data = await resp.json();
  } catch (e) {
    return bounce(port, { error: `exchange_request_failed: ${String(e)}`, state });
  }

  if (!data.ok) return bounce(port, { error: data.error ?? "exchange_failed", state });
  const webhook = data.incoming_webhook?.url;
  if (!webhook) return bounce(port, { error: "no_webhook", state });
  return bounce(port, { webhook, state });
}

/** アプリ → ここ。loopback の port と state を受け、Notion の同意画面へ 302。 */
function notionStart(url: URL, env: Env): Response {
  const port = url.searchParams.get("port") ?? "";
  const state = url.searchParams.get("state") ?? "";
  if (!ALLOWED_PORTS.has(port) || !state) {
    return new Response("bad request", { status: 400 });
  }
  if (!env.NOTION_CLIENT_ID) {
    return new Response("NOTION_CLIENT_ID not configured", { status: 500 });
  }
  // callback で loopback ポートを復元できるよう state に port を載せる（state は base64url で "." を含まない）。
  const packed = `${port}.${state}`;
  const authorize = new URL(NOTION_AUTHORIZE);
  authorize.searchParams.set("client_id", env.NOTION_CLIENT_ID);
  authorize.searchParams.set("response_type", "code");
  authorize.searchParams.set("owner", "user");
  authorize.searchParams.set("redirect_uri", NOTION_REDIRECT);
  authorize.searchParams.set("state", packed);
  return Response.redirect(authorize.toString(), 302);
}

/** Notion → ここ。code を Basic 認証（client_id:secret）で交換し、アクセストークンを loopback へ 302。 */
async function notionCallback(url: URL, env: Env): Promise<Response> {
  const packed = url.searchParams.get("state") ?? "";
  const dot = packed.indexOf(".");
  const port = dot >= 0 ? packed.slice(0, dot) : "";
  const state = dot >= 0 ? packed.slice(dot + 1) : "";
  if (!ALLOWED_PORTS.has(port)) {
    return new Response("bad state", { status: 400 });
  }

  const oauthErr = url.searchParams.get("error");
  if (oauthErr) return bounce(port, { error: oauthErr, state });

  const code = url.searchParams.get("code");
  if (!code) return bounce(port, { error: "no_code", state });

  // Notion は token エンドポイントを HTTP Basic（client_id:secret）+ JSON ボディで受ける。
  const basic = btoa(`${env.NOTION_CLIENT_ID}:${env.NOTION_CLIENT_SECRET}`);
  let data: { access_token?: string; error?: string };
  try {
    const resp = await fetch(NOTION_TOKEN, {
      method: "POST",
      headers: {
        Authorization: `Basic ${basic}`,
        "Content-Type": "application/json",
        // token エンドポイントも Notion-Version 必須（アプリの notion.rs と同じ安定版にピン）。
        "Notion-Version": "2022-06-28",
      },
      body: JSON.stringify({
        grant_type: "authorization_code",
        code,
        redirect_uri: NOTION_REDIRECT,
      }),
    });
    data = await resp.json();
  } catch (e) {
    return bounce(port, { error: `exchange_request_failed: ${String(e)}`, state });
  }

  const token = data.access_token;
  if (!token) return bounce(port, { error: data.error ?? "exchange_failed", state });
  return bounce(port, { notion: token, state });
}

/** loopback（アプリ）へ 302 で結果を返す。 */
function bounce(port: string, params: Record<string, string>): Response {
  const q = new URLSearchParams(params).toString();
  return Response.redirect(`http://127.0.0.1:${port}/?${q}`, 302);
}
