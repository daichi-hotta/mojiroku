//! Recording の CRUD（保存 / 一覧 / 詳細 / 改名 / 削除）。mod.rs から分割。
use super::*;

/// transcript の segments を idx 昇順で INSERT する（save_recording / replace_* で共通）。
/// 呼び出し側が事前に既存 segments を DELETE 済みであること（差し替え時）。
fn insert_segments(
    tx: &rusqlite::Transaction<'_>,
    recording_id: &str,
    transcript: &Transcript,
) -> Result<()> {
    let mut stmt = tx.prepare(
        "INSERT INTO segments (recording_id, idx, start_ms, end_ms, text, speaker_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for (i, seg) in transcript.segments.iter().enumerate() {
        stmt.execute(params![
            recording_id,
            i as i64,
            seg.start_ms as i64,
            seg.end_ms as i64,
            seg.text,
            seg.speaker_id,
        ])?;
    }
    Ok(())
}

/// speakers 行を丸ごと差し替える（既存を DELETE → INSERT）。空なら削除のみ。
fn replace_speakers_rows(
    tx: &rusqlite::Transaction<'_>,
    recording_id: &str,
    speakers: &[Speaker],
) -> Result<()> {
    tx.execute(
        "DELETE FROM speakers WHERE recording_id = ?1",
        params![recording_id],
    )?;
    let mut stmt = tx.prepare(
        "INSERT INTO speakers (recording_id, speaker_id, label, display_name)
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    for sp in speakers {
        stmt.execute(params![recording_id, sp.id, sp.label, sp.display_name])?;
    }
    Ok(())
}

impl SqliteStore {
    /// transcribe 完了時に Recording + Transcript（+ 話者）を 1 トランザクションで保存。
    /// `speakers` は話者分離を行った場合のみ非空。各 `speaker.id` は segment.speaker_id と一致する。
    pub fn save_recording(
        &self,
        rec: &Recording,
        transcript: &Transcript,
        speakers: &[Speaker],
    ) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO recordings
               (id, source_type, title, duration_ms, sample_rate, language, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                rec.id,
                source_type_str(rec.source_type),
                rec.title,
                rec.duration_ms as i64,
                rec.sample_rate as i64,
                transcript.language,
                rec.created_at,
            ],
        )?;
        insert_segments(&tx, &rec.id, transcript)?;
        replace_speakers_rows(&tx, &rec.id, speakers)?;
        // FTS 同期: 1 録音 = 1 行（title + segment.text を idx 昇順で改行連結）。
        let body = transcript
            .segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        tx.execute(
            "INSERT INTO rec_fts (title, body, recording_id) VALUES (?1, ?2, ?3)",
            params![rec.title.as_deref().unwrap_or(""), body, rec.id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 録音行だけを先に作る（文字起こし前・ADR-0024）。segments/speakers は無し、language は NULL。
    /// rec_fts は空 body の 1 行を作っておく（タイトル検索は即可、本文は `replace_transcript` が UPDATE で埋める）。
    /// 停止/取込コマンドが呼び、STT はジョブキュー経由で後から `replace_transcript` する。
    pub fn insert_recording_only(&self, rec: &Recording) -> Result<()> {
        self.insert_recording_with_translations(rec, &[])
    }

    /// Save the recording and its completed live translations atomically before enqueueing STT.
    pub fn insert_recording_with_translations(
        &self,
        rec: &Recording,
        translations: &[SavedLiveTranslation],
    ) -> Result<()> {
        validate_live_translations(translations)?;
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO recordings
               (id, source_type, title, duration_ms, sample_rate, language, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6)",
            params![
                rec.id,
                source_type_str(rec.source_type),
                rec.title,
                rec.duration_ms as i64,
                rec.sample_rate as i64,
                rec.created_at,
            ],
        )?;
        tx.execute(
            "INSERT INTO rec_fts (title, body, recording_id) VALUES (?1, '', ?2)",
            params![rec.title.as_deref().unwrap_or(""), rec.id],
        )?;
        translation::insert_rows(&tx, &rec.id, translations)?;
        tx.commit()?;
        Ok(())
    }

    /// 文字起こしジョブ完了時に transcript（+話者）を差し替える（再処理・冪等）。
    /// 既存 segments/speakers を消して入れ直し、rec_fts 本文・language を更新する。
    /// duration_ms は**未確定（0）のときだけ**最終 segment 末尾で埋める（mic/会議は停止時に正確値が
    /// 入っているので上書きしない。file は 0 で取り込むのでここで確定する）。
    pub fn replace_transcript(
        &self,
        recording_id: &str,
        transcript: &Transcript,
        speakers: &[Speaker],
    ) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM segments WHERE recording_id = ?1",
            params![recording_id],
        )?;
        insert_segments(&tx, recording_id, transcript)?;
        replace_speakers_rows(&tx, recording_id, speakers)?;
        let duration_ms = transcript.segments.last().map(|s| s.end_ms).unwrap_or(0) as i64;
        tx.execute(
            "UPDATE recordings
               SET language = ?2,
                   duration_ms = CASE WHEN duration_ms = 0 THEN ?3 ELSE duration_ms END
             WHERE id = ?1",
            params![recording_id, transcript.language, duration_ms],
        )?;
        let body = transcript
            .segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        tx.execute(
            "UPDATE rec_fts SET body = ?2 WHERE recording_id = ?1",
            params![recording_id, body],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 発言 1 件の話者を差し替える（発言単位の手動訂正・Issue #19）。
    ///
    /// 対象は `(recording_id, idx)` で指す。`idx` は `get_recording_detail` が返す値
    /// （`insert_segments` が `enumerate()` で採番した連番）。
    ///
    /// `speaker_id` に `Some` を渡す場合、**その話者が当該録音の `speakers` に実在すること**を
    /// 検証する。存在しない id を許すと、改名 UI に出ない話者が発言側にだけ生まれる
    /// （`speakers` の id 集合と `segments.speaker_id` の集合がズレる）。
    /// `None` は「話者不明に戻す」。
    ///
    /// **移動元の話者行は消さない。** 最後の 1 発言を移して発言ゼロになっても
    /// `speakers` 行・声紋・ライブラリ紐づけは残す（訂正を戻せるようにするため）。
    ///
    /// 既存要約は stale にする（要約本文に話者名が出るため）。
    /// 本文は変わらないので `rec_fts` は触らない。
    ///
    /// **戻り値は「実際に変えたか」。** 同じ話者を選び直したときは `false` を返して何もしない。
    /// 呼び出し側（UI）はこれを見て「要約が古い」の表示を出し分ける — 真偽を知っているのは
    /// ここだけなので、UI 側で現在値と比較させない（UI の値が DB と一致している前提に依存してしまう）。
    pub fn set_segment_speaker(
        &self,
        recording_id: &str,
        idx: u32,
        speaker_id: Option<&str>,
    ) -> Result<bool> {
        let mut conn = self.conn();
        // rusqlite の Transaction は DropBehavior 既定が Rollback。以降の早期 return では
        // tx が drop された時点で自動的にロールバックされる（明示の rollback は不要）。
        let tx = conn.transaction()?;

        if let Some(sid) = speaker_id {
            let known: i64 = tx.query_row(
                "SELECT COUNT(*) FROM speakers WHERE recording_id = ?1 AND speaker_id = ?2",
                params![recording_id, sid],
                |r| r.get(0),
            )?;
            if known == 0 {
                // キーは `error.` 始まりにする。コマンド層の core_err が Display 接頭辞を外す。
                return Err(crate::error::CoreError::Db(
                    "error.speaker.unknown_for_recording".to_string(),
                ));
            }
        }

        // 対象が存在するかを先に見る（存在しない idx を黙って 0 行更新にすると、
        // 訂正が失われたことに気づけない）。
        let current: Option<String> = tx
            .query_row(
                "SELECT speaker_id FROM segments WHERE recording_id = ?1 AND idx = ?2",
                params![recording_id, idx],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| crate::error::CoreError::Db("error.segment.not_found".to_string()))?;

        // 同じ話者を選び直しただけなら何もしない。要約を stale にすると 7B モデルでの
        // 作り直しが分単位で走るため、内容が変わっていないのに促すのは害。
        if current.as_deref() == speaker_id {
            return Ok(false);
        }

        tx.execute(
            "UPDATE segments SET speaker_id = ?3 WHERE recording_id = ?1 AND idx = ?2",
            params![recording_id, idx, speaker_id],
        )?;

        tx.execute(
            "UPDATE summaries SET stale = 1 WHERE recording_id = ?1",
            params![recording_id],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// 後付け（再）話者分離の結果で話者割当を差し替える（ベスト努力引き継ぎ・ADR-0024）。
    /// `transcript` は既存本文に新 diarization を `merge::assign_speakers` した後のもの（text 不変・
    /// speaker_id のみ変化）。`remap` は新 speaker_id → 引き継ぐ display_name（声紋 cosine で新旧一致した改名）。
    /// speaker_matches（ライブラリ照合）は再計算対象なので消し、既存要約は stale マークする。
    #[allow(clippy::too_many_arguments)]
    pub fn replace_speaker_assignments(
        &self,
        recording_id: &str,
        transcript: &Transcript,
        new_speakers: &[Speaker],
        embeddings: &[crate::diarization::SpeakerEmbedding],
        model: &str,
        remap: &[(String, Option<String>)],
    ) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        // 1) segments の speaker_id を更新（text は不変なので rec_fts は触らない）。
        tx.execute(
            "DELETE FROM segments WHERE recording_id = ?1",
            params![recording_id],
        )?;
        insert_segments(&tx, recording_id, transcript)?;
        // 2) speakers を差し替え、remap の display_name を反映（引き継ぎ）。
        let remap_name = |id: &str| -> Option<String> {
            remap
                .iter()
                .find(|(sid, _)| sid == id)
                .and_then(|(_, name)| name.clone())
        };
        tx.execute(
            "DELETE FROM speakers WHERE recording_id = ?1",
            params![recording_id],
        )?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO speakers (recording_id, speaker_id, label, display_name)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for sp in new_speakers {
                let display = sp.display_name.clone().or_else(|| remap_name(&sp.id));
                stmt.execute(params![recording_id, sp.id, sp.label, display])?;
            }
        }
        // 3) 声紋を丸ごと差し替え（旧話者 id の残骸を残さないため一旦全削除）。
        tx.execute(
            "DELETE FROM speaker_embeddings WHERE recording_id = ?1",
            params![recording_id],
        )?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO speaker_embeddings
                   (recording_id, speaker_id, vector, model, duration_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for e in embeddings {
                stmt.execute(params![
                    recording_id,
                    e.speaker_id,
                    f32_to_blob(&e.vector),
                    model,
                    e.duration_ms as i64,
                ])?;
            }
        }
        // 4) ライブラリ照合は再計算対象なのでリンクを消す（ADR-0018）。ただし会議の自分（`self`）は
        //    mic トラック由来で再分離されない（Issue #102）ので、手で付けたリンクを残す。
        tx.execute(
            "DELETE FROM speaker_matches WHERE recording_id = ?1 AND speaker_id <> ?2",
            params![recording_id, crate::merge::SELF_SPEAKER_ID],
        )?;
        // 5) 既存要約を stale マーク（元の文字起こし/話者が変わった）。
        tx.execute(
            "UPDATE summaries SET stale = 1 WHERE recording_id = ?1",
            params![recording_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 録音の話者声紋（speaker_id, vector）を読む。後付け再話者分離で旧声紋を取り、
    /// 新話者と cosine マッチして display_name を引き継ぐのに使う（`carry_display_names`）。
    pub fn get_speaker_embeddings(&self, recording_id: &str) -> Result<Vec<(String, Vec<f32>)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT speaker_id, vector FROM speaker_embeddings
             WHERE recording_id = ?1 ORDER BY speaker_id ASC",
        )?;
        let rows = stmt
            .query_map(params![recording_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    blob_to_f32(&r.get::<_, Vec<u8>>(1)?),
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// 既存 recording に要約を後追加（同一 recording に複数テンプレ可）。
    pub fn save_summary(&self, recording_id: &str, summary: &Summary) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        // created_at は DDL の DEFAULT (datetime('now')) に任せる。
        tx.execute(
            "INSERT INTO summaries (recording_id, template_id, content)
             VALUES (?1, ?2, ?3)",
            params![recording_id, summary.template_id, summary.content],
        )?;
        let summary_id = tx.last_insert_rowid();
        {
            let mut stmt = tx.prepare(
                "INSERT INTO action_items (summary_id, idx, text, assignee, due)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (i, item) in summary.action_items.iter().enumerate() {
                stmt.execute(params![
                    summary_id,
                    i as i64,
                    item.text,
                    item.assignee,
                    item.due
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Store the mic-vs-system start offset of a meeting recording (Issue #65). Positive
    /// means the mic track started later. Kept on the row so re-processing produces the
    /// same merge. A missing row is a no-op.
    pub fn set_mic_offset_ms(&self, recording_id: &str, offset_ms: i64) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "UPDATE recordings SET mic_offset_ms = ?1 WHERE id = ?2",
            params![offset_ms, recording_id],
        )?;
        Ok(())
    }

    /// The stored start offset; `None` for single-track recordings, rows older than
    /// schema v6, and unknown ids.
    pub fn get_mic_offset_ms(&self, recording_id: &str) -> Result<Option<i64>> {
        use rusqlite::OptionalExtension;
        let conn = self.conn();
        let v: Option<Option<i64>> = conn
            .query_row(
                "SELECT mic_offset_ms FROM recordings WHERE id = ?1",
                params![recording_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(v.flatten())
    }

    /// 履歴一覧（created_at 降順）。
    pub fn list_recordings(&self) -> Result<Vec<Recording>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, source_type, title, duration_ms, sample_rate, created_at
             FROM recordings ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], row_to_recording)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 履歴一覧を状態つきで返す（created_at 降順・Issue #109）。直近のジョブは作成順で最後の 1 本。
    pub fn list_recording_rows(&self) -> Result<Vec<RecordingRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT r.id, r.source_type, r.title, r.duration_ms, r.sample_rate, r.created_at,
                    (SELECT COUNT(*) FROM segments s WHERE s.recording_id = r.id),
                    (SELECT COUNT(*) FROM speakers sp WHERE sp.recording_id = r.id),
                    (SELECT COUNT(*) FROM summaries su WHERE su.recording_id = r.id),
                    j.kind, j.status, j.error
             FROM recordings r
             LEFT JOIN jobs j ON j.id = (
               SELECT id FROM jobs WHERE recording_id = r.id AND kind <> 'title'
               ORDER BY created_at DESC, rowid DESC LIMIT 1
             )
             ORDER BY r.created_at DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            let kind: Option<String> = r.get(9)?;
            Ok(RecordingRow {
                recording: row_to_recording(r)?,
                segment_count: r.get::<_, i64>(6)? as u32,
                speaker_count: r.get::<_, i64>(7)? as u32,
                summary_count: r.get::<_, i64>(8)? as u32,
                latest_job: match kind {
                    Some(kind) => Some(JobBrief { kind, status: r.get(10)?, error: r.get(11)? }),
                    None => None,
                },
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 履歴詳細（Recording + Transcript + 全 Summary）。無ければ `None`。
    pub fn get_recording_detail(&self, recording_id: &str) -> Result<Option<RecordingDetail>> {
        let conn = self.conn();

        // recording 本体（+ language）。無ければ None で早期 return。
        let rec_row = conn
            .query_row(
                // 列 0-5 は recordings（row_to_recording と一致）、列 6 は language（search.rs と同型）。
                "SELECT id, source_type, title, duration_ms, sample_rate, created_at, language
                 FROM recordings WHERE id = ?1",
                params![recording_id],
                |r| {
                    let recording = row_to_recording(r)?;
                    let language: Option<String> = r.get(6)?;
                    Ok((recording, language))
                },
            )
            .optional()?;
        let Some((recording, language)) = rec_row else {
            return Ok(None);
        };

        // segments（idx 昇順で順序保持）
        let segments = {
            let mut stmt = conn.prepare(
                "SELECT idx, start_ms, end_ms, text, speaker_id FROM segments
                 WHERE recording_id = ?1 ORDER BY idx ASC",
            )?;
            let rows = stmt
                .query_map(params![recording_id], |r| {
                    Ok(Segment {
                        idx: r.get::<_, i64>(0)? as u32,
                        start_ms: r.get::<_, i64>(1)? as u64,
                        end_ms: r.get::<_, i64>(2)? as u64,
                        text: r.get(3)?,
                        speaker_id: r.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };

        // summaries（古い順）+ それぞれの action_items
        let sum_rows = {
            // stale 列は v5 で追加。open_readonly で未 migrate の旧 DB（MCP リーダー）を読む場合は
            // 列が無いので、存在しなければ `0`（=false）を選ぶ（speakers 表の有無チェックと同思想）。
            let stale_col = if column_exists(&conn, "summaries", "stale")? {
                "stale"
            } else {
                "0"
            };
            let mut stmt = conn.prepare(&format!(
                "SELECT id, template_id, content, {stale_col} FROM summaries
                 WHERE recording_id = ?1 ORDER BY created_at ASC, id ASC"
            ))?;
            let rows = stmt
                .query_map(params![recording_id], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)? != 0,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        // 同一 (recording_id, template_id) の要約を再生成すると summaries 表に行が累積する
        // （save_summary は素の INSERT で upsert しない）。エクスポート（Notion/Slack）や MCP に
        // 旧要約が混ざらないよう、ここで template_id ごと最新行に畳む。古い順に走査し、既出
        // template_id は内容を最新で置換（初出位置は保持）— フロント DetailView のメモリ内 dedup と一致させる。
        let mut summaries: Vec<Summary> = Vec::with_capacity(sum_rows.len());
        for (sid, template_id, content, stale) in sum_rows {
            let mut stmt = conn.prepare(
                "SELECT text, assignee, due FROM action_items
                 WHERE summary_id = ?1 ORDER BY idx ASC",
            )?;
            let action_items = stmt
                .query_map(params![sid], |r| {
                    Ok(ActionItem {
                        text: r.get(0)?,
                        assignee: r.get(1)?,
                        due: r.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let summary = Summary {
                template_id,
                content,
                action_items,
                stale,
            };
            match summaries
                .iter_mut()
                .find(|s| s.template_id == summary.template_id)
            {
                Some(slot) => *slot = summary, // 再生成 → 最新で置換
                None => summaries.push(summary),
            }
        }

        // speakers（話者分離した録音のみ行が存在。無ければ空 Vec でフロントが既定ラベルへ）。
        // open_readonly で v2 DB（speakers 表なし・no-migrate）を開く場合があるため、
        // 表の存在を確認してから読む。表が無ければ空 Vec にフォールバックする。
        let speakers = {
            let has_speakers = conn
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type='table' AND name='speakers'",
                    [],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if has_speakers {
                let mut stmt = conn.prepare(
                    "SELECT speaker_id, label, display_name FROM speakers
                     WHERE recording_id = ?1 ORDER BY speaker_id ASC",
                )?;
                let rows = stmt
                    .query_map(params![recording_id], |r| {
                        Ok(Speaker {
                            id: r.get(0)?,
                            label: r.get(1)?,
                            display_name: r.get(2)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            } else {
                Vec::new()
            }
        };

        // 進行中ジョブ（pending|running）。jobs は v5 で追加なので、未 migrate の旧 DB
        // （open_readonly・MCP リーダー）では表が無い → 表の存在を確認してから読む（無ければ None）。
        // 既に保持している conn を渡す（self.active_job_for_recording は self.conn() を再ロックして
        // std Mutex デッドロックになるため使わない）。
        let active_job = if table_exists(&conn, "jobs")? {
            super::job::active_job_row(&conn, recording_id)?
        } else {
            None
        };

        Ok(Some(RecordingDetail {
            recording,
            transcript: Transcript { language, segments },
            summaries,
            speakers,
            active_job,
        }))
    }

    /// 録音タイトルを変更する。`title` が None/空白なら NULL（既定の「無題」表示へ戻す）。
    /// recordings.title と rec_fts.title を同一トランザクションで更新し、全文検索の整合を保つ。
    /// （rec_fts.recording_id は UNINDEXED。delete_recording と同様 = で引き当て可能。）
    /// 該当録音が無ければ 0 行更新で no-op（エラーにしない）。
    pub fn rename_recording(&self, recording_id: &str, title: Option<&str>) -> Result<()> {
        let title = title.map(str::trim).filter(|s| !s.is_empty());
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE recordings SET title = ?2 WHERE id = ?1",
            params![recording_id, title],
        )?;
        tx.execute(
            "UPDATE rec_fts SET title = ?2 WHERE recording_id = ?1",
            params![recording_id, title.unwrap_or("")],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 1 件削除（FK CASCADE で segments/summaries/action_items も消える）。
    /// rec_fts は standalone（FK 無関係）なので明示削除が必要。整合のため同一トランザクションで。
    pub fn delete_recording(&self, recording_id: &str) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM rec_fts WHERE recording_id = ?1",
            params![recording_id],
        )?;
        tx.execute(
            "DELETE FROM recordings WHERE id = ?1",
            params![recording_id],
        )?;
        tx.commit()?;
        Ok(())
    }
}
