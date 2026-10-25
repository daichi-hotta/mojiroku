//! 要約プロバイダ。既定=同梱 llama.cpp（llama-cpp-2, GGUF, Metal）。
//! 任意で OpenAI / Anthropic（BYOK）。詳細は `docs/03_design/spec.md` §8。

pub mod byok;
pub mod title;

pub use byok::{AnthropicSummarizer, OpenAiSummarizer};
pub use title::{
    build_title_prompt, is_default_title, sanitize_title, should_auto_title, title_template, DEFAULT_TITLES,
};

use crate::error::Result;
use crate::lang::{default_speaker_label, Lang};
use crate::schemas::{Summary, SummaryTemplate, TemplateKind, Transcript};

/// 要約プロバイダの抽象。
pub trait SummarizeProvider {
    fn summarize(&self, transcript: &Transcript, template: &SummaryTemplate) -> Result<Summary>;
}

const MINUTES_INSTRUCTION_JA: &str = "以下の会議の文字起こしから、日本語で議事録を作成してください。次の構成で簡潔にまとめてください: # 議題 / # 決定事項 / # 議論の要点 / # ToDo（担当と期限がわかれば併記）。文字起こしに無い情報は創作しないでください。前置きや結びの挨拶（「承知しました」「以上です」等）は書かず、見出しと内容のみを出力してください。";
const SUMMARY_INSTRUCTION_JA: &str = "以下の会議の文字起こしを、日本語で3〜6個の箇条書きに要約してください。重要な結論を優先し、相槌や言い淀みは省いてください。文字起こしに無い情報は創作しないでください。前置きは書かず、箇条書きのみを出力してください。";
const ACTION_INSTRUCTION_JA: &str = "以下の会議の文字起こしから、アクションアイテム（やるべきこと）だけを日本語の箇条書きで抽出してください。可能なら「担当者: 内容（期限）」の形式。該当が無ければ「なし」とだけ書いてください。文字起こしに無い情報は創作しないでください。前置きは書かず、箇条書き（または「なし」）のみを出力してください。";

// 英語 instruction は Qwen2.5-7B Q4_K_M の実測スパイクで採用した文面（構造遵守・反復なし・
// 忠実性良好を確認済み。変更するなら要再検証）。
// ⚠️ スパイクで判明した罠: instruction に日付の扱い（"write dates exactly as spoken" 等）を
// 足すと Q4 量子化 Qwen は逆に月日をフル創作する。日付に言及しないこの文面が最良だった。
const MINUTES_INSTRUCTION_EN: &str = "From the meeting transcript below, write meeting minutes in English. Use exactly this structure and keep it concise: # Agenda / # Decisions / # Key Points / # Action Items (include the owner and due date when they are mentioned). Do not invent information that is not in the transcript. Do not add any preamble or closing remarks - output only the headings and their content.";
const SUMMARY_INSTRUCTION_EN: &str = "Summarize the meeting transcript below into 3-6 bullet points in English. Prioritize the important conclusions and leave out filler and back-channel remarks. Do not invent information that is not in the transcript. Output only the bullet points, with no preamble.";
const ACTION_INSTRUCTION_EN: &str = "From the meeting transcript below, extract only the action items (things somebody has to do) as bullet points in English. Use the format \"Owner: task (due date)\" when possible. If there are none, write only \"None\". Do not invent information that is not in the transcript. Output only the bullet points (or \"None\"), with no preamble.";

/// 組み込みの要約テンプレート（議事録 / 要約 / アクションアイテム）。
/// instruction と表示名はコンテンツ言語（`lang`）に追従する。
pub fn builtin_templates(lang: Lang) -> Vec<SummaryTemplate> {
    let (minutes, summary, actions) = match lang {
        Lang::Ja => (
            ("議事録", MINUTES_INSTRUCTION_JA),
            ("要約", SUMMARY_INSTRUCTION_JA),
            ("アクションアイテム", ACTION_INSTRUCTION_JA),
        ),
        Lang::En => (
            ("Minutes", MINUTES_INSTRUCTION_EN),
            ("Summary", SUMMARY_INSTRUCTION_EN),
            ("Action Items", ACTION_INSTRUCTION_EN),
        ),
    };
    vec![
        SummaryTemplate {
            id: "minutes".into(),
            name: minutes.0.into(),
            kind: TemplateKind::Minutes,
            prompt: minutes.1.into(),
        },
        SummaryTemplate {
            id: "summary".into(),
            name: summary.0.into(),
            kind: TemplateKind::Summary,
            prompt: summary.1.into(),
        },
        SummaryTemplate {
            id: "action_items".into(),
            name: actions.0.into(),
            kind: TemplateKind::ActionItems,
            prompt: actions.1.into(),
        },
    ]
}

/// テンプレート id から取得（無ければ議事録）。
pub fn template_by_id(id: &str, lang: Lang) -> SummaryTemplate {
    builtin_templates(lang)
        .into_iter()
        .find(|t| t.id == id)
        .unwrap_or_else(|| builtin_templates(lang).remove(0))
}

/// speaker_id（"S1"）→ 既定ラベル（ja "話者1" / en "Speaker 1"）。要約プロンプトに生の "S1" を
/// 渡すと LLM 出力に不可読な記号が漏れるため、人間が読めるラベルへ変換する。
/// フロントの speakerLabelFromId と表記を揃える。解析できなければそのまま返す。
fn speaker_label(spk: &str, lang: Lang) -> String {
    spk.strip_prefix('S')
        .filter(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
        .map(|n| default_speaker_label(n, lang))
        .unwrap_or_else(|| spk.to_string())
}

/// 文字起こし → プロンプト本文（話者ラベルがあれば付与。Phase 2 で活きる）。
pub fn transcript_to_text(transcript: &Transcript, lang: Lang) -> String {
    let mut s = String::new();
    for seg in &transcript.segments {
        match &seg.speaker_id {
            Some(spk) => s.push_str(&format!("{}: {}\n", speaker_label(spk, lang), seg.text)),
            None => {
                s.push_str(&seg.text);
                s.push('\n');
            }
        }
    }
    s
}

/// instruction + 文字起こし本文 → ユーザープロンプト。
/// 区切りマーカーもコンテンツ言語に揃える（en はスパイクで検証済みの形式）。
pub fn build_prompt(transcript: &Transcript, template: &SummaryTemplate, lang: Lang) -> String {
    let (open, close) = match lang {
        Lang::Ja => ("--- 文字起こし ---", "--- ここまで ---"),
        Lang::En => ("--- Transcript ---", "--- End ---"),
    };
    format!(
        "{}\n\n{}\n{}{}",
        template.prompt,
        open,
        transcript_to_text(transcript, lang),
        close
    )
}

/// Remove the blank-line padding some local models add before it is saved (ADR-0044).
///
/// The detail view renders summaries with `white-space: pre-wrap`, so every extra blank line
/// shows up as a gap. Qwen3.5-4B writing English put two to three blank lines between
/// sections and a blank line between list items. Headings keep one blank line before them;
/// consecutive list items are joined. Text inside lines is not touched.
pub fn tidy_local_output(text: &str) -> String {
    let is_item = |l: &str| {
        let t = l.trim_start();
        t.starts_with("- ")
            || t.starts_with("* ")
            || t.split_once(". ")
                .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
    };
    let lines: Vec<&str> = text.trim().lines().map(str::trim_end).collect();
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        if line.is_empty() {
            let prev = out.last().copied().unwrap_or("");
            let next = lines[i + 1..].iter().find(|l| !l.is_empty()).copied();
            let between_items = is_item(prev) && next.is_some_and(is_item);
            if prev.is_empty() || between_items {
                continue;
            }
        }
        out.push(line);
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schemas::Segment;

    #[test]
    fn tidy_local_output_collapses_padding_but_keeps_sections() {
        let raw = "# Agenda\n\nReview.\n\n\n\n# Decisions\n\n- One.\n\n- Two.\n\n1. A\n\n2. B\n\n\n# ToDo\n- Three.  \n";
        assert_eq!(
            tidy_local_output(raw),
            "# Agenda\n\nReview.\n\n# Decisions\n\n- One.\n- Two.\n1. A\n2. B\n\n# ToDo\n- Three."
        );
    }

    #[test]
    fn tidy_local_output_leaves_compact_text_unchanged() {
        let ja = "# 議題\n- 進捗の確認\n\n# 決定事項\nなし\n\n# ToDo\n- 資料を送る（担当：自分）";
        assert_eq!(tidy_local_output(ja), ja);
    }

    fn seg(text: &str, speaker: Option<&str>) -> Segment {
        Segment {
            idx: 0,
            start_ms: 0,
            end_ms: 1000,
            text: text.into(),
            speaker_id: speaker.map(|s| s.to_string()),
        }
    }

    #[test]
    fn speaker_id_is_rendered_as_japanese_label() {
        assert_eq!(speaker_label("S1", Lang::Ja), "話者1");
        assert_eq!(speaker_label("S12", Lang::Ja), "話者12");
        // 解析できない id はそのまま（生の記号を勝手に書き換えない）。
        assert_eq!(speaker_label("guest", Lang::Ja), "guest");
        assert_eq!(speaker_label("S", Lang::Ja), "S");
    }

    /// en はフロント speakerLabelFromId（"Speaker N"）と表記一致。
    #[test]
    fn speaker_id_is_rendered_as_english_label() {
        assert_eq!(speaker_label("S1", Lang::En), "Speaker 1");
        assert_eq!(speaker_label("S12", Lang::En), "Speaker 12");
        assert_eq!(speaker_label("guest", Lang::En), "guest");
    }

    #[test]
    fn transcript_with_speakers_uses_readable_labels_no_raw_s1() {
        let t = Transcript {
            language: Some("ja".into()),
            segments: vec![seg("おはよう", Some("S1")), seg("はい", Some("S2"))],
        };
        let text = transcript_to_text(&t, Lang::Ja);
        assert_eq!(text, "話者1: おはよう\n話者2: はい\n");
        assert!(!text.contains("[S1]"), "生の S1 がプロンプトに漏れている");
    }

    #[test]
    fn transcript_with_speakers_english_labels() {
        let t = Transcript {
            language: Some("en".into()),
            segments: vec![seg("hello", Some("S1")), seg("hi", Some("S2"))],
        };
        assert_eq!(
            transcript_to_text(&t, Lang::En),
            "Speaker 1: hello\nSpeaker 2: hi\n"
        );
    }

    #[test]
    fn transcript_without_speakers_is_plain_lines() {
        let t = Transcript {
            language: None,
            segments: vec![seg("一行目", None), seg("二行目", None)],
        };
        assert_eq!(transcript_to_text(&t, Lang::Ja), "一行目\n二行目\n");
    }

    /// テンプレート解決と build_prompt の区切りマーカーが lang に追従する。
    #[test]
    fn templates_and_prompt_follow_lang() {
        // ja（従来挙動の保存）: 名前・instruction・マーカーとも日本語。
        let t_ja = template_by_id("minutes", Lang::Ja);
        assert_eq!(t_ja.name, "議事録");
        assert_eq!(t_ja.prompt, MINUTES_INSTRUCTION_JA);
        let p_ja = build_prompt(
            &Transcript { language: None, segments: vec![seg("本文", None)] },
            &t_ja,
            Lang::Ja,
        );
        assert!(p_ja.contains("--- 文字起こし ---") && p_ja.ends_with("--- ここまで ---"));

        // en: スパイク採用の instruction + 英語マーカー。未知 id は議事録へフォールバック。
        let t_en = template_by_id("minutes", Lang::En);
        assert_eq!(t_en.name, "Minutes");
        assert_eq!(t_en.prompt, MINUTES_INSTRUCTION_EN);
        assert_eq!(template_by_id("unknown", Lang::En).id, "minutes");
        let p_en = build_prompt(
            &Transcript { language: None, segments: vec![seg("body", None)] },
            &t_en,
            Lang::En,
        );
        assert!(p_en.contains("--- Transcript ---") && p_en.ends_with("--- End ---"));
    }
}
