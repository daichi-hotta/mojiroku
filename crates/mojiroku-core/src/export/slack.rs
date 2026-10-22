//! Slack へ要約を投稿（Incoming Webhook = BYOK, $0, OAuth 不要）。
//!
//! ⚠️ 送信時に**要約**が Slack のサーバへ出る（プライバシーのトレードオフ）。Notion と違い
//! **文字起こしは送らない**（Slack = ダイジェスト）。webhook URL 自体が秘密でキーチェーン管轄
//! （src-tauri の `secrets`）。チャンネルは webhook 作成時に固定され、リクエストでは変えられない。
//!
//! 裏取り済み（docs.slack.dev）: POST `https://hooks.slack.com/services/T.../B.../XXX`、
//! body `{text, blocks}`、成功は 200 + 本文 `"ok"`、失敗は `no_service`/`channel_not_found`/
//! `invalid_payload`。Block Kit は 50 ブロック/メッセージ、section(mrkdwn) ≤3000 字、header(plain_text) ≤150 字。

use super::common::{cap_chars, meeting_title, parse_bullet, template_label};
use crate::error::{CoreError, Result};
use crate::lang::Lang;
use crate::store::RecordingDetail;
use serde_json::{json, Value};

/// Incoming Webhook URL の必須プレフィックス（誤 URL で任意ホストへ要約を流さないため検証する）。
const WEBHOOK_PREFIX: &str = "https://hooks.slack.com/services/";
/// Slack: 1 メッセージ最大 50 ブロック。
const MAX_BLOCKS: usize = 50;
/// Slack: section の mrkdwn テキストは最大 3000 文字。
const SECTION_TEXT_LIMIT: usize = 3000;
/// Slack: header の plain_text は最大 150 文字。
const HEADER_TEXT_LIMIT: usize = 150;
/// HTTP 全体タイムアウト（秒）。応答ストールでスレッドが無期限ブロックしない保険。
const HTTP_TIMEOUT_SECS: u64 = 30;

/// Slack へ要約を投稿する BYOK エクスポータ（webhook URL = 秘密）。
pub struct SlackExporter {
    /// Incoming Webhook URL（キーチェーン由来）。チャンネルはこの URL に内包。
    pub webhook_url: String,
    /// ラベル・既定タイトルのコンテンツ言語（エクスポート実行時のアプリ設定に追従）。
    pub lang: Lang,
}

impl SlackExporter {
    /// 要約を Slack チャンネルへ投稿する。要約が無ければエラー（空メッセージを投げない）。
    pub fn export(&self, detail: &RecordingDetail) -> Result<()> {
        let url = validate_webhook(&self.webhook_url)?;
        if detail.summaries.is_empty() {
            return Err(CoreError::Model(
                "error.export.slack_no_summary".to_string(),
            ));
        }
        let title = meeting_title(detail.recording.title.as_deref(), self.lang);
        let blocks = build_blocks(detail, title, self.lang);
        let fallback = fallback_text(title);

        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
            .build();

        // 50 ブロック制限 → 超えたら複数メッセージへ分割（無言の切り捨てはしない。通常は 1 通）。
        for chunk in blocks.chunks(MAX_BLOCKS) {
            let body = json!({ "text": fallback, "blocks": chunk });
            post(&agent, &url, body)?;
        }
        Ok(())
    }
}

/// webhook へ POST し、200 + 本文 "ok" を成功とする。
fn post(agent: &ureq::Agent, url: &str, body: Value) -> Result<()> {
    let resp = agent.post(url).send_json(body).map_err(slack_err)?;
    let s = resp.into_string().unwrap_or_default();
    if s.trim() != "ok" {
        return Err(CoreError::Model(format!(
            "slack: 想定外の応答: {}",
            s.chars().take(200).collect::<String>()
        )));
    }
    Ok(())
}

/// webhook URL を検証して返す。`hooks.slack.com/services/` で始まらなければ拒否
/// （タイプミス/誤 URL で要約を任意ホストへ送らないためのガード）。
fn validate_webhook(raw: &str) -> Result<String> {
    let s = raw.trim();
    if !s.starts_with(WEBHOOK_PREFIX) {
        return Err(CoreError::Model(format!(
            "Slack Webhook URL の形式が不正です（{WEBHOOK_PREFIX}… で始まる必要があります）。設定 → 連携 を確認してください。"
        )));
    }
    Ok(s.to_string())
}

/// Slack エラーを `error.export.*` の安定キーに整形する（表示文言はフロントの i18n 辞書が持ち、
/// `no_service`/`channel_not_found`＝webhook 無効系 を専用キーで区別する）。HTTP コード・応答ボディは
/// キーの後ろに ": " で連結して詳細として届ける。
///
/// ⚠️ Transport ブランチは `ureq::Error` の `Display`（`{e}`）を**使わない**。ureq 2.x の
/// `Display for Transport` は失敗 URL を先頭に出すため、それを使うと**秘密の webhook URL**が
/// エラー文字列→トースト→JS まで漏れる（webhook URL 自体がクレデンシャル）。URL を含まない
/// `kind()` の分類（"Dns Failed"/"Network Error" 等）のみを出す。
fn slack_err(e: ureq::Error) -> CoreError {
    match e {
        ureq::Error::Status(code, resp) => {
            let body: String = resp
                .into_string()
                .unwrap_or_default()
                .chars()
                .take(300)
                .collect();
            let key = match body.trim() {
                "no_service" | "channel_not_found" => "error.export.slack_webhook_invalid",
                _ => "error.export.slack_api",
            };
            CoreError::Model(format!("{key}: {code} {body}"))
        }
        // {other} は使わない（URL 漏洩防止）。kind() は分類のみで URL を含まない。
        other => CoreError::Model(format!("error.export.slack_api: {}", other.kind())),
    }
}

/// blocks を表示できない通知に出すトップレベルの `text`。Slack はこの欄も mrkdwn として解釈するので、
/// section と同じく `&`/`<`/`>` をエスケープする（タイトルの `<!channel>` 等をメンションにしない）。
/// 太字変換（`inline_mrkdwn`）は通さない（`__init__` のようなタイトルを書き換えないため）。
fn fallback_text(title: &str) -> String {
    format!("📝 {}", escape_mrkdwn(title))
}

/// 要約だけを Slack ブロックへ組む（header + 各要約の *ラベル* + 本文 section）。
fn build_blocks(detail: &RecordingDetail, title: &str, lang: Lang) -> Vec<Value> {
    let mut blocks: Vec<Value> = vec![header_block(title)];
    for s in &detail.summaries {
        blocks.push(section_block(&format!(
            "*{}*",
            template_label(&s.template_id, lang)
        )));
        let mrkdwn = md_to_mrkdwn(&s.content);
        for chunk in chunk_chars(&mrkdwn, SECTION_TEXT_LIMIT) {
            if !chunk.trim().is_empty() {
                blocks.push(section_block(&chunk));
            }
        }
    }
    blocks
}

/// LLM Markdown を Slack mrkdwn へ変換（除去ではなく**変換**）。
/// 見出し `#`/`##`… → `*太字*`、箇条書き `-`/`*`/`+` → `• `、`**bold**`/`__bold__` → `*bold*`。
/// ⚠️ 厳密な変換ではない（インラインは素朴置換。MVP 制限。UI で開示）。
fn md_to_mrkdwn(md: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for raw in md.lines() {
        let t = raw.trim();
        if t.is_empty() || is_thematic_break(t) {
            // 空行と区切り線（`---`/`***`/`___`）は段落区切りに畳む。Slack mrkdwn は
            // 水平線を持たず `---` がそのまま表示されるため、区切り線は落とす。
            out.push(String::new());
            continue;
        }
        let line = if let Some(rest) = parse_heading(t) {
            format!("*{}*", inline_mrkdwn(rest))
        } else if let Some(rest) = parse_bullet(t) {
            format!("• {}", inline_mrkdwn(rest))
        } else {
            inline_mrkdwn(t)
        };
        out.push(line);
    }
    out.join("\n")
}

/// インライン: section(mrkdwn) 用にテキストを整形する。
/// Slack mrkdwn は `&`/`<`/`>` を制御文字（リンク/メンション markup）として扱うため、
/// 先にエンティティへエスケープしないと `Vec<String>` や `<未定>` のような角括弧表記が
/// 括弧ごと食われて本文が消える（開発者の議事録でジェネリクスが頻出するため現実的）。
/// `&` を最初に置換する（順序を誤ると `&lt;` 自体が再エスケープされる）。エスケープ後に
/// Markdown の太字 `**`/`__` を Slack の太字 `*` へ変換する。
/// ⚠️ header（plain_text）はこの経路を通らない（plain_text はエンティティ不要）。
fn inline_mrkdwn(s: &str) -> String {
    escape_mrkdwn(s).replace("**", "*").replace("__", "*")
}

/// Slack mrkdwn の制御文字 `&`/`<`/`>` をエンティティへエスケープする（`&` が先）。
fn escape_mrkdwn(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// `# 見出し` の本文を返す（レベルは Slack では不要なので捨てる）。`#tag` は見出し扱いしない。
/// LLM が見出しを二重マークするケース（例: `## # 議題`）に備え、先頭の `#` と空白の連なりを
/// まとめて剥がす（残った `#` が太字内にリテラル表示されるのを防ぐ）。
fn parse_heading(line: &str) -> Option<&str> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if hashes == 0 {
        return None;
    }
    // 直後に空白が無い `#foo`（ハッシュタグ等）は見出し扱いしない。
    if !line[hashes..].starts_with(' ') {
        return None;
    }
    Some(line[hashes..].trim_start_matches(['#', ' ']))
}

/// 区切り線（`---`/`***`/`___` が 3 個以上、他の文字を含まない行）か。
fn is_thematic_break(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 3
        && (t.bytes().all(|b| b == b'-')
            || t.bytes().all(|b| b == b'*')
            || t.bytes().all(|b| b == b'_'))
}

fn header_block(title: &str) -> Value {
    json!({
        "type": "header",
        "text": { "type": "plain_text", "text": cap_chars(title, HEADER_TEXT_LIMIT) }
    })
}

fn section_block(text: &str) -> Value {
    json!({ "type": "section", "text": { "type": "mrkdwn", "text": text } })
}

/// 文字列を limit 文字（char 単位、マルチバイト安全）ごとに分割。空文字は空配列。
/// ⚠️ 既知の制限（low）: 単一要約が 3000 字超で `*bold*` マーカーが境界を跨ぐと、
/// 前後 section で `*` が不均衡になり太字が崩れる（表示のみ・データ欠落なし）。MVP では許容。
fn chunk_chars(s: &str, limit: usize) -> Vec<String> {
    if s.is_empty() {
        return Vec::new();
    }
    s.chars()
        .collect::<Vec<_>>()
        .chunks(limit)
        .map(|c| c.iter().collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schemas::{Recording, SourceType, Summary, Transcript};

    #[test]
    fn validate_accepts_hooks_url_rejects_others() {
        let ok = "https://hooks.slack.com/services/T000/B000/xxxxxxxx";
        assert_eq!(validate_webhook(ok).unwrap(), ok);
        assert!(validate_webhook("https://evil.example.com/x").is_err());
        assert!(validate_webhook("http://hooks.slack.com/services/a/b/c").is_err()); // http は不可
        assert!(validate_webhook("   ").is_err());
    }

    #[test]
    fn mrkdwn_converts_bold_heading_bullet() {
        let md = "# 見出し\n- 箇条 **太字**\n本文 __強調__";
        let out = md_to_mrkdwn(md);
        assert_eq!(out, "*見出し*\n• 箇条 *太字*\n本文 *強調*");
    }

    #[test]
    fn mrkdwn_strips_double_marked_heading_and_drops_rules() {
        // LLM の `## # 議題`（見出し二重マーク）→ `*議題*`、`---` 区切り線は落とす。
        let md = "# 議事録\n\n---\n\n## # 議題\n\n本文";
        let out = md_to_mrkdwn(md);
        assert_eq!(out, "*議事録*\n\n\n\n*議題*\n\n本文");
        assert!(is_thematic_break("---"));
        assert!(is_thematic_break("***"));
        assert!(is_thematic_break("___"));
        assert!(!is_thematic_break("--"));
        assert!(!is_thematic_break("**太字**"));
        // 見出し本文に正規の `#` が含まれても先頭以外は残す（C# 等）。
        assert_eq!(md_to_mrkdwn("## C# の話"), "*C# の話*");
        // 空白の無い `#tag` は見出しにしない。
        assert_eq!(md_to_mrkdwn("#tag です"), "#tag です");
    }

    #[test]
    fn mrkdwn_escapes_slack_control_chars() {
        // `<` `>` `&` をエンティティ化しないと Slack がリンク/メンション扱いして本文が消える。
        assert_eq!(
            md_to_mrkdwn("戻り値: Vec<String>"),
            "戻り値: Vec&lt;String&gt;"
        );
        assert_eq!(md_to_mrkdwn("期日: <未定>"), "期日: &lt;未定&gt;");
        assert_eq!(md_to_mrkdwn("Q&A は来週"), "Q&amp;A は来週");
        // `&` を先に置換するので `&lt;` が二重エスケープされない。
        assert_eq!(md_to_mrkdwn("a < b & c"), "a &lt; b &amp; c");
        // 太字変換とエスケープが両立する。
        assert_eq!(md_to_mrkdwn("**A** <x>"), "*A* &lt;x&gt;");
    }

    #[test]
    fn fallback_text_escapes_title_without_bold_conversion() {
        // トップレベルの text も mrkdwn なので、タイトルの `<!channel>` をメンションにしない。
        assert_eq!(
            fallback_text("定例 <!channel> & 共有"),
            "📝 定例 &lt;!channel&gt; &amp; 共有"
        );
        // 太字変換はしない（`__init__` を `*init*` にしない）。
        assert_eq!(fallback_text("__init__ の件"), "📝 __init__ の件");
    }

    #[test]
    fn chunk_chars_splits_over_limit() {
        let long = "あ".repeat(SECTION_TEXT_LIMIT + 5);
        assert_eq!(chunk_chars(&long, SECTION_TEXT_LIMIT).len(), 2);
        assert!(chunk_chars("", SECTION_TEXT_LIMIT).is_empty());
    }

    #[test]
    fn build_blocks_header_label_content() {
        let detail = detail_with(vec![Summary {
            template_id: "minutes".into(),
            content: "# 決定事項\n- A".into(),
            action_items: vec![],
            stale: false,
        }]);
        let blocks = build_blocks(&detail, "会議X", Lang::Ja);
        // header + *議事録* + 本文 = 3 ブロック
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0]["type"], "header");
        assert_eq!(blocks[0]["text"]["text"], "会議X");
        assert_eq!(blocks[1]["type"], "section");
        assert_eq!(blocks[1]["text"]["text"], "*議事録*");
        assert_eq!(blocks[2]["text"]["text"], "*決定事項*\n• A");
    }

    /// en: ラベルが英語になる（frontend templates.ts と一致する template_label 経由）。
    #[test]
    fn build_blocks_english_label() {
        let detail = detail_with(vec![Summary {
            template_id: "action_items".into(),
            content: "- do it".into(),
            action_items: vec![],
            stale: false,
        }]);
        let blocks = build_blocks(&detail, "Meeting X", Lang::En);
        assert_eq!(blocks[1]["text"]["text"], "*Action Items*");
    }

    #[test]
    fn export_errors_when_no_summaries() {
        // 有効そうな webhook + 要約ゼロ → ネットワークに触れず Err（空投稿防止）。
        let detail = detail_with(vec![]);
        let exp = SlackExporter {
            webhook_url: "https://hooks.slack.com/services/T/B/X".into(),
            lang: Lang::Ja,
        };
        assert!(exp.export(&detail).is_err());
    }

    fn detail_with(summaries: Vec<Summary>) -> RecordingDetail {
        RecordingDetail {
            recording: Recording {
                id: "r1".into(),
                source_type: SourceType::File,
                title: Some("t".into()),
                duration_ms: 1000,
                sample_rate: 16000,
                created_at: "2026-06-27T00:00:00Z".into(),
            },
            transcript: Transcript {
                language: None,
                segments: vec![],
            },
            summaries,
            speakers: vec![],
            active_job: None,
        }
    }
}
