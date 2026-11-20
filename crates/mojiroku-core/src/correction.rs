//! 発言単位の話者訂正（Issue #19）と、話者分離をやり直したときの引き継ぎ（ADR-0048）。
//!
//! 訂正は「この発言はこの人」という、分離結果そのものへの否定である。改名
//! （`carry_display_names`）よりも強い主張なので、やり直しで黙って捨てない。

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::merge::SELF_SPEAKER_ID;
use crate::schemas::Transcript;

/// 利用者が手で話者を選び直した発言 1 件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeakerCorrection {
    /// 対象の発言（`Segment.idx`）。
    pub idx: u32,
    /// 話者分離が付けた話者（利用者が変える前）。`None` は未割当。
    pub predicted: Option<String>,
    /// 利用者が選んだ話者。`None` は「話者不明」に戻した。
    pub corrected: Option<String>,
}

/// 話者分離をやり直したあとの訂正。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CarriedCorrections {
    /// 新しい話者 id で書き直した訂正。`predicted` はやり直しの結果。
    pub corrections: Vec<SpeakerCorrection>,
    /// 選んだ話者が新しい結果の誰に当たるか決められず、未割当に戻した発言の数。
    pub unmapped: usize,
}

/// 話者分離のやり直しで、手で直した発言を新しい話者 id へ引き継ぐ。
///
/// - `previous`: やり直す前の各発言の話者（訂正を含む保存済みの状態）。`transcript` と同じ並び。
/// - `transcript`: やり直しの割り当てを済ませた本文。本文と `idx` は変わらない。ここで上書きする。
/// - `voice_matches`: 声紋で対応づけた（旧 id, 新 id）。多数決で決まらないときに使う。
///
/// 選んだ話者 X の新しい id は、X の**直していない**発言がやり直しで付いた話者の多数決で決める。
/// 会議の自分（`self`）は再分離されないのでそのまま。どちらでも決まらない発言と、選んだ話者が
/// 元の予測（利用者が否定した話者）と同じ新しい話者にまとまった発言は未割当に戻し、件数を返す
/// （黙って別の話者に付けたままにしない）。
pub fn carry_corrections(
    corrections: &[SpeakerCorrection],
    previous: &[Option<String>],
    transcript: &mut Transcript,
    voice_matches: &[(String, String)],
) -> CarriedCorrections {
    let corrected_idx: HashSet<u32> = corrections.iter().map(|c| c.idx).collect();

    // 旧話者 X ごとに、直していない発言が新しく付いた話者を数える。
    let mut votes: HashMap<&str, HashMap<&str, usize>> = HashMap::new();
    for (seg, prev) in transcript.segments.iter().zip(previous) {
        if corrected_idx.contains(&seg.idx) {
            continue;
        }
        if let (Some(old), Some(new)) = (prev.as_deref(), seg.speaker_id.as_deref()) {
            *votes.entry(old).or_default().entry(new).or_default() += 1;
        }
    }
    let new_id_for = |old: &str| -> Option<String> {
        if old == SELF_SPEAKER_ID {
            return Some(SELF_SPEAKER_ID.to_string());
        }
        let by_vote = votes.get(old).and_then(|counts| {
            counts
                .iter()
                // 票が多い方、同数なら id の小さい方（実行ごとに結果を変えない）。
                .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
                .map(|(new, _)| new.to_string())
        });
        by_vote.or_else(|| {
            voice_matches
                .iter()
                .find(|(o, _)| o == old)
                .map(|(_, n)| n.clone())
        })
    };
    // 本文を書き換える前に、選ばれた話者と元の予測の行き先を決めておく。
    let new_ids: HashMap<String, Option<String>> = corrections
        .iter()
        .flat_map(|c| [c.corrected.as_deref(), c.predicted.as_deref()])
        .flatten()
        .map(|old| (old.to_string(), new_id_for(old)))
        .collect();
    let new_id = |old: Option<&str>| old.and_then(|o| new_ids.get(o).cloned().flatten());

    let position: HashMap<u32, usize> = transcript
        .segments
        .iter()
        .enumerate()
        .map(|(i, s)| (s.idx, i))
        .collect();
    let mut out = CarriedCorrections::default();
    for c in corrections {
        let Some(&i) = position.get(&c.idx) else {
            continue;
        };
        let target = match c.corrected.as_deref() {
            None => Some(None),
            // やり直しで、選んだ話者と利用者が否定した話者が同じ人にまとまったら、
            // 訂正の意味が失われる。引き継げないものとして扱う。前のやり直しが選択と
            // 一致した行（predicted = corrected）には否定した話者が無いので当てはまらない。
            Some(_)
                if c.predicted != c.corrected
                    && new_id(c.predicted.as_deref()).is_some()
                    && new_id(c.predicted.as_deref()) == new_id(c.corrected.as_deref()) =>
            {
                None
            }
            Some(old) => new_id(Some(old)).map(Some),
        };
        let seg = &mut transcript.segments[i];
        match target {
            Some(corrected) => {
                out.corrections.push(SpeakerCorrection {
                    idx: c.idx,
                    predicted: seg.speaker_id.clone(),
                    corrected: corrected.clone(),
                });
                seg.speaker_id = corrected;
            }
            None => {
                seg.speaker_id = None;
                out.unmapped += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schemas::Segment;

    fn transcript(ids: &[Option<&str>]) -> Transcript {
        Transcript {
            language: Some("ja".into()),
            segments: ids
                .iter()
                .enumerate()
                .map(|(i, id)| Segment {
                    idx: i as u32,
                    start_ms: i as u64 * 1000,
                    end_ms: i as u64 * 1000 + 900,
                    text: format!("t{i}"),
                    speaker_id: id.map(str::to_string),
                })
                .collect(),
        }
    }

    fn ids(t: &Transcript) -> Vec<Option<&str>> {
        t.segments.iter().map(|s| s.speaker_id.as_deref()).collect()
    }

    fn fix(idx: u32, predicted: Option<&str>, corrected: Option<&str>) -> SpeakerCorrection {
        SpeakerCorrection {
            idx,
            predicted: predicted.map(str::to_string),
            corrected: corrected.map(str::to_string),
        }
    }

    fn prev(ids: &[Option<&str>]) -> Vec<Option<String>> {
        ids.iter().map(|s| s.map(str::to_string)).collect()
    }

    #[test]
    fn maps_the_chosen_speaker_by_where_their_other_utterances_went() {
        // 旧: S1 S1 S2 S2、発言 3 を S2 → S1 に直した。やり直しで旧 S1 は S2、旧 S2 は S1 になった。
        let previous = prev(&[Some("S1"), Some("S1"), Some("S2"), Some("S1")]);
        let mut t = transcript(&[Some("S2"), Some("S2"), Some("S1"), Some("S1")]);
        let out = carry_corrections(&[fix(3, Some("S2"), Some("S1"))], &previous, &mut t, &[]);

        assert_eq!(
            ids(&t),
            vec![Some("S2"), Some("S2"), Some("S1"), Some("S2")]
        );
        assert_eq!(out.corrections, vec![fix(3, Some("S1"), Some("S2"))]);
        assert_eq!(out.unmapped, 0);
    }

    #[test]
    fn corrected_utterances_do_not_vote() {
        // 旧 S3 の発言は全部、手で S3 にしたもの。多数決の材料が無いので声紋の対応に頼る。
        let previous = prev(&[Some("S1"), Some("S3"), Some("S3")]);
        let mut t = transcript(&[Some("S1"), Some("S1"), Some("S1")]);
        let corrections = [
            fix(1, Some("S1"), Some("S3")),
            fix(2, Some("S1"), Some("S3")),
        ];
        let voice = [("S3".to_string(), "S2".to_string())];
        let out = carry_corrections(&corrections, &previous, &mut t, &voice);

        assert_eq!(ids(&t), vec![Some("S1"), Some("S2"), Some("S2")]);
        assert_eq!(out.unmapped, 0);
    }

    #[test]
    fn unmappable_correction_becomes_unassigned_and_is_counted() {
        let previous = prev(&[Some("S1"), Some("S3")]);
        let mut t = transcript(&[Some("S1"), Some("S1")]);
        let out = carry_corrections(&[fix(1, Some("S1"), Some("S3"))], &previous, &mut t, &[]);

        assert_eq!(ids(&t), vec![Some("S1"), None]);
        assert!(out.corrections.is_empty());
        assert_eq!(out.unmapped, 1);
    }

    #[test]
    fn correction_merged_back_into_the_rejected_speaker_is_unmapped() {
        // 発言 2 を S1 → S3 に直した。やり直しで旧 S1 と旧 S3 が新 S1 にまとまった。
        let previous = prev(&[Some("S1"), Some("S3"), Some("S3"), Some("S1")]);
        let mut t = transcript(&[Some("S1"), Some("S1"), Some("S1"), Some("S2")]);
        let out = carry_corrections(&[fix(2, Some("S1"), Some("S3"))], &previous, &mut t, &[]);

        assert_eq!(t.segments[2].speaker_id, None);
        assert!(out.corrections.is_empty());
        assert_eq!(out.unmapped, 1);
    }

    #[test]
    fn line_confirmed_by_a_previous_re_run_is_carried_again() {
        // 前のやり直しが利用者の選択と一致した行（predicted = corrected）。否定した話者が
        // 無いので、まとまりの規則で未割当にしない。
        let previous = prev(&[Some("S1"), Some("S2"), Some("S2")]);
        let mut t = transcript(&[Some("S1"), Some("S2"), Some("S2")]);
        let out = carry_corrections(&[fix(2, Some("S2"), Some("S2"))], &previous, &mut t, &[]);

        assert_eq!(ids(&t), vec![Some("S1"), Some("S2"), Some("S2")]);
        assert_eq!(out.corrections, vec![fix(2, Some("S2"), Some("S2"))]);
        assert_eq!(out.unmapped, 0);
    }

    #[test]
    fn line_confirmed_by_hand_keeps_the_choice_when_the_re_run_moves_it() {
        // やり直しで旧 S1 は新 S2、旧 S2 は新 S1 になり、手で確かめた発言 2 は新 S2 に付いた。
        let previous = prev(&[Some("S1"), Some("S2"), Some("S2")]);
        let mut t = transcript(&[Some("S2"), Some("S1"), Some("S2")]);
        let out = carry_corrections(&[fix(2, Some("S2"), Some("S2"))], &previous, &mut t, &[]);

        assert_eq!(ids(&t), vec![Some("S2"), Some("S1"), Some("S1")]);
        assert_eq!(out.corrections, vec![fix(2, Some("S2"), Some("S1"))]);
        assert_eq!(out.unmapped, 0);
    }

    #[test]
    fn cleared_speaker_stays_cleared() {
        let previous = prev(&[Some("S1"), None]);
        let mut t = transcript(&[Some("S1"), Some("S1")]);
        let out = carry_corrections(&[fix(1, Some("S1"), None)], &previous, &mut t, &[]);

        assert_eq!(ids(&t), vec![Some("S1"), None]);
        assert_eq!(out.corrections, vec![fix(1, Some("S1"), None)]);
    }

    #[test]
    fn self_in_a_meeting_is_kept() {
        // 自分の発言を相手に、相手の発言を自分に直した会議。
        let previous = prev(&[Some("self"), Some("S1"), Some("self"), Some("S1")]);
        let mut t = transcript(&[Some("self"), Some("S2"), Some("self"), Some("S2")]);
        let corrections = [
            fix(1, Some("self"), Some("S1")),
            fix(2, Some("S1"), Some("self")),
        ];
        let out = carry_corrections(&corrections, &previous, &mut t, &[]);

        assert_eq!(
            ids(&t),
            vec![Some("self"), Some("S2"), Some("self"), Some("S2")]
        );
        assert_eq!(out.unmapped, 0);
    }

    #[test]
    fn ties_are_broken_by_the_smaller_id() {
        let previous = prev(&[Some("S1"), Some("S1"), Some("S2")]);
        let mut t = transcript(&[Some("S3"), Some("S2"), Some("S1")]);
        carry_corrections(&[fix(2, Some("S2"), Some("S1"))], &previous, &mut t, &[]);

        assert_eq!(t.segments[2].speaker_id.as_deref(), Some("S2"));
    }
}
