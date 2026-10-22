use super::*;
use crate::correction::SpeakerCorrection;

// v8: 発言単位の話者訂正（Issue #19・ADR-0048）。1 発言 1 行で、訂正後の状態だけを持つ。
// `predicted` は話者分離が付けた話者で、何度直しても最初の値を残す。直した先が `predicted`
// に戻ったら行を消す（訂正の取り消し）。話者分離をやり直すと新しい id で書き直す。
pub(super) const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS speaker_corrections (
  recording_id TEXT    NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
  idx          INTEGER NOT NULL,
  predicted    TEXT,
  corrected    TEXT,
  corrected_at TEXT    NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY (recording_id, idx)
);
"#;

impl SqliteStore {
    /// 録音の手動訂正を `idx` 順に読む。
    pub fn speaker_corrections(&self, recording_id: &str) -> Result<Vec<SpeakerCorrection>> {
        let conn = self.conn();
        read_rows(&conn, recording_id)
    }
}

pub(super) fn read_rows(conn: &Connection, recording_id: &str) -> Result<Vec<SpeakerCorrection>> {
    let mut stmt = conn.prepare(
        "SELECT idx, predicted, corrected FROM speaker_corrections
         WHERE recording_id = ?1 ORDER BY idx ASC",
    )?;
    let rows = stmt
        .query_map(params![recording_id], |r| {
            Ok(SpeakerCorrection {
                idx: r.get(0)?,
                predicted: r.get(1)?,
                corrected: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// 発言 1 件を `current` から `chosen` へ直したことを記録する（`set_segment_speaker` から呼ぶ）。
pub(super) fn record(
    conn: &Connection,
    recording_id: &str,
    idx: u32,
    current: Option<&str>,
    chosen: Option<&str>,
) -> Result<()> {
    let existing: Option<Option<String>> = conn
        .query_row(
            "SELECT predicted FROM speaker_corrections WHERE recording_id = ?1 AND idx = ?2",
            params![recording_id, idx],
            |r| r.get(0),
        )
        .optional()?;
    // 2 回目以降の訂正でも、話者分離が付けた最初の値を残す。
    let predicted = match existing {
        Some(p) => p,
        None => current.map(str::to_string),
    };
    if predicted.as_deref() == chosen {
        conn.execute(
            "DELETE FROM speaker_corrections WHERE recording_id = ?1 AND idx = ?2",
            params![recording_id, idx],
        )?;
    } else {
        conn.execute(
            "INSERT INTO speaker_corrections (recording_id, idx, predicted, corrected)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (recording_id, idx)
             DO UPDATE SET corrected = excluded.corrected, corrected_at = datetime('now')",
            params![recording_id, idx, predicted, chosen],
        )?;
    }
    Ok(())
}

/// やり直しの結果が利用者の選択と一致して残った行（`predicted` = `corrected` = 今の話者）を消す。
/// 今の話者を選び直したとき（`set_segment_speaker` の同値の経路）に呼ぶ。発言は変わらないので、
/// ふつうの訂正の行（`predicted` が別の話者）は残す。
pub(super) fn forget_confirmed(
    conn: &Connection,
    recording_id: &str,
    idx: u32,
    current: Option<&str>,
) -> Result<()> {
    // `IS` で比べる。話者不明に戻した訂正が、やり直しでも未割当の発言に引き継がれると
    // 両方 NULL の行になる。
    conn.execute(
        "DELETE FROM speaker_corrections
         WHERE recording_id = ?1 AND idx = ?2 AND predicted IS ?3 AND corrected IS ?3",
        params![recording_id, idx, current],
    )?;
    Ok(())
}

/// 話者分離のやり直しで書き直した訂正に差し替える。残る行は直した時刻を保ち、
/// 引き継げなかった行は消す。
pub(super) fn replace_after_rediarize(
    conn: &Connection,
    recording_id: &str,
    carried: &[SpeakerCorrection],
) -> Result<()> {
    let keep: Vec<u32> = carried.iter().map(|c| c.idx).collect();
    for old in read_rows(conn, recording_id)? {
        if !keep.contains(&old.idx) {
            conn.execute(
                "DELETE FROM speaker_corrections WHERE recording_id = ?1 AND idx = ?2",
                params![recording_id, old.idx],
            )?;
        }
    }
    for c in carried {
        conn.execute(
            "INSERT INTO speaker_corrections (recording_id, idx, predicted, corrected)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (recording_id, idx)
             DO UPDATE SET predicted = excluded.predicted, corrected = excluded.corrected",
            params![recording_id, c.idx, c.predicted, c.corrected],
        )?;
    }
    Ok(())
}

/// 本文を入れ直すと `idx` が別の発言を指しうるので、訂正をすべて消す。
pub(super) fn clear(conn: &Connection, recording_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM speaker_corrections WHERE recording_id = ?1",
        params![recording_id],
    )?;
    Ok(())
}
