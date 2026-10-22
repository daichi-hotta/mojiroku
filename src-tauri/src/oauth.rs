//! OAuth 連携（Notion MCP / Slack / Google Calendar）の共通基盤。
//!
//! 方針（docs/05_decisions 連携OAuth・計画書）:
//! - **loopback リダイレクト一本化**: `http://127.0.0.1:{port}` に std の TcpListener で
//!   ワンショット（1 リクエストだけ受けて即クローズ）。`gcloud`/`gh` と同方式で、spec.md が
//!   禁じる「常駐 HTTP サーバー」とは別物（数十秒・loopback bind・1 回限り）。
//! - **PKCE(S256) で client_secret なし**: 3 連携とも public client。verifier/state の乱数は
//!   uuid v4 のバイト列を流用（getrandom を直接足さない）。
//! - **token 交換は ureq（blocking）**: core と同じ。重い/ブロックする処理なので呼び出し側の
//!   コマンドは `spawn_blocking` の中でこのモジュールを呼ぶこと（UI スレッドを止めない）。
//!
//! 各プロバイダの client_id は公開値（secret ではない）なので定数として同梱する。
//! Daichi が各サービスでアプリ登録後にここを埋める。空のままなら connect は誘導エラーを返す。
//! Notion は DCR（動的登録）で client_id すら不要 → 別途実装（このファイルの基盤を流用）。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use base64::Engine;
use sha2::{Digest, Sha256};
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

// ── プロバイダ定数 ───────────────────────────────────────────────────────────

/// OAuth ブローカー Worker のベース URL（mojiroku.com の Cloudflare Worker）。
/// Slack は loopback を「非 web リダイレクト」とみなし bot スコープ（incoming-webhook）を
/// **拒否する**ため、web(https) リダイレクト = この Worker を経由する。Worker が client_id/secret を
/// 保持してトークン交換まで行い、結果の Webhook URL を下の loopback ポートへ 302 で返す。
/// アプリ側は client_id も client_secret も PKCE も持たない（最小・登録は Worker 側だけ）。
const WORKER_BASE: &str = "https://mojiroku.com";

/// Worker からの 302 を受ける loopback の固定ポート候補（Worker 側の許可ポートと一致させること）。
/// Slack/Notion とも Worker ブローカー経由でこの受け口を共用する（外部サービスへの登録は不要 —
/// 各サービスが知るのは Worker の callback URL だけ）。先頭から空きを探す。
const BROKER_REDIRECT_PORTS: &[u16] = &[8765, 8766, 8767];

/// ユーザーが同意を完了するまで loopback で待つ最大時間。
const REDIRECT_TIMEOUT: Duration = Duration::from_secs(300);

// ── PKCE / 乱数 ────────────────────────────────────────────────────────────

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// PKCE の code_verifier（43〜128 文字の url-safe 文字列）。uuid v4 を 2 本連結した
/// 32 バイトのエントロピーを base64url 化（= 43 文字）。Google/Notion 用（Slack は Worker 経由）。
fn gen_verifier() -> String {
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    b64url(&bytes)
}

/// code_challenge = base64url(SHA256(verifier))（method=S256）。Google/Notion 用。
fn challenge_of(verifier: &str) -> String {
    let mut h = Sha256::new();
    h.update(verifier.as_bytes());
    b64url(&h.finalize())
}

/// CSRF 用の不透明な state。
fn gen_state() -> String {
    b64url(uuid::Uuid::new_v4().as_bytes())
}

// ── loopback listener ──────────────────────────────────────────────────────

/// loopback リダイレクトの受け口を bind する。`ports` が空なら ephemeral（:0、Google 用。
/// 任意ポートが許される）。非空なら先頭から空きを探す（Slack/Notion 用。事前登録ポート）。
/// 戻り値は (listener, 実際に bind したポート)。
fn bind_loopback(ports: &[u16]) -> Result<(TcpListener, u16), String> {
    if ports.is_empty() {
        let l = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("loopback bind: {e}"))?;
        let port = l.local_addr().map_err(|e| e.to_string())?.port();
        return Ok((l, port));
    }
    for &p in ports {
        if let Ok(l) = TcpListener::bind(("127.0.0.1", p)) {
            return Ok((l, p));
        }
    }
    Err(format!(
        "loopback の固定ポート {ports:?} がすべて使用中です。一度閉じて再試行してください。"
    ))
}

/// リダイレクト URI を 1 箇所で組み立てる（authorize と token 交換で同一文字列を使うこと）。
/// PKCE 直接フロー（Google/Notion）用。Slack は Worker 経由なので使わない。
fn redirect_uri(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// OS の既定ブラウザで URL を開く。opener はこの spawn_blocking スレッドから呼ばれるため、
/// 失敗時は macOS の `open` にフォールバックして確実に起動する。
fn open_browser(app: &AppHandle, url: &str) -> Result<(), String> {
    if let Err(e) = app.opener().open_url(url.to_string(), None::<&str>) {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map_err(|e2| format!("ブラウザを開けませんでした（opener: {e} / open: {e2}）"))?;
    }
    Ok(())
}

/// loopback に届いた 1 リクエストをどう扱うか。
#[derive(Debug, PartialEq)]
enum RedirectOutcome {
    /// このフローのリダイレクトではない（favicon 等・`state` の欠落や不一致）。中立ページを返して待ち続ける。
    Ignore,
    /// このフローのリダイレクト。完了ページを返し、クエリパラメータを呼び出し側へ渡す。
    Done,
}

/// 完了とみなすのは `state` が一致し、かつ code（PKCE 直接フロー）/ webhook（Slack の Worker
/// ブローカー）/ notion（Notion の Worker ブローカー）/ error のいずれかを含むリクエストだけ。
/// `state` が一致しない `error` で待機を終わらせない（Worker の bounce と Google のエラー応答は
/// どちらも `state` を返すので、利用者の拒否はこの条件で拾える）。
fn redirect_outcome(params: &HashMap<String, String>, expected_state: &str) -> RedirectOutcome {
    if params.get("state").map(String::as_str) != Some(expected_state) {
        return RedirectOutcome::Ignore;
    }
    if ["code", "webhook", "notion", "error"]
        .iter()
        .any(|key| params.contains_key(*key))
    {
        RedirectOutcome::Done
    } else {
        RedirectOutcome::Ignore
    }
}

/// loopback で 1 回のリダイレクトを待ち、クエリパラメータを返す。`state` が一致する完了リクエスト
/// （[`redirect_outcome`]）が来るまで、期限（[`REDIRECT_TIMEOUT`]）までループする。
/// それ以外のリクエストには中立ページを返して継続する。完了時はブラウザに「完了しました」ページを返す。
fn wait_for_redirect(
    listener: &TcpListener,
    expected_state: &str,
) -> Result<HashMap<String, String>, String> {
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("loopback nonblocking: {e}"))?;
    let deadline = Instant::now() + REDIRECT_TIMEOUT;

    loop {
        // 期限は接続の有無に関係なく毎周確かめる（無関係な接続が続いても期限で終わる）。
        if Instant::now() >= deadline {
            return Err("error.oauth.timeout".into());
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                // ⚠️ macOS では accept したストリームがリスナの nonblocking を継承する（BSD 特有。
                // Linux は継承しない）。明示的に blocking へ戻さないと read が即 WouldBlock を返し、
                // リクエストを取りこぼして白画面のまま code を拾えない。set_read_timeout は blocking
                // 前提で効く。
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                // リクエスト行（"GET /?code=...&state=... HTTP/1.1"）は最初のパケット先頭にある。
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let target = req
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("");
                let params = parse_query(target);

                let done = redirect_outcome(&params, expected_state) == RedirectOutcome::Done;
                let body = if done {
                    "<!doctype html><html lang=\"ja\"><meta charset=\"utf-8\">\
                     <title>mojiroku</title><body style=\"font-family:system-ui;text-align:center;padding:48px;color:#111\">\
                     <h2>連携が完了しました</h2><p>このタブを閉じて mojiroku に戻ってください。</p></body></html>"
                } else {
                    // favicon 等・state が一致しないリクエスト。握りつぶして次の接続を待つ。
                    "<!doctype html><title>mojiroku</title>"
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();

                if done {
                    return Ok(params);
                }
                // 無関係リクエストなら継続
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(120));
            }
            Err(e) => return Err(format!("loopback accept: {e}")),
        }
    }
}

/// "/path?a=b&c=d" → {a:b, c:d}（url-safe デコード）。
fn parse_query(target: &str) -> HashMap<String, String> {
    let query = target.split_once('?').map(|x| x.1).unwrap_or("");
    url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect()
}

// ── 汎用フロー（authorization code + PKCE） ─────────────────────────────────

/// authorization code + PKCE フローの設定。client_secret は持たない（public client）。
/// Google/Notion の loopback 直接フロー用（Slack は Worker 経由のため未使用）。
pub struct PkceFlow {
    pub client_id: String,
    /// Google Desktop 型の「秘密扱いしない」client_secret。None なら送らない（純 PKCE）。
    pub client_secret: Option<String>,
    pub auth_endpoint: String,
    pub token_endpoint: String,
    pub scope: String,
    /// 固定ポート（事前登録が要る Slack 等）。空なら ephemeral（Google は任意ポート可）。
    pub redirect_ports: Vec<u16>,
    /// authorize URL の追加クエリ（Google の access_type=offline / prompt=consent 等）。
    pub extra_auth_params: Vec<(String, String)>,
}

/// ブラウザ同意 → loopback で code 受領 → token 交換まで一気通貫で行い、token レスポンス JSON を返す。
/// **blocking**（loopback 待機・ureq）なので必ず `spawn_blocking` の中で呼ぶこと。
/// Google/Notion 用（Slack は Worker 経由なので未使用）。
pub fn authorize_and_exchange(
    app: &AppHandle,
    flow: &PkceFlow,
) -> Result<serde_json::Value, String> {
    let (listener, port) = bind_loopback(&flow.redirect_ports)?;
    eprintln!("[oauth] loopback 127.0.0.1:{port} で待機 → ブラウザを開きます");
    let redirect = redirect_uri(port);
    let verifier = gen_verifier();
    let challenge = challenge_of(&verifier);
    let state = gen_state();

    // authorize URL を組み立て（パラメータは url crate が percent-encode する）。
    let mut auth_url =
        url::Url::parse(&flow.auth_endpoint).map_err(|e| format!("authorize URL: {e}"))?;
    {
        let mut q = auth_url.query_pairs_mut();
        q.append_pair("client_id", &flow.client_id);
        q.append_pair("response_type", "code");
        q.append_pair("redirect_uri", &redirect);
        q.append_pair("scope", &flow.scope);
        q.append_pair("state", &state);
        q.append_pair("code_challenge", &challenge);
        q.append_pair("code_challenge_method", "S256");
        for (k, v) in &flow.extra_auth_params {
            q.append_pair(k, v);
        }
    }

    // OS の既定ブラウザで同意ページを開く。
    open_browser(app, auth_url.as_str())?;

    // loopback で code を待つ（state が一致するリダイレクトだけが返る）。
    let params = wait_for_redirect(&listener, &state)?;
    eprintln!(
        "[oauth] リダイレクト受領: keys={:?}",
        params.keys().collect::<Vec<_>>()
    );
    if params.get("state").map(String::as_str) != Some(state.as_str()) {
        // state 不一致（CSRF 防止）。wait_for_redirect が弾くので通常は来ない。
        return Err("error.oauth.state_mismatch".into());
    }
    if let Some(err) = params.get("error") {
        let desc = params
            .get("error_description")
            .map(String::as_str)
            .unwrap_or("");
        return Err(format!("error.oauth.denied: {err} {desc}")
            .trim_end()
            .to_string());
    }
    let code = params
        .get("code")
        .cloned()
        .ok_or("リダイレクトに認可コードがありません。")?;

    // token 交換（form POST）。PKCE の code_verifier を送る。Google Desktop は非機密 secret も併送。
    exchange_code(
        &flow.token_endpoint,
        &flow.client_id,
        &code,
        &verifier,
        &redirect,
        flow.client_secret.as_deref(),
    )
}

/// 認可コード → token（form-urlencoded POST）。PKCE の code_verifier を送る。
/// `client_secret` は Google Desktop 型のみ Some（非機密）。Slack 直接フローは使わない。
fn exchange_code(
    token_endpoint: &str,
    client_id: &str,
    code: &str,
    verifier: &str,
    redirect: &str,
    client_secret: Option<&str>,
) -> Result<serde_json::Value, String> {
    let mut pairs: Vec<(&str, &str)> = vec![
        ("grant_type", "authorization_code"),
        ("client_id", client_id),
        ("code", code),
        ("code_verifier", verifier),
        ("redirect_uri", redirect),
    ];
    if let Some(secret) = client_secret {
        pairs.push(("client_secret", secret));
    }
    match ureq::post(token_endpoint).send_form(&pairs) {
        Ok(resp) => resp
            .into_json::<serde_json::Value>()
            .map_err(|e| format!("token 応答の JSON 解析に失敗: {e}")),
        // OAuth のエラーは 400 + JSON ボディで来る（Slack は 200 + ok:false なのでここには来ない）。
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            Err(format!("token エンドポイント {code}: {body}"))
        }
        Err(e) => Err(format!("token 交換の通信に失敗: {e}")),
    }
}

// ── Slack（Worker ブローカー経由の webhook-via-OAuth） ───────────────────────

/// Worker ブローカー経由 OAuth（Slack / Notion 共通）の 1 プロバイダ分の可変部。
/// この 6 値だけがプロバイダ差で、CSRF state 照合を含む本体は [`connect_via_broker`] が共有する。
struct BrokerConfig {
    /// URL とログの `[oauth/{slug}]` に使うスラッグ（"slack" / "notion"）。
    slug: &'static str,
    /// 連携エラーの表示名（"Slack" / "Notion"）。
    display: &'static str,
    /// Worker が 302 で返す応答パラメータ名（"webhook" / "notion"）。
    result_key: &'static str,
    /// 応答に値が無いときのエラー文（全文）。
    missing_msg: &'static str,
    /// 保存先キーチェーンキー。
    secrets_key: &'static str,
    /// 成功時のログ（全文）。
    success_log: &'static str,
}

/// loopback を立て Worker の start を開き、302 で返る値（webhook/token）を CSRF state 照合の上で
/// 検証・保存する共通本体。プロバイダ差は `cfg` の 6 値のみ（ログ・エラー文・保存キーは byte 不変）。
fn connect_via_broker(app: &AppHandle, cfg: &BrokerConfig) -> Result<(), String> {
    // Worker からの 302 を受ける loopback を立てる（プロバイダ側への登録は不要）。
    let (listener, port) = bind_loopback(BROKER_REDIRECT_PORTS)?;
    let state = gen_state();
    eprintln!(
        "[oauth/{}] loopback 127.0.0.1:{port} で待機 → ブラウザを開きます",
        cfg.slug
    );

    // ブローカー Worker の start を開く（loopback の port と CSRF 用 state を渡す）。
    let mut start = url::Url::parse(&format!("{WORKER_BASE}/oauth/{}/start", cfg.slug))
        .map_err(|e| format!("start URL: {e}"))?;
    start
        .query_pairs_mut()
        .append_pair("port", &port.to_string())
        .append_pair("state", &state);
    open_browser(app, start.as_str())?;

    // Worker が値（または error）を loopback へ 302 で返す（state が一致するリダイレクトだけが返る）。
    let params = wait_for_redirect(&listener, &state)?;
    eprintln!(
        "[oauth/{}] リダイレクト受領: keys={:?}",
        cfg.slug,
        params.keys().collect::<Vec<_>>()
    );
    if params.get("state").map(String::as_str) != Some(state.as_str()) {
        // state 不一致（CSRF 防止）。wait_for_redirect が弾くので通常は来ない。
        return Err("error.oauth.state_mismatch".into());
    }
    if let Some(err) = params.get("error") {
        return Err(format!("error.oauth.denied: {}: {err}", cfg.display));
    }
    let value = params
        .get(cfg.result_key)
        .map(String::as_str)
        .filter(|s| !s.is_empty())
        .ok_or(cfg.missing_msg)?;

    crate::secrets::set(cfg.secrets_key, value)?;
    eprintln!("{}", cfg.success_log);
    Ok(())
}

/// Slack と OAuth 連携し、得た Incoming Webhook URL を既存スロット（`slack_webhook_url`）へ保存する。
/// Slack は loopback を「非 web リダイレクト」とみなし bot スコープ（incoming-webhook）を拒否するため、
/// web(https) リダイレクト = mojiroku.com の Worker をブローカーにする。アプリは Worker の start URL を
/// 開くだけで、Worker が Slack 同意 → トークン交換まで行い、Webhook URL を loopback へ 302 で返す。
/// 得た Webhook URL を保存 → 既存の `SlackExporter` がそのまま使える（slack.rs 不変）。
/// **blocking**（loopback 待機）を内部の spawn_blocking で隔離する。
pub async fn connect_slack(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        connect_via_broker(
            &app,
            &BrokerConfig {
                slug: "slack",
                display: "Slack",
                result_key: "webhook",
                missing_msg: "Slack の応答に Webhook がありません。",
                secrets_key: crate::secrets::SLACK_WEBHOOK_KEY,
                success_log: "[oauth/slack] Webhook を受領しキーチェーンへ保存しました（連携完了）",
            },
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

// ── Notion（Worker ブローカー経由の REST OAuth） ─────────────────────────────

/// Notion と OAuth 連携し、得たアクセストークンを既存スロット（`notion_token`）へ保存する。
/// Notion の公開 OAuth は token 交換に client_secret（confidential client）が要るため、Slack と同じく
/// mojiroku.com の Worker をブローカーにする（Worker が client_id/secret を保持し code→token 交換まで行い、
/// アクセストークンを loopback へ 302 で返す）。Notion の標準 OAuth はトークン**無期限**なので refresh は
/// 不要 — Worker は連携時だけ通り、書き出しのホットパス（`NotionExporter`）には一切現れない（slack.rs と同型）。
/// 得たトークンを保存 → 既存の `NotionExporter` がそのまま使える（notion.rs 不変）。
/// **blocking**（loopback 待機）を内部の spawn_blocking で隔離する。
pub async fn connect_notion(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        connect_via_broker(
            &app,
            &BrokerConfig {
                slug: "notion",
                display: "Notion",
                result_key: "notion",
                missing_msg: "Notion の応答にトークンがありません。",
                secrets_key: crate::secrets::NOTION_TOKEN_KEY,
                success_log:
                    "[oauth/notion] トークンを受領しキーチェーンへ保存しました（連携完了）",
            },
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

// ── Google Calendar（loopback + PKCE 直接フロー・Worker 不要） ───────────────

/// Google OAuth クライアント（**Desktop app 型**）。client_id/secret とも Desktop 型では
/// 「秘密扱いしない」と Google が明言しているため同梱する（Worker 不要・トークンは端末↔Google 直結）。
/// 空なら connect は誘導エラー。
///
/// ローテーション手順（漏洩を疑う場合や定期更新）:
/// 1. GCP Console → API とサービス → 認証情報 → 該当 OAuth クライアント → 「シークレットを再発行」
/// 2. 下の GOOGLE_CLIENT_SECRET を新しい値に差し替えてリリース
/// 3. 旧シークレットを無効化（再発行時に選択可）。旧バイナリの連携は新規接続のみ失敗し、
///    保存済み refresh_token はそのまま使える
const GOOGLE_CLIENT_ID: &str =
    "346232126917-hr9l0f8cun26n8e9h53d97hcls2rmff3.apps.googleusercontent.com";
const GOOGLE_CLIENT_SECRET: &str = "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";
/// 予定の読み取りのみ（最小権限）。
const GOOGLE_SCOPE: &str = "https://www.googleapis.com/auth/calendar.events.readonly";

/// Keychain スロット（フロント types.ts の GOOGLE_* と一致させる）。
pub const GOOGLE_ACCESS_KEY: &str = "google_oauth_access";
pub const GOOGLE_REFRESH_KEY: &str = "google_oauth_refresh";
/// access token の失効時刻（epoch 秒の文字列）。
pub const GOOGLE_EXPIRY_KEY: &str = "google_token_expiry";

/// Google と OAuth 連携し、access/refresh token を Keychain に保存する。
/// Desktop 型 + PKCE + loopback（任意ポート）。refresh 取得のため access_type=offline / prompt=consent。
pub async fn connect_google(app: AppHandle) -> Result<(), String> {
    if GOOGLE_CLIENT_ID.is_empty() {
        return Err(
            "Google の Client ID が未設定です（src-tauri/src/oauth.rs の GOOGLE_CLIENT_ID）。"
                .into(),
        );
    }
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let flow = PkceFlow {
            client_id: GOOGLE_CLIENT_ID.to_string(),
            client_secret: (!GOOGLE_CLIENT_SECRET.is_empty()).then(|| GOOGLE_CLIENT_SECRET.to_string()),
            auth_endpoint: "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
            token_endpoint: "https://oauth2.googleapis.com/token".to_string(),
            scope: GOOGLE_SCOPE.to_string(),
            redirect_ports: Vec::new(), // ephemeral（Google は loopback の任意ポートを許可）
            extra_auth_params: vec![
                ("access_type".to_string(), "offline".to_string()),
                ("prompt".to_string(), "consent".to_string()),
            ],
        };
        let token = authorize_and_exchange(&app, &flow)?;
        let access = token
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or("Google 応答に access_token がありません。")?;
        let refresh = token
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .ok_or("Google 応答に refresh_token がありません（access_type=offline / prompt=consent を確認）。")?;
        let expires_in = token.get("expires_in").and_then(|v| v.as_i64()).unwrap_or(3600);
        store_google_tokens(access, Some(refresh), expires_in)?;
        eprintln!("[oauth/google] トークンを保存しました（連携完了）");
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 有効な Google access token を返す。失効（60 秒バッファ）していれば refresh_token で更新し保存する。
/// **blocking**（Keychain + ureq）なので呼び出し側の spawn_blocking 内で使うこと。
pub fn valid_google_access_token() -> Result<String, String> {
    let refresh = crate::secrets::get(GOOGLE_REFRESH_KEY)?
        .filter(|s| !s.trim().is_empty())
        .ok_or("error.calendar.not_connected")?;
    let access = crate::secrets::get(GOOGLE_ACCESS_KEY)?.unwrap_or_default();
    let expiry: i64 = crate::secrets::get(GOOGLE_EXPIRY_KEY)?
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    if !access.is_empty() && chrono::Utc::now().timestamp() < expiry - 60 {
        return Ok(access);
    }
    refresh_google(&refresh)
}

/// refresh_token で access token を更新し、保存して返す。
fn refresh_google(refresh_token: &str) -> Result<String, String> {
    let mut pairs: Vec<(&str, &str)> = vec![
        ("grant_type", "refresh_token"),
        ("client_id", GOOGLE_CLIENT_ID),
        ("refresh_token", refresh_token),
    ];
    if !GOOGLE_CLIENT_SECRET.is_empty() {
        pairs.push(("client_secret", GOOGLE_CLIENT_SECRET));
    }
    let json = match ureq::post("https://oauth2.googleapis.com/token").send_form(&pairs) {
        Ok(r) => r
            .into_json::<serde_json::Value>()
            .map_err(|e| format!("Google refresh 応答の JSON 解析に失敗: {e}"))?,
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            // refresh が無効化された（再認可が必要）場合も含む。
            return Err(format!("error.calendar.google_refresh: {code}: {body}"));
        }
        Err(e) => return Err(format!("error.calendar.google_refresh: {e}")),
    };
    let access = json
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or("Google refresh 応答に access_token がありません。")?;
    let expires_in = json
        .get("expires_in")
        .and_then(|v| v.as_i64())
        .unwrap_or(3600);
    // refresh_token は通常再発行されないので据え置き（access と expiry のみ更新）。
    store_google_tokens(access, None, expires_in)?;
    Ok(access.to_string())
}

/// access（+任意で refresh）と失効時刻を Keychain に保存する。
fn store_google_tokens(access: &str, refresh: Option<&str>, expires_in: i64) -> Result<(), String> {
    crate::secrets::set(GOOGLE_ACCESS_KEY, access)?;
    if let Some(r) = refresh {
        crate::secrets::set(GOOGLE_REFRESH_KEY, r)?;
    }
    let expiry = chrono::Utc::now().timestamp() + expires_in;
    crate::secrets::set(GOOGLE_EXPIRY_KEY, &expiry.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_b64url_sha256_of_verifier() {
        // RFC 7636 付録 B のベクタ: verifier の SHA256 を base64url(no-pad) したものが challenge。
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = challenge_of(verifier);
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn verifier_len_is_valid_pkce_range() {
        let v = gen_verifier();
        assert!(
            v.len() >= 43 && v.len() <= 128,
            "verifier len = {}",
            v.len()
        );
    }

    #[test]
    fn parse_query_decodes_pairs() {
        let p = parse_query("/?code=abc%20123&state=xyz");
        assert_eq!(p.get("code").map(String::as_str), Some("abc 123"));
        assert_eq!(p.get("state").map(String::as_str), Some("xyz"));
    }

    #[test]
    fn parse_query_empty_when_no_query() {
        assert!(parse_query("/").is_empty());
    }

    fn outcome(target: &str) -> RedirectOutcome {
        redirect_outcome(&parse_query(target), "expected")
    }

    #[test]
    fn redirect_without_matching_state_keeps_waiting() {
        // error / code / webhook があっても state が無い・違うなら待機を続ける。
        assert_eq!(outcome("/?error=access_denied"), RedirectOutcome::Ignore);
        assert_eq!(
            outcome("/?error=access_denied&state=other"),
            RedirectOutcome::Ignore
        );
        assert_eq!(outcome("/?code=abc&state=other"), RedirectOutcome::Ignore);
        assert_eq!(outcome("/?webhook=x&state="), RedirectOutcome::Ignore);
        assert_eq!(outcome("/favicon.ico"), RedirectOutcome::Ignore);
    }

    #[test]
    fn redirect_with_matching_state_completes() {
        assert_eq!(outcome("/?code=abc&state=expected"), RedirectOutcome::Done);
        assert_eq!(outcome("/?webhook=x&state=expected"), RedirectOutcome::Done);
        assert_eq!(outcome("/?notion=t&state=expected"), RedirectOutcome::Done);
        // 利用者の拒否も state が一致すれば完了として返す（呼び出し側が denied にする）。
        assert_eq!(
            outcome("/?error=access_denied&state=expected"),
            RedirectOutcome::Done
        );
    }

    #[test]
    fn matching_state_without_result_keeps_waiting() {
        assert_eq!(outcome("/?state=expected"), RedirectOutcome::Ignore);
    }
}
