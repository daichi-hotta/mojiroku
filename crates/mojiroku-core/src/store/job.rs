//! 永続ジョブキュー（ADR-0024）の CRUD。mod.rs から分割。
//!
//! 録音停止/ファイル取込は録音行だけ先に作り、重い処理（STT / 後付け話者分離）をここへ投入する。
//! ワーカー（src-tauri/src/jobs.rs）が `next_pending_job` で 1 本ずつ pull し、完了/失敗を書き戻す。
//! プロセス再起動時は `requeue_running_jobs` で running を pending へ戻し継続する。
use super::*;

/// `jobs` の 1 行 → `Job`。列順は下の SELECT_COLS と一致させること。
/// `params` は JSON 文字列カラム。壊れていたら既定（ja / diarize=false）にフォールバックする
/// （ジョブ 1 本の params 破損で復元不能にしない。ワーカー側は妥当なデフォルトで回る）。
fn row_to_job(r: &rusqlite::Row) -> rusqlite::Result<Job> {
    let params_json: String = r.get(4)?;
    let params: JobParams = serde_json::from_str(&params_json).unwrap_or(JobParams {
        diarize: false,
        stt_lang: None,
        transcription_model: String::new(),
        lang: "ja".to_string(),
    });
    Ok(Job {
        id: r.get(0)?,
        recording_id: r.get(1)?,
        kind: r.get(2)?,
        status: r.get(3)?,
        params,
        stage: r.get(5)?,
        error: r.get(6)?,
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
    })
}

/// row_to_job が期待する列順（全メソッドで使い回す）。
const SELECT_COLS: &str =
    "id, recording_id, kind, status, params, stage, error, created_at, updated_at";

/// `active_job_for_recording` の実体（`&Connection` を受ける版）。呼び出し側が既に conn を
/// 保持している場合に使う — `get_recording_detail` は自分の conn を渡すことで、`self.conn()` の
/// 再ロック（std Mutex は再入不可＝デッドロック）を避ける。pub(super) で store モジュール内に限定公開。
/// 文字起こし後にタイトルを自動で付けるジョブ（Issue #4）。裏方の処理なので、録音の状態
/// （一覧の処理中・失敗の印、詳細画面の処理中表示）には数えない。
pub const TITLE_JOB_KIND: &str = "title";

pub(super) fn active_job_row(conn: &Connection, recording_id: &str) -> Result<Option<Job>> {
    let job = conn
        .query_row(
            &format!(
                "SELECT {SELECT_COLS} FROM jobs
                 WHERE recording_id = ?1 AND status IN ('pending', 'running')
                   AND kind <> '{TITLE_JOB_KIND}'
                 ORDER BY updated_at DESC, id ASC LIMIT 1"
            ),
            params![recording_id],
            row_to_job,
        )
        .optional()?;
    Ok(job)
}

impl SqliteStore {
    /// ジョブを投入する（status='pending'）。`params` は enqueue 時点の設定スナップショット。
    pub fn enqueue_job(
        &self,
        id: &str,
        recording_id: &str,
        kind: &str,
        params: &JobParams,
    ) -> Result<()> {
        let params_json =
            serde_json::to_string(params).map_err(|e| CoreError::Db(e.to_string()))?;
        let conn = self.conn();
        conn.execute(
            "INSERT INTO jobs (id, recording_id, kind, status, params)
             VALUES (?1, ?2, ?3, 'pending', ?4)",
            params![id, recording_id, kind, params_json],
        )?;
        Ok(())
    }

    /// 次に実行すべき pending ジョブ（投入順）。無ければ None。
    pub fn next_pending_job(&self) -> Result<Option<Job>> {
        let conn = self.conn();
        let job = conn
            .query_row(
                &format!(
                    "SELECT {SELECT_COLS} FROM jobs
                     WHERE status = 'pending' ORDER BY created_at ASC, id ASC LIMIT 1"
                ),
                [],
                row_to_job,
            )
            .optional()?;
        Ok(job)
    }

    /// 録音に紐づく「進行中」ジョブ（pending|running）。詳細ビューの初期状態決定に使う。
    /// 複数あれば最新（updated_at 降順）の 1 本。
    pub fn active_job_for_recording(&self, recording_id: &str) -> Result<Option<Job>> {
        let conn = self.conn();
        active_job_row(&conn, recording_id)
    }

    /// 指定ステータス群のジョブ一覧（更新の新しい順）。UI のキュー表示・バッジ用。
    pub fn list_jobs(&self, statuses: &[&str]) -> Result<Vec<Job>> {
        if statuses.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn();
        // IN (?,?,..) を statuses 数だけ生成する（動的だが値はプレースホルダで安全）。
        let placeholders = vec!["?"; statuses.len()].join(", ");
        let mut stmt = conn.prepare(&format!(
            "SELECT {SELECT_COLS} FROM jobs
             WHERE status IN ({placeholders}) ORDER BY updated_at DESC, id ASC"
        ))?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(statuses), row_to_job)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// running へ遷移（stage/error はクリアしない）。
    pub fn set_job_running(&self, id: &str) -> Result<()> {
        self.update_job_status(id, "running", None, None)
    }

    /// 進捗ステージを記録（表示用）。status は据え置き。
    pub fn set_job_stage(&self, id: &str, stage: &str) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "UPDATE jobs SET stage = ?2, updated_at = datetime('now') WHERE id = ?1",
            params![id, stage],
        )?;
        Ok(())
    }

    /// 正常完了。
    pub fn set_job_done(&self, id: &str) -> Result<()> {
        self.update_job_status(id, "done", None, None)
    }

    /// 失敗（キー化メッセージを保存）。
    pub fn set_job_failed(&self, id: &str, error: &str) -> Result<()> {
        self.update_job_status(id, "failed", Some(error), None)
    }

    /// 実行中に中断されたジョブを canceled にする（Issue #114）。ワーカーが処理を止めた後に呼ぶ。
    pub fn set_job_canceled(&self, id: &str) -> Result<()> {
        self.update_job_status(id, "canceled", None, None)
    }

    /// pending ジョブをキャンセルする（running は対象外）。canceled にできたら true。
    /// running の中断はワーカーの中断フラグで行い、止まった後に `set_job_canceled` で記録する。
    pub fn cancel_job(&self, id: &str) -> Result<bool> {
        let conn = self.conn();
        let n = conn.execute(
            "UPDATE jobs SET status = 'canceled', updated_at = datetime('now')
             WHERE id = ?1 AND status = 'pending'",
            params![id],
        )?;
        Ok(n > 0)
    }

    /// プロセス再起動時の復帰: 中断された running を pending へ戻す。戻した本数を返す。
    /// （実行中にアプリが落ちるとジョブは running のまま残る。音声はディスクにあるので再実行で足りる。）
    pub fn requeue_running_jobs(&self) -> Result<usize> {
        let conn = self.conn();
        let n = conn.execute(
            "UPDATE jobs SET status = 'pending', stage = NULL, updated_at = datetime('now')
             WHERE status = 'running'",
            [],
        )?;
        Ok(n)
    }

    /// status（+任意で error/stage）を更新する内部ヘルパ。
    fn update_job_status(
        &self,
        id: &str,
        status: &str,
        error: Option<&str>,
        stage: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "UPDATE jobs SET status = ?2, error = ?3, stage = COALESCE(?4, stage),
                             updated_at = datetime('now')
             WHERE id = ?1",
            params![id, status, error, stage],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schemas::{Recording, SourceType};

    fn rec(id: &str) -> Recording {
        Recording {
            id: id.to_string(),
            source_type: SourceType::Mic,
            title: Some("録音".into()),
            duration_ms: 0,
            sample_rate: 16000,
            created_at: "2026-07-07T10:00:00Z".into(),
        }
    }

    fn params() -> JobParams {
        JobParams { diarize: true, stt_lang: Some("ja".into()), transcription_model: String::new(), lang: "ja".into() }
    }

    #[test]
    fn enqueue_and_next_pending_fifo() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.insert_recording_only(&rec("r1")).unwrap();
        s.insert_recording_only(&rec("r2")).unwrap();
        s.enqueue_job("j1", "r1", "transcribe", &params()).unwrap();
        s.enqueue_job("j2", "r2", "diarize", &params()).unwrap();
        // 投入順（created_at, id）で最初の 1 本。
        let j = s.next_pending_job().unwrap().unwrap();
        assert_eq!(j.id, "j1");
        assert_eq!(j.kind, "transcribe");
        assert!(j.params.diarize);
        assert_eq!(j.params.lang, "ja");
    }

    #[test]
    fn status_transitions_and_stage() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.insert_recording_only(&rec("r1")).unwrap();
        s.enqueue_job("j1", "r1", "transcribe", &params()).unwrap();
        s.set_job_running("j1").unwrap();
        s.set_job_stage("j1", "transcribe").unwrap();
        // running は next_pending に出ない。
        assert!(s.next_pending_job().unwrap().is_none());
        let active = s.active_job_for_recording("r1").unwrap().unwrap();
        assert_eq!(active.status, "running");
        assert_eq!(active.stage.as_deref(), Some("transcribe"));
        s.set_job_done("j1").unwrap();
        assert!(s.active_job_for_recording("r1").unwrap().is_none());
        // done は list_jobs(["done"]) に出る。
        assert_eq!(s.list_jobs(&["done"]).unwrap().len(), 1);
    }

    #[test]
    fn failed_records_error() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.insert_recording_only(&rec("r1")).unwrap();
        s.enqueue_job("j1", "r1", "transcribe", &params()).unwrap();
        s.set_job_running("j1").unwrap();
        s.set_job_failed("j1", "error.job.boom").unwrap();
        let j = s.list_jobs(&["failed"]).unwrap();
        assert_eq!(j.len(), 1);
        assert_eq!(j[0].error.as_deref(), Some("error.job.boom"));
    }

    #[test]
    fn cancel_only_pending() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.insert_recording_only(&rec("r1")).unwrap();
        s.enqueue_job("j1", "r1", "transcribe", &params()).unwrap();
        assert!(s.cancel_job("j1").unwrap()); // pending → canceled
        assert!(!s.cancel_job("j1").unwrap()); // 既に canceled は false
        // running はキャンセル不可。
        s.enqueue_job("j2", "r1", "transcribe", &params()).unwrap();
        s.set_job_running("j2").unwrap();
        assert!(!s.cancel_job("j2").unwrap());
    }

    #[test]
    fn requeue_running_on_restart() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.insert_recording_only(&rec("r1")).unwrap();
        s.enqueue_job("j1", "r1", "transcribe", &params()).unwrap();
        s.set_job_running("j1").unwrap();
        s.set_job_stage("j1", "decode").unwrap();
        // 再起動相当: running → pending（stage クリア）。
        assert_eq!(s.requeue_running_jobs().unwrap(), 1);
        let j = s.next_pending_job().unwrap().unwrap();
        assert_eq!(j.id, "j1");
        assert!(j.stage.is_none());
    }

    #[test]
    fn jobs_cascade_on_recording_delete() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.insert_recording_only(&rec("r1")).unwrap();
        s.enqueue_job("j1", "r1", "transcribe", &params()).unwrap();
        s.delete_recording("r1").unwrap();
        // FK CASCADE でジョブ行も消える。
        assert!(s.list_jobs(&["pending", "running", "done", "failed"]).unwrap().is_empty());
    }

    #[test]
    fn get_recording_detail_includes_active_job() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.insert_recording_only(&rec("r1")).unwrap();
        // ジョブ無し → active_job は None。
        assert!(s.get_recording_detail("r1").unwrap().unwrap().active_job.is_none());
        // pending を積むと詳細に同梱される（詳細ビューを「処理中」で開くため）。
        s.enqueue_job("j1", "r1", "transcribe", &params()).unwrap();
        let d = s.get_recording_detail("r1").unwrap().unwrap();
        assert_eq!(d.active_job.as_ref().unwrap().id, "j1");
        assert_eq!(d.active_job.as_ref().unwrap().status, "pending");
        // 完了すると active（pending|running）ではなくなる。
        s.set_job_running("j1").unwrap();
        s.set_job_done("j1").unwrap();
        assert!(s.get_recording_detail("r1").unwrap().unwrap().active_job.is_none());
        // タイトル生成（Issue #4）は詳細を「処理中」にしない。
        s.enqueue_job("j2", "r1", TITLE_JOB_KIND, &params()).unwrap();
        assert!(s.get_recording_detail("r1").unwrap().unwrap().active_job.is_none());
    }

    #[test]
    fn params_json_roundtrip_with_defaults() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.insert_recording_only(&rec("r1")).unwrap();
        let p = JobParams { diarize: false, stt_lang: None, transcription_model: String::new(), lang: "en".into() };
        s.enqueue_job("j1", "r1", "diarize", &p).unwrap();
        let j = s.next_pending_job().unwrap().unwrap();
        assert!(!j.params.diarize);
        assert!(j.params.stt_lang.is_none());
        assert_eq!(j.params.lang, "en");
    }

    #[test]
    fn old_jobs_default_to_turbo_and_new_jobs_keep_their_model() {
        use crate::models::{select_whisper_model, DEFAULT_WHISPER_MODEL, FULL_WHISPER_MODEL};
        let old: JobParams = serde_json::from_str(r#"{"lang":"en"}"#).unwrap();
        assert_eq!(select_whisper_model(Some(&old.transcription_model)).file, DEFAULT_WHISPER_MODEL);
        let store = SqliteStore::open_in_memory().unwrap();
        store.insert_recording_only(&rec("model-test")).unwrap();
        let mut chosen = old;
        chosen.transcription_model = FULL_WHISPER_MODEL.to_string();
        store.enqueue_job("model-job", "model-test", "transcribe", &chosen).unwrap();
        store.set_job_running("model-job").unwrap();
        store.requeue_running_jobs().unwrap();
        let restored = store.next_pending_job().unwrap().unwrap();
        assert_eq!(restored.params.transcription_model, FULL_WHISPER_MODEL);
    }
}
