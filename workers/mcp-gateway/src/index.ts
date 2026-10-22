// mojiroku リモート MCP の OAuth 2.1 ゲートウェイ（ADR-0025）。
//
// - OAuth まわり（/authorize の検証・/token・/register(DCR)・/.well-known/*）は
//   @cloudflare/workers-oauth-provider が処理する。トークン類は OAUTH_KV に
//   ハッシュのみ保存される（ライブラリの設計）。
// - アクセストークン検証済みの /mcp リクエストだけが apiHandler に届き、
//   Cloudflare Tunnel の origin へ ORIGIN_TOKEN を付けてプロキシされる。
// - /authorize の「ユーザー認証」は自分専用パスフレーズ（GATEWAY_PASSPHRASE）。
//   議事録 DB は単一ユーザーのローカルデータなので、ID 基盤は持たない。
// - クライアント登録とリダイレクト先は claude.ai のコールバックだけに絞る。DCR は誰でも
//   叩けるので、絞らないと「攻撃者の URL へ認可コードを送るクライアント」を登録され、
//   正規ドメインの同意画面でパスフレーズを入れた瞬間にトークンを持ち去られる。

import { DurableObject } from 'cloudflare:workers';
import { OAuthProvider, type OAuthHelpers } from '@cloudflare/workers-oauth-provider';

interface Env {
  OAUTH_KV: KVNamespace;
  OAUTH_PROVIDER: OAuthHelpers;
  /** パスフレーズ試行回数の全体カウンタ（AuthorizeGuard、インスタンスは 1 つだけ） */
  AUTHORIZE_GUARD: DurableObjectNamespace<AuthorizeGuard>;
  /** /authorize 同意画面のパスフレーズ（wrangler secret） */
  GATEWAY_PASSPHRASE: string;
  /** origin（mojiroku-mcp --http）の Bearer トークン（wrangler secret） */
  ORIGIN_TOKEN: string;
  /**
   * origin ホスト名を Cloudflare Access で守るときのサービストークン（wrangler secret、任意）。
   * 両方そろっているときだけ CF-Access-Client-Id/Secret ヘッダを付ける。
   */
  CF_ACCESS_CLIENT_ID?: string;
  CF_ACCESS_CLIENT_SECRET?: string;
}

/** Tunnel の origin。ここより先（Mac 側）は mojiroku-mcp 自身の Bearer 検証が守る。 */
const ORIGIN = 'https://mcp-origin.mojiroku.com';

/**
 * 認可コードの送り先として許すのは claude.ai カスタムコネクタのコールバックだけ。
 * 公式ドキュメントが claude.com への移行予定を明記しているので両方を許可する。
 */
const ALLOWED_REDIRECT_URIS: ReadonlySet<string> = new Set([
  'https://claude.ai/api/mcp/auth_callback',
  'https://claude.com/api/mcp/auth_callback',
]);

/**
 * パスフレーズ試行の全体上限（全 IP 合算）。失敗後の 1 秒待ちはリクエストごとにしか効かず、
 * 並列に投げられると総当たりを止められないため、窓内の試行数を Durable Object で数える。
 * 上限に達すると窓が明けるまで正しいパスフレーズでも拒否する（自分も締め出されるが、
 * 承認は 30 日に 1 回程度なので許容する）。
 */
const MAX_ATTEMPTS_PER_WINDOW = 10;
const ATTEMPT_WINDOW_MS = 60 * 60 * 1000;

interface GuardState {
  windowStart: number;
  attempts: number;
}

/**
 * /authorize のパスフレーズ試行を数える Durable Object。インスタンスは idFromName('global') の 1 つ。
 * 試行は照合の前に数え（並列リクエストで上限をすり抜けさせない）、成功したら 0 に戻す。
 */
export class AuthorizeGuard extends DurableObject<Env> {
  /** 窓内の試行が上限未満なら 1 回分を予約して true。上限なら false。 */
  async tryAttempt(): Promise<boolean> {
    const now = Date.now();
    let state = await this.ctx.storage.get<GuardState>('state');
    if (!state || now - state.windowStart >= ATTEMPT_WINDOW_MS) {
      state = { windowStart: now, attempts: 0 };
    }
    if (state.attempts >= MAX_ATTEMPTS_PER_WINDOW) return false;
    await this.ctx.storage.put('state', { ...state, attempts: state.attempts + 1 });
    return true;
  }

  async reset(): Promise<void> {
    await this.ctx.storage.delete('state');
  }
}

/** タイミング攻撃でパスフレーズを先頭から確定されないよう、全バイトを必ず比較する。 */
function constantTimeEq(a: string, b: string): boolean {
  const ab = new TextEncoder().encode(a);
  const bb = new TextEncoder().encode(b);
  if (ab.length !== bb.length) return false;
  let diff = 0;
  for (let i = 0; i < ab.length; i++) diff |= ab[i] ^ bb[i];
  return diff === 0;
}

/**
 * 同意画面の HTML 応答。スクリプトは無く、インライン style だけを許す。
 * frame-ancestors で他サイトへの埋め込み（クリックジャッキング）を拒否する。
 * form-action は付けない（承認後の claude.ai への 302 まで制限され、フローが壊れるため）。
 */
function html(body: string, status = 200): Response {
  return new Response(body, {
    status,
    headers: {
      'content-type': 'text/html; charset=utf-8',
      'content-security-policy': "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'",
      'x-frame-options': 'DENY',
      'referrer-policy': 'no-referrer',
      'cache-control': 'no-store',
    },
  });
}

/** 同意画面。フォームは同じ /authorize（クエリ付き）へ POST し、OAuth パラメータを引き継ぐ。 */
function consentPage(clientName: string, redirectHost: string, action: string, error?: string): string {
  return `<!doctype html>
<html lang="ja"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex">
<title>mojiroku MCP 接続の承認</title>
<style>
  body { font-family: -apple-system, sans-serif; background: #111; color: #eee;
         display: grid; place-items: center; min-height: 100dvh; margin: 0; }
  form { background: #1c1c1e; padding: 2rem; border-radius: 12px; max-width: 22rem; width: 90%; }
  h1 { font-size: 1.1rem; margin: 0 0 .5rem; }
  p { font-size: .85rem; color: #aaa; margin: 0 0 1rem; }
  input { width: 100%; box-sizing: border-box; padding: .6rem; border-radius: 8px;
          border: 1px solid #444; background: #111; color: #eee; margin-bottom: 1rem; }
  button { width: 100%; padding: .6rem; border: 0; border-radius: 8px;
           background: #e8590c; color: #fff; font-weight: 600; cursor: pointer; }
  .err { color: #ff6b6b; font-size: .85rem; margin-bottom: 1rem; }
</style></head><body>
<form method="post" action="${action}">
  <h1>mojiroku の議事録への接続を承認</h1>
  <p>クライアント: <strong>${clientName}</strong><br>
     承認後の送り先: <strong>${redirectHost}</strong><br>
     承認すると、このクライアントはローカル Mac の議事録を検索・閲覧できます（読み取り専用）。</p>
  ${error ? `<div class="err">${error}</div>` : ''}
  <input type="password" name="passphrase" placeholder="パスフレーズ" autofocus required>
  <button type="submit">承認する</button>
</form></body></html>`;
}

/** HTML 属性・テキストに埋め込む値のエスケープ（クライアント名は DCR で外部入力になる）。 */
function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!,
  );
}

const defaultHandler = {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);

    if (url.pathname === '/authorize') {
      let oauthReqInfo;
      try {
        oauthReqInfo = await env.OAUTH_PROVIDER.parseAuthRequest(request);
      } catch {
        // 未登録クライアント・未登録リダイレクト先など。詳細は返さない。
        return new Response('Invalid authorization request', { status: 400 });
      }
      // 許可リスト導入前に登録されたクライアントも、ここで claude.ai 以外への送付を止める。
      if (!ALLOWED_REDIRECT_URIS.has(oauthReqInfo.redirectUri)) {
        return new Response('Redirect URI not allowed', { status: 400 });
      }
      const client = await env.OAUTH_PROVIDER.lookupClient(oauthReqInfo.clientId);
      const clientName = escapeHtml(client?.clientName ?? oauthReqInfo.clientId);
      const redirectHost = escapeHtml(new URL(oauthReqInfo.redirectUri).host);
      // フォームの POST 先に元のクエリをそのまま残す（OAuth パラメータの持ち回り）。
      const action = escapeHtml(url.pathname + url.search);

      if (request.method === 'GET') {
        return html(consentPage(clientName, redirectHost, action));
      }
      if (request.method === 'POST') {
        const guard = env.AUTHORIZE_GUARD.get(env.AUTHORIZE_GUARD.idFromName('global'));
        if (!(await guard.tryAttempt())) {
          return html(
            consentPage(clientName, redirectHost, action, '試行回数の上限に達しました。1 時間ほど待ってから再度お試しください'),
            429,
          );
        }
        const form = await request.formData();
        const passphrase = String(form.get('passphrase') ?? '');
        if (!constantTimeEq(passphrase, env.GATEWAY_PASSPHRASE)) {
          await new Promise((r) => setTimeout(r, 1000));
          return html(consentPage(clientName, redirectHost, action, 'パスフレーズが違います'), 401);
        }
        await guard.reset();
        const { redirectTo } = await env.OAUTH_PROVIDER.completeAuthorization({
          request: oauthReqInfo,
          userId: 'daichi',
          metadata: { approvedAt: new Date().toISOString() },
          scope: oauthReqInfo.scope,
          props: {},
        });
        return Response.redirect(redirectTo, 302);
      }
      return new Response('Method not allowed', { status: 405 });
    }

    return new Response('mojiroku MCP gateway', { status: 404 });
  },
};

const apiHandler = {
  async fetch(request: Request, env: Env): Promise<Response> {
    // ここに届く時点でアクセストークン検証は済んでいる。origin へは ORIGIN_TOKEN に
    // 差し替えて転送する（クライアントのトークンを origin に見せない）。
    const url = new URL(request.url);
    const target = new URL(url.pathname + url.search, ORIGIN);
    const headers = new Headers(request.headers);
    headers.set('authorization', `Bearer ${env.ORIGIN_TOKEN}`);
    headers.delete('cookie');
    if (env.CF_ACCESS_CLIENT_ID && env.CF_ACCESS_CLIENT_SECRET) {
      headers.set('cf-access-client-id', env.CF_ACCESS_CLIENT_ID);
      headers.set('cf-access-client-secret', env.CF_ACCESS_CLIENT_SECRET);
    }
    // Response をそのまま返すことで SSE ストリーミングと Mcp-Session-Id を素通しする。
    return fetch(target, {
      method: request.method,
      headers,
      body: request.body,
      redirect: 'manual',
    });
  },
};

export default new OAuthProvider({
  apiRoute: '/mcp',
  // ライブラリの型は WorkerEntrypoint 前提の総称型だが、README 記載の通り
  // 「fetch を持つオブジェクト」も受け付ける。
  apiHandler: apiHandler as any,
  defaultHandler: defaultHandler as any,
  authorizeEndpoint: '/authorize',
  tokenEndpoint: '/token',
  clientRegistrationEndpoint: '/register',
  scopesSupported: ['meetings.read'],
  // OAuth 2.1: S256 のみ許可（plain PKCE を無効化）。
  allowPlainPKCE: false,
  // DCR は claude.ai のコールバックだけを送り先に持つクライアントに限る。
  clientRegistrationCallback: ({ clientMetadata }) => {
    const uris = clientMetadata.redirect_uris;
    const ok =
      Array.isArray(uris) &&
      uris.length > 0 &&
      uris.every((u) => typeof u === 'string' && ALLOWED_REDIRECT_URIS.has(u));
    if (!ok) return { description: 'redirect_uris must be the Claude connector callback' };
  },
});
