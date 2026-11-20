//! The live (realtime) transcript of a meeting, saved next to the final transcript (ADR-0048).
use super::*;
use std::collections::HashSet;

pub(super) const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS live_captions (
  recording_id TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
  caption_id INTEGER NOT NULL CHECK (caption_id >= 0),
  text TEXT NOT NULL,
  committed INTEGER NOT NULL CHECK (committed IN (0, 1)),
  PRIMARY KEY (recording_id, caption_id)
);
"#;

/// Stored rows per recording. A line about every 3.5 s fills this after about 19 hours.
pub const MAX_LIVE_CAPTION_ROWS: usize = 20_000;
/// Total caption text per recording.
pub const MAX_LIVE_CAPTION_BYTES: usize = 16 * 1024 * 1024;
/// One caption. Whisper segments are far shorter; longer text is cut at a char boundary.
pub const MAX_LIVE_CAPTION_TEXT_BYTES: usize = 4096;

/// One live caption exactly as it was shown. `caption_id` is the `meeting://live` line ID,
/// which `live_translations.source_id` also uses. `committed=false` marks the unfinished
/// tail that was on screen at Stop. Not aligned to the final transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedLiveCaption {
    pub caption_id: i64,
    pub text: String,
    pub committed: bool,
}

pub fn validate_live_captions(rows: &[SavedLiveCaption]) -> Result<()> {
    let mut ids = HashSet::new();
    let mut bytes = 0;
    for row in rows {
        bytes += row.text.len();
        if row.caption_id < 0
            || row.text.trim().is_empty()
            || row.text.len() > MAX_LIVE_CAPTION_TEXT_BYTES
            || !ids.insert(row.caption_id)
        {
            return Err(CoreError::Db("Invalid live caption snapshot".into()));
        }
    }
    if rows.len() > MAX_LIVE_CAPTION_ROWS || bytes > MAX_LIVE_CAPTION_BYTES {
        return Err(CoreError::Db("Live transcript is too large".into()));
    }
    Ok(())
}

/// Collects the live transcript of one meeting while it runs, within the stored limits,
/// so that the rows handed over at Stop always pass `validate_live_captions`.
/// The 80-line display window of the live worker is separate and unaffected.
#[derive(Debug, Default)]
pub struct LiveCaptionArchive {
    committed: Vec<SavedLiveCaption>,
    committed_bytes: usize,
    tail: Vec<SavedLiveCaption>,
    full: bool,
}

impl LiveCaptionArchive {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a line that will no longer change. Blank lines are ignored; once the limits are
    /// reached, later lines are dropped (the display and the recording continue).
    pub fn commit(&mut self, caption_id: u64, text: &str) {
        let Some(text) = clean(text) else {
            return;
        };
        if self.full
            || self.committed.len() >= MAX_LIVE_CAPTION_ROWS
            || self.committed_bytes + text.len() > MAX_LIVE_CAPTION_BYTES
        {
            self.full = true;
            return;
        }
        self.committed_bytes += text.len();
        self.committed.push(SavedLiveCaption {
            caption_id: caption_id as i64,
            text,
            committed: true,
        });
    }

    /// Replace the unfinished tail with the lines now on screen (IDs from `first_id`).
    pub fn set_tail(&mut self, first_id: u64, texts: &[String]) {
        self.tail = texts
            .iter()
            .enumerate()
            .filter_map(|(index, text)| {
                clean(text).map(|text| SavedLiveCaption {
                    caption_id: (first_id + index as u64) as i64,
                    text,
                    committed: false,
                })
            })
            .collect();
    }

    /// True once a committed line was dropped because of the limits.
    pub fn is_full(&self) -> bool {
        self.full
    }

    /// Rows to save: every committed line, then as much of the last tail as still fits.
    pub fn into_rows(self) -> Vec<SavedLiveCaption> {
        let mut rows = self.committed;
        let mut bytes = self.committed_bytes;
        if !self.full {
            for row in self.tail {
                if rows.len() >= MAX_LIVE_CAPTION_ROWS
                    || bytes + row.text.len() > MAX_LIVE_CAPTION_BYTES
                {
                    break;
                }
                bytes += row.text.len();
                rows.push(row);
            }
        }
        rows
    }
}

fn clean(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut end = text.len().min(MAX_LIVE_CAPTION_TEXT_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Some(text[..end].to_owned())
}

pub(super) fn insert_rows(
    tx: &rusqlite::Transaction<'_>,
    recording_id: &str,
    rows: &[SavedLiveCaption],
) -> Result<()> {
    let mut statement = tx.prepare(
        "INSERT INTO live_captions (recording_id, caption_id, text, committed)
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    for row in rows {
        statement.execute(params![
            recording_id,
            row.caption_id,
            row.text,
            row.committed
        ])?;
    }
    Ok(())
}

impl SqliteStore {
    pub fn list_live_captions(&self, recording_id: &str) -> Result<Vec<SavedLiveCaption>> {
        let conn = self.conn();
        let mut statement = conn.prepare(
            "SELECT caption_id, text, committed FROM live_captions
             WHERE recording_id = ?1 ORDER BY caption_id",
        )?;
        let rows = statement.query_map([recording_id], |r| {
            Ok(SavedLiveCaption {
                caption_id: r.get(0)?,
                text: r.get(1)?,
                committed: r.get(2)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> Recording {
        Recording {
            id: "meeting".into(),
            source_type: SourceType::Live,
            title: Some("Meeting".into()),
            duration_ms: 1000,
            sample_rate: 16000,
            created_at: "2026-10-29T00:00:00Z".into(),
        }
    }

    fn caption(caption_id: i64, text: &str) -> SavedLiveCaption {
        SavedLiveCaption {
            caption_id,
            text: text.into(),
            committed: true,
        }
    }

    fn translation() -> SavedLiveTranslation {
        SavedLiveTranslation {
            source_id: 1,
            source_text: "Second line.".into(),
            target: "ja".into(),
            translation: "二行目。".into(),
        }
    }

    fn count(store: &SqliteStore, table: &str) -> i64 {
        store
            .conn()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn archive_keeps_every_committed_line_and_the_last_tail() {
        let mut archive = LiveCaptionArchive::new();
        for id in 0..200 {
            archive.commit(id, &format!("line {id}"));
        }
        archive.set_tail(200, &["old draft".into()]);
        archive.set_tail(200, &["  ".into(), "new draft".into()]);
        let rows = archive.into_rows();
        // The live worker shows only 80 lines; the archive keeps all of them.
        assert_eq!(rows.len(), 201);
        assert_eq!(rows[0], caption(0, "line 0"));
        assert_eq!(rows[199], caption(199, "line 199"));
        assert_eq!(
            rows[200],
            SavedLiveCaption {
                caption_id: 201,
                text: "new draft".into(),
                committed: false,
            }
        );
        validate_live_captions(&rows).unwrap();
    }

    #[test]
    fn archive_stays_within_the_stored_limits() {
        let mut archive = LiveCaptionArchive::new();
        let line = "あ".repeat(MAX_LIVE_CAPTION_TEXT_BYTES); // 3 bytes per char: cut at a boundary
        let mut id = 0;
        while !archive.is_full() {
            archive.commit(id, &line);
            id += 1;
        }
        archive.set_tail(id, &["tail".into()]);
        let rows = archive.into_rows();
        assert!(rows
            .iter()
            .all(|r| r.committed && r.text.len() <= MAX_LIVE_CAPTION_TEXT_BYTES));
        assert!(rows.iter().map(|r| r.text.len()).sum::<usize>() <= MAX_LIVE_CAPTION_BYTES);
        validate_live_captions(&rows).unwrap();

        let mut archive = LiveCaptionArchive::new();
        for id in 0..MAX_LIVE_CAPTION_ROWS as u64 + 5 {
            archive.commit(id, "x");
        }
        assert!(archive.is_full());
        assert_eq!(archive.into_rows().len(), MAX_LIVE_CAPTION_ROWS);
    }

    #[test]
    fn invalid_captions_are_rejected() {
        for rows in [
            vec![caption(-1, "a")],
            vec![caption(0, " ")],
            vec![caption(0, &"a".repeat(MAX_LIVE_CAPTION_TEXT_BYTES + 1))],
            vec![caption(0, "a"), caption(0, "b")],
        ] {
            assert!(validate_live_captions(&rows).is_err());
        }
    }

    #[test]
    fn captions_survive_transcript_replacement_and_deletion_cascades() {
        let store = SqliteStore::open_in_memory().unwrap();
        let rows = vec![caption(0, "First line."), caption(1, "Second line.")];
        store
            .insert_recording_with_live_history(&record(), &[translation()], &rows)
            .unwrap();
        store
            .replace_transcript(
                "meeting",
                &Transcript {
                    language: Some("en".into()),
                    segments: vec![],
                },
                &[],
            )
            .unwrap();
        assert_eq!(store.list_live_captions("meeting").unwrap(), rows);
        // The saved translation still points at its caption by ID.
        assert_eq!(
            store.list_live_translations("meeting").unwrap()[0].source_id,
            rows[1].caption_id
        );
        store.delete_recording("meeting").unwrap();
        assert!(store.list_live_captions("meeting").unwrap().is_empty());
    }

    #[test]
    fn invalid_captions_do_not_insert_the_recording() {
        let store = SqliteStore::open_in_memory().unwrap();
        assert!(store
            .insert_recording_with_live_history(&record(), &[], &[caption(0, "a"), caption(0, "b")])
            .is_err());
        assert_eq!(count(&store, "recordings"), 0);
    }

    #[test]
    fn sql_failure_rolls_back_the_whole_save() {
        let store = SqliteStore::open_in_memory().unwrap();
        store.conn().execute_batch("CREATE TRIGGER fail_caption BEFORE INSERT ON live_captions BEGIN SELECT RAISE(FAIL, 'simulated write failure'); END;").unwrap();
        assert!(store
            .insert_recording_with_live_history(&record(), &[translation()], &[caption(0, "a")])
            .is_err());
        for table in [
            "recordings",
            "rec_fts",
            "live_translations",
            "live_captions",
        ] {
            assert_eq!(count(&store, table), 0, "{table}");
        }
    }

    #[test]
    fn migration_from_v7_keeps_recordings_and_captions_survive_reopen() {
        let path = std::env::temp_dir().join(format!(
            "mojiroku-live-captions-{}.db",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        {
            let store = SqliteStore::open(&path).unwrap();
            store
                .insert_recording_with_translations(&record(), &[translation()])
                .unwrap();
            store
                .conn()
                .execute_batch("DROP TABLE live_captions; PRAGMA user_version=7;")
                .unwrap();
        }
        {
            let store = SqliteStore::open(&path).unwrap();
            assert!(store.list_live_captions("meeting").unwrap().is_empty());
            assert_eq!(
                store.list_live_translations("meeting").unwrap(),
                vec![translation()]
            );
            let mut next = record();
            next.id = "next".into();
            store
                .insert_recording_with_live_history(&next, &[], &[caption(3, "Saved.")])
                .unwrap();
        }
        {
            let store = SqliteStore::open(&path).unwrap();
            assert_eq!(
                store.list_live_captions("next").unwrap(),
                vec![caption(3, "Saved.")]
            );
        }
        let _ = std::fs::remove_file(path);
    }
}
