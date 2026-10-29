//! 実バイナリを起動して stdio / streamable HTTP の両経路を通すスモークテスト。
//!
//! rmcp を上げたときに、MCP クライアント（Claude Desktop / Claude Code / claude.ai）から
//! 見た振る舞い（initialize → tools/list → tools/call、HTTP の Bearer 検証と Host 検証）が
//! 変わっていないことを確かめる。DB は一時ディレクトリに作るので実データには触れない。

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use mojiroku_core::store::SqliteStore;
use mojiroku_core::{ActionItem, Recording, Segment, SourceType, Speaker, Summary, Transcript};
use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_mojiroku-mcp");
/// `--http` の起動条件（32 文字以上）を満たす固定トークン。
const TOKEN: &str = "smoke-test-token-0123456789abcdef";
/// Claude Desktop / Claude Code が送ってくる版。サーバーはこれをそのまま返すべき。
const PROTOCOL: &str = "2025-06-18";
const REC_ID: &str = "rec-smoke";
const TIMEOUT: Duration = Duration::from_secs(30);

/// テスト終了時（panic 含む）に子プロセスを必ず止める。
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn fixture_db(dir: &Path) -> PathBuf {
    let path = dir.join("mojiroku.db");
    let store = SqliteStore::open(&path).expect("open fixture db");
    let rec = Recording {
        id: REC_ID.into(),
        source_type: SourceType::File,
        title: Some("予算会議".into()),
        duration_ms: 180_000,
        sample_rate: 16_000,
        created_at: "2026-10-01T10:00:00Z".into(),
    };
    let segment = |idx: u32, text: &str, speaker: &str| Segment {
        idx,
        start_ms: u64::from(idx) * 4_000,
        end_ms: u64::from(idx + 1) * 4_000,
        text: text.into(),
        speaker_id: Some(speaker.into()),
    };
    let transcript = Transcript {
        language: Some("ja".into()),
        segments: vec![
            segment(0, "来期の予算を確認します", "S1"),
            segment(1, "MVP の範囲は据え置きです", "S2"),
        ],
    };
    let speakers = [
        Speaker {
            id: "S1".into(),
            label: "S1".into(),
            display_name: Some("田中".into()),
        },
        Speaker {
            id: "S2".into(),
            label: "S2".into(),
            display_name: None,
        },
    ];
    store
        .save_recording(&rec, &transcript, &speakers)
        .expect("save recording");
    store
        .save_summary(
            REC_ID,
            &Summary {
                template_id: "standard".into(),
                content: "予算は据え置き。".into(),
                action_items: vec![ActionItem {
                    text: "見積もりを更新".into(),
                    assignee: Some("田中".into()),
                    due: None,
                }],
                stale: false,
            },
        )
        .expect("save summary");
    path
}

/// 読み取りを別スレッドに逃がし、応答が来ないときにテストが固まらないようにする。
fn line_reader(r: impl Read + Send + 'static) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(r).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

fn request(id: u64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn initialize_params() -> Value {
    json!({
        "protocolVersion": PROTOCOL,
        "capabilities": {},
        "clientInfo": { "name": "mojiroku-mcp-smoke", "version": "0" },
    })
}

/// tools/call の結果（JSON 文字列のテキストコンテンツ 1 個）を取り出す。
fn tool_json(resp: &Value) -> Value {
    assert_ne!(resp["result"]["isError"], json!(true), "tool error: {resp}");
    let text = resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no text content: {resp}"));
    serde_json::from_str(text).expect("tool output is JSON")
}

/// initialize 済みのセッションで、3 ツールの一覧と呼び出し結果を確かめる。
fn exercise_tools(call: &mut dyn FnMut(&str, Value) -> Value) {
    let list = call("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().expect("tools array");
    let mut names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        ["get_meeting", "list_recent_meetings", "search_meetings"]
    );
    let search = tools
        .iter()
        .find(|t| t["name"] == "search_meetings")
        .unwrap();
    assert_eq!(search["inputSchema"]["required"], json!(["query"]));

    // 空白区切りは OR 検索（片方がヒットしなくても結果は出る）。
    let hits = tool_json(&call(
        "tools/call",
        json!({ "name": "search_meetings", "arguments": { "query": "来期の予算 存在しない語" } }),
    ));
    assert_eq!(hits[0]["recording_id"], REC_ID, "{hits}");

    let recent = tool_json(&call(
        "tools/call",
        json!({ "name": "list_recent_meetings", "arguments": {} }),
    ));
    assert_eq!(recent[0]["recording_id"], REC_ID, "{recent}");
    assert_eq!(recent[0]["duration_minutes"], 3);

    let meeting = tool_json(&call(
        "tools/call",
        json!({
            "name": "get_meeting",
            "arguments": { "recording_id": REC_ID, "include_transcript": true },
        }),
    ));
    assert_eq!(meeting["speakers"][0]["name"], "田中", "{meeting}");
    assert_eq!(
        meeting["summaries"][0]["action_items"][0]["text"],
        "見積もりを更新"
    );
    let transcript = meeting["transcript"].as_str().expect("transcript");
    assert!(
        transcript.contains("MVP の範囲は据え置きです"),
        "{transcript}"
    );

    // 存在しない id は JSON-RPC エラーとして返る（ツール結果の isError ではない）。
    let missing = call(
        "tools/call",
        json!({ "name": "get_meeting", "arguments": { "recording_id": "nope" } }),
    );
    let message = missing["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("not found"), "{missing}");
}

#[test]
fn stdio_serves_tools() {
    let dir = tempfile::tempdir().unwrap();
    let db = fixture_db(dir.path());
    let mut child = ChildGuard(
        Command::new(BIN)
            .arg("--db")
            .arg(&db)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn mojiroku-mcp"),
    );
    let mut stdin = child.0.stdin.take().unwrap();
    let stdout = line_reader(child.0.stdout.take().unwrap());

    let mut next_id = 0;
    let mut call = |method: &str, params: Value| -> Value {
        if method.starts_with("notifications/") {
            writeln!(stdin, "{}", json!({ "jsonrpc": "2.0", "method": method })).unwrap();
            stdin.flush().unwrap();
            return Value::Null;
        }
        next_id += 1;
        writeln!(stdin, "{}", request(next_id, method, params)).unwrap();
        stdin.flush().unwrap();
        loop {
            let line = stdout
                .recv_timeout(TIMEOUT)
                .unwrap_or_else(|_| panic!("no response to {method}"));
            // stdout は JSON-RPC 専用。ログが 1 行でも混ざればここで落ちる。
            let msg: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("non JSON-RPC line on stdout ({e}): {line}"));
            if msg["id"] == json!(next_id) {
                return msg;
            }
        }
    };

    let init = call("initialize", initialize_params());
    assert_eq!(init["result"]["protocolVersion"], PROTOCOL, "{init}");
    assert!(
        init["result"]["capabilities"]["tools"].is_object(),
        "{init}"
    );
    assert!(init["result"]["instructions"]
        .as_str()
        .is_some_and(|s| s.contains("search_meetings")));
    call("notifications/initialized", Value::Null);
    exercise_tools(&mut call);
}

/// SSE 応答（`data: {...}` 行）か素の JSON 応答から、指定 id の JSON-RPC 応答を取り出す。
fn rpc_body(body: &str, id: u64) -> Value {
    body.lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .chain(std::iter::once(body))
        .filter_map(|s| serde_json::from_str::<Value>(s.trim()).ok())
        .find(|m| m["id"] == json!(id))
        .unwrap_or_else(|| panic!("no response for id {id}: {body}"))
}

#[test]
fn http_checks_bearer_and_host_then_serves_tools() {
    let dir = tempfile::tempdir().unwrap();
    let db = fixture_db(dir.path());
    let tunnel_host = "mcp-origin.example.com";
    let mut child = ChildGuard(
        Command::new(BIN)
            .arg("--db")
            .arg(&db)
            .args(["--http", "0", "--allowed-host", tunnel_host])
            .env("MOJIROKU_MCP_TOKEN", TOKEN)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn mojiroku-mcp --http"),
    );
    // ポート 0 で起動し、待ち受けログから実際の URL を拾う。stderr は最後まで読み続ける。
    let stderr = line_reader(child.0.stderr.take().unwrap());
    let url = loop {
        let line = stderr.recv_timeout(TIMEOUT).expect("server did not start");
        if let Some(i) = line.find("http://127.0.0.1:") {
            break line[i..].trim().to_string();
        }
    };

    let agent = ureq::AgentBuilder::new().timeout(TIMEOUT).build();
    let post = |host: &str, token: &str, session: Option<&str>, body: &Value| {
        let mut req = agent
            .post(&url)
            .set("Host", host)
            .set("Authorization", &format!("Bearer {token}"))
            .set("Content-Type", "application/json")
            .set("Accept", "application/json, text/event-stream");
        if let Some(session) = session {
            req = req
                .set("Mcp-Session-Id", session)
                .set("MCP-Protocol-Version", PROTOCOL);
        }
        match req.send_string(&body.to_string()) {
            Ok(r) | Err(ureq::Error::Status(_, r)) => (
                r.status(),
                r.header("Mcp-Session-Id").map(str::to_owned),
                r.into_string().unwrap_or_default(),
            ),
            Err(e) => panic!("transport error: {e}"),
        }
    };

    let init = request(1, "initialize", initialize_params());
    // トークン不一致は rmcp に届く前に一律 401。
    let (status, _, _) = post(tunnel_host, "wrong-token-0123456789abcdef0123", None, &init);
    assert_eq!(status, 401);
    // --allowed-host に無い Host は rmcp の DNS rebinding 対策で弾かれる。
    let (status, _, _) = post("attacker.example.com", TOKEN, None, &init);
    assert_eq!(status, 403);

    // Tunnel 経由と同じ Host で 1 セッションを通す。
    let (status, session, body) = post(tunnel_host, TOKEN, None, &init);
    assert_eq!(status, 200, "{body}");
    assert_eq!(rpc_body(&body, 1)["result"]["protocolVersion"], PROTOCOL);
    let session = session.expect("Mcp-Session-Id header");
    let initialized = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    let (status, _, body) = post(tunnel_host, TOKEN, Some(&session), &initialized);
    assert_eq!(status, 202, "{body}");

    let mut next_id = 1;
    exercise_tools(&mut |method, params| {
        next_id += 1;
        let req = request(next_id, method, params);
        let (status, _, body) = post(tunnel_host, TOKEN, Some(&session), &req);
        assert_eq!(status, 200, "{body}");
        rpc_body(&body, next_id)
    });
}
