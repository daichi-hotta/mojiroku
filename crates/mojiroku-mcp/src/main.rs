//! mojiroku ローカル MCP サーバー（stdio / streamable HTTP）。ADR-0010 / ADR-0025。
//!
//! 履歴 DB（`mojiroku.db`）を**読み取り専用**で開き、Claude Desktop / Claude Code 等の
//! MCP クライアントから自分の議事録を検索・参照できるようにする。ローカル完結・$0。
//!
//! 既定は従来通り stdio。`--http <port>` を渡すと streamable HTTP サーバーとして
//! `127.0.0.1:<port>/mcp` で待ち受ける（Cloudflare Tunnel 経由で claude.ai から使う経路。
//! Bearer トークン必須・loopback bind のみ）。
//!
//! **stdio モードでは stdout は JSON-RPC 専用**（rmcp の stdio transport が使う）。ログ・
//! panic・診断はすべて **stderr** へ出す。stdout に 1 行でも混ざると JSON-RPC ストリームが
//! 壊れる（HTTP モードでも同じ規約に揃える）。
//!
//! DB は `SqliteStore::open_readonly`（migrate しない・スキーマを触らない）で開く。
//! アプリ本体（writer）と並行可能（WAL）で、アプリ非起動でも履歴を読める。

use std::{path::PathBuf, sync::Arc};

use axum::response::IntoResponse;
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerConfig},
    tool, tool_handler, tool_router,
    transport::{
        stdio,
        streamable_http_server::{
            session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
        },
    },
    ErrorData, ServerHandler, ServiceExt,
};
use serde::Serialize;

use mojiroku_core::store::SqliteStore;
use mojiroku_core::summarize::transcript_to_text;

// ============================ ツール入力 ============================

// 引数 doc（schemars 経由でツールスキーマに載る）とツール description の読者は LLM クライアント
// なので**英語に一本化**する（アプリ設定の言語では切り替えない。会議データ自体は日英どちらでも
// 検索・参照できる）。
#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SearchArgs {
    /// Search keywords. Pass **short terms** that identify the meeting (e.g. "MVP", "budget",
    /// "kickoff"). Full-text search matches contiguous substrings, so a natural-language
    /// question (e.g. "what was decided in last week's MVP meeting") tends to miss.
    /// **Separate multiple terms with spaces** to OR-search them and merge the results.
    query: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct GetMeetingArgs {
    /// recording_id of the target meeting (an id returned by search_meetings /
    /// list_recent_meetings).
    recording_id: String,
    /// When true, also return the verbatim full transcript. Defaults to false because it can
    /// be very long.
    #[serde(default)]
    include_transcript: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ListRecentArgs {
    /// Maximum number of meetings to return (default 20).
    #[serde(default)]
    limit: Option<usize>,
}

// ============================ ツール出力（LLM 向け・可読フィールド）============================

#[derive(Serialize)]
struct MeetingRef {
    recording_id: String,
    title: String,
    created_at: String,
    duration_minutes: u64,
}

#[derive(Serialize)]
struct SearchHitOut {
    recording_id: String,
    title: String,
    created_at: String,
    /// マッチ箇所のスニペット（全文ではない）。
    snippet: String,
}

#[derive(Serialize)]
struct SpeakerOut {
    id: String,
    /// 表示名（ユーザー改名があればそれ、無ければ既定ラベル「話者N」）。
    name: String,
}

#[derive(Serialize)]
struct ActionItemOut {
    text: String,
    assignee: Option<String>,
    due: Option<String>,
}

#[derive(Serialize)]
struct SummaryOut {
    template_id: String,
    content: String,
    action_items: Vec<ActionItemOut>,
}

#[derive(Serialize)]
struct MeetingOut {
    recording_id: String,
    title: String,
    created_at: String,
    duration_minutes: u64,
    language: Option<String>,
    speakers: Vec<SpeakerOut>,
    summaries: Vec<SummaryOut>,
    /// include_transcript=true のときだけ含む話者ラベル付き逐語全文。
    #[serde(skip_serializing_if = "Option::is_none")]
    transcript: Option<String>,
}

// ============================ サーバ ============================

#[derive(Clone)]
struct MojirokuMcp {
    // SqliteStore は Mutex<Connection> を内包し非 Clone。サーバは rmcp により clone され得るので Arc 共有。
    store: std::sync::Arc<SqliteStore>,
    // #[tool_handler] が生成するコードから参照されるが、lint はマクロ生成の read を見られず誤検知する。
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl MojirokuMcp {
    fn new(store: SqliteStore) -> Self {
        Self {
            store: std::sync::Arc::new(store),
            tool_router: Self::tool_router(),
        }
    }
}

fn title_of(r: &mojiroku_core::Recording) -> String {
    r.title.clone().unwrap_or_else(|| "(untitled)".to_string())
}

fn db_err(ctx: &str, e: impl std::fmt::Display) -> ErrorData {
    ErrorData::internal_error(format!("{ctx}: {e}"), None)
}

fn to_json(v: &impl Serialize) -> Result<String, ErrorData> {
    serde_json::to_string_pretty(v).map_err(|e| db_err("serialize", e))
}

#[tool_router]
impl MojirokuMcp {
    #[tool(
        description = "Full-text search over locally stored meeting notes (matches transcript \
                       body and title; works for Japanese and English content). Pass **short \
                       keywords** that identify the meeting, not a natural-language question. \
                       Multiple space-separated terms are OR-searched. Pass a hit's \
                       recording_id to get_meeting for details."
    )]
    fn search_meetings(
        &self,
        Parameters(SearchArgs { query }): Parameters<SearchArgs>,
    ) -> Result<String, ErrorData> {
        // LLM クライアントは自然文を渡しがちだが、store の全文検索は「クエリ全体＝1 フレーズの
        // 連続部分文字列一致」（UI 検索ボックス向けの意味論）。ここ（MCP 層）だけ寛容化し、
        // 空白区切りの各語を OR 検索して recording_id で union する（UI の search_recordings は無変更）。
        let terms: Vec<&str> = query.split_whitespace().collect();
        let hits = if terms.len() <= 1 {
            self.store
                .search_recordings(query.trim())
                .map_err(|e| db_err("search", e))?
        } else {
            let mut seen = std::collections::HashSet::new();
            let mut merged = Vec::new();
            for term in terms {
                let part = self
                    .store
                    .search_recordings(term)
                    .map_err(|e| db_err("search", e))?;
                for h in part {
                    if seen.insert(h.recording.id.clone()) {
                        merged.push(h);
                    }
                }
            }
            merged
        };
        let out: Vec<SearchHitOut> = hits
            .into_iter()
            .map(|h| SearchHitOut {
                recording_id: h.recording.id.clone(),
                title: title_of(&h.recording),
                created_at: h.recording.created_at,
                snippet: h.snippet,
            })
            .collect();
        to_json(&out)
    }

    #[tool(
        description = "List recent locally stored meetings, newest first. Pass a recording_id \
                       to get_meeting for details."
    )]
    fn list_recent_meetings(
        &self,
        Parameters(ListRecentArgs { limit }): Parameters<ListRecentArgs>,
    ) -> Result<String, ErrorData> {
        let limit = limit.unwrap_or(20);
        let recs = self
            .store
            .list_recordings()
            .map_err(|e| db_err("list", e))?;
        let out: Vec<MeetingRef> = recs
            .into_iter()
            .take(limit)
            .map(|r| MeetingRef {
                recording_id: r.id.clone(),
                title: title_of(&r),
                created_at: r.created_at,
                duration_minutes: r.duration_ms / 60_000,
            })
            .collect();
        to_json(&out)
    }

    #[tool(
        description = "Get the details of a meeting (summaries, metadata, speakers). The \
                       verbatim transcript is omitted by default; set include_transcript=true \
                       to also return the full speaker-labeled transcript (can be very long)."
    )]
    fn get_meeting(
        &self,
        Parameters(GetMeetingArgs {
            recording_id,
            include_transcript,
        }): Parameters<GetMeetingArgs>,
    ) -> Result<String, ErrorData> {
        let detail = self
            .store
            .get_recording_detail(&recording_id)
            .map_err(|e| db_err("get_meeting", e))?
            .ok_or_else(|| {
                ErrorData::resource_not_found(
                    format!("recording_id not found: {recording_id}"),
                    None,
                )
            })?;

        let speakers = detail
            .speakers
            .iter()
            .map(|s| SpeakerOut {
                id: s.id.clone(),
                name: s.display_name.clone().unwrap_or_else(|| s.label.clone()),
            })
            .collect();
        let summaries = detail
            .summaries
            .iter()
            .map(|s| SummaryOut {
                template_id: s.template_id.clone(),
                content: s.content.clone(),
                action_items: s
                    .action_items
                    .iter()
                    .map(|a| ActionItemOut {
                        text: a.text.clone(),
                        assignee: a.assignee.clone(),
                        due: a.due.clone(),
                    })
                    .collect(),
            })
            .collect();
        // 話者ラベルのフォールバック（生の "S1" → 「話者N」/ "Speaker N"）は、その録音の
        // 文字起こし言語に合わせる（DB 内容の表示であり、アプリ設定には依存させない）。
        let lang = mojiroku_core::lang::Lang::from_code(
            detail.transcript.language.as_deref().unwrap_or("ja"),
        );
        let transcript = include_transcript.then(|| transcript_to_text(&detail.transcript, lang));

        to_json(&MeetingOut {
            recording_id: detail.recording.id.clone(),
            title: title_of(&detail.recording),
            created_at: detail.recording.created_at.clone(),
            duration_minutes: detail.recording.duration_ms / 60_000,
            language: detail.transcript.language.clone(),
            speakers,
            summaries,
            transcript,
        })
    }
}

#[tool_handler]
impl ServerHandler for MojirokuMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Search and read the user's local mojiroku meeting notes (recording -> transcript \
             -> summaries, with speaker separation). Start with search_meetings to find a \
             meeting, or list_recent_meetings for the latest ones, then pass the recording_id \
             to get_meeting for summaries, speakers, and (optionally) the full transcript. \
             Give search_meetings short identifying keywords (e.g. \"MVP\", \"budget\"), not a \
             natural-language question. If a search misses, try shorter or different terms, or \
             list recent meetings and narrow down with get_meeting.",
        )
    }
}

// ============================ CLI ============================

/// `--http` 時の設定。
struct HttpConfig {
    /// bind は常に 127.0.0.1（LAN へは露出しない。外部公開は Cloudflare Tunnel の仕事）。
    port: u16,
    /// rmcp の Host ヘッダ検証（DNS rebinding 対策）に追加で許可するホスト名。
    /// 既定は loopback のみなので、Tunnel 経由（Host: mcp-origin.example.com）は
    /// `--allowed-host` で明示しないと弾かれる。
    extra_allowed_hosts: Vec<String>,
}

struct Cli {
    db_path: PathBuf,
    /// None = 従来通り stdio。
    http: Option<HttpConfig>,
}

/// 受け付ける引数: `--db <path>` / `--http <port>` / `--allowed-host <host>`（繰り返し可、
/// いずれも `--flag=value` 形式も可）。未知の引数はエラー（タイポで意図せず stdio モードに
/// 落ちて launchd がハングする事故を防ぐ）。
fn parse_cli() -> Result<Cli, String> {
    let mut db: Option<PathBuf> = None;
    let mut http_port: Option<u16> = None;
    let mut extra_allowed_hosts: Vec<String> = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let (flag, inline_value) = match arg.split_once('=') {
            Some((f, v)) => (f.to_owned(), Some(v.to_owned())),
            None => (arg, None),
        };
        let mut take_value = || -> Result<String, String> {
            inline_value
                .clone()
                .or_else(|| args.next())
                .ok_or_else(|| format!("{flag} requires a value"))
        };
        match flag.as_str() {
            "--db" => db = Some(PathBuf::from(take_value()?)),
            "--http" => {
                let v = take_value()?;
                http_port = Some(v.parse().map_err(|_| format!("invalid --http port: {v}"))?);
            }
            "--allowed-host" => extra_allowed_hosts.push(take_value()?),
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    // DB パスの優先順: `--db` 引数 → 環境変数 `MOJIROKU_DB` → 既定 app_data パス。
    let db_path = db
        .or_else(|| std::env::var_os("MOJIROKU_DB").map(PathBuf::from))
        .unwrap_or_else(default_db_path);
    Ok(Cli {
        db_path,
        http: http_port.map(|port| HttpConfig {
            port,
            extra_allowed_hosts,
        }),
    })
}

/// macOS の既定: `~/Library/Application Support/com.daichi0812.mojiroku/mojiroku.db`。
fn default_db_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Library/Application Support/com.daichi0812.mojiroku/mojiroku.db")
}

// ============================ HTTP モード（ADR-0025） ============================

/// `Authorization: Bearer <token>` が期待値と一致するか。
fn bearer_token_matches(headers: &axum::http::HeaderMap, expected: &str) -> bool {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|got| constant_time_eq(got.as_bytes(), expected.as_bytes()))
}

/// タイミング攻撃でトークンを先頭から 1 文字ずつ確定されないよう、長さ一致時は
/// 全バイトを必ず比較する（早期 return しない）。
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn run_http(
    handler: MojirokuMcp,
    http: HttpConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    // トークンは `ps` に映る CLI 引数ではなく環境変数で受ける。未設定・短すぎは起動拒否
    // （fail-closed。認証なしで議事録が公開される事故を構成ミスの段階で止める）。
    let token = std::env::var("MOJIROKU_MCP_TOKEN").unwrap_or_default();
    if token.len() < 32 {
        return Err(
            "--http requires env MOJIROKU_MCP_TOKEN (>= 32 chars, e.g. `openssl rand -hex 32`)"
                .into(),
        );
    }
    let token: Arc<str> = token.into();

    let mut config = StreamableHttpServerConfig::default();
    config
        .allowed_hosts
        .extend(http.extra_allowed_hosts.iter().cloned());

    let mcp_service = StreamableHttpService::new(
        move || Ok(handler.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    );

    let app =
        axum::Router::new()
            .nest_service("/mcp", mcp_service)
            .layer(axum::middleware::from_fn({
                let token = token.clone();
                move |req: axum::extract::Request, next: axum::middleware::Next| {
                    let token = token.clone();
                    async move {
                        if bearer_token_matches(req.headers(), &token) {
                            next.run(req).await
                        } else {
                            // 失敗理由（ヘッダ欠落/形式不正/不一致）は区別せず一律 401。
                            (
                                axum::http::StatusCode::UNAUTHORIZED,
                                [(axum::http::header::WWW_AUTHENTICATE, "Bearer")],
                            )
                                .into_response()
                        }
                    }
                }
            }));

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], http.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(
        "serving MCP (streamable HTTP, bearer auth) at http://{}/mcp",
        listener.local_addr()?
    );
    axum::serve(listener, app).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ログは必ず stderr（stdio モードの stdout は JSON-RPC 専用）。ANSI 無効でログファイル可読に。
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    // panic も stderr へ（既定でも stderr だが、stdout 汚染を確実に避けるため明示）。
    std::panic::set_hook(Box::new(|info| {
        eprintln!("[mojiroku-mcp] panic: {info}");
    }));

    let cli = parse_cli()?;
    tracing::info!("opening db (read-only): {}", cli.db_path.display());
    // open_readonly は本バイナリの `SCHEMA_VERSION` より**新しい** DB を拒否する（未来スキーマガード）。
    // 本体を v5 化（ADR-0024）したら、v4 時にビルドした古い MCP は v5 DB を開けなくなるため、
    // **MCP は本体と同じ v5 core から再ビルドして同時に配布する**（リリース順序が load-bearing）。
    // v5 core の `get_recording_detail` は `stale`（column_exists）/`active_job`（table_exists）を
    // ガード付きで読むので、v5 DB も未 migrate の旧 DB も安全に読める。
    let store = SqliteStore::open_readonly(&cli.db_path)
        .map_err(|e| format!("DB open failed ({}): {e}", cli.db_path.display()))?;
    let handler = MojirokuMcp::new(store);

    match cli.http {
        Some(http) => run_http(handler, http).await?,
        None => {
            let service = handler
                .serve(stdio())
                .await
                .inspect_err(|e| tracing::error!("serve error: {e:?}"))?;
            service.waiting().await?;
        }
    }
    Ok(())
}
