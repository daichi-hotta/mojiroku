//! 永続化（SQLite / rusqlite）。履歴・詳細（`docs/03_design/spec.md` §12）。
//!
//! `mojiroku-core` は Tauri 非依存（`lib.rs` 冒頭の約束）。本 `SqliteStore` は
//! rusqlite のみに依存し、`Mutex<Connection>` を内部に持つ。`app.manage` / `State`
//! は `src-tauri` 側に置く。`schemas.rs` のメモリ型は変えず、エンティティ間の関連は
//! DB 層と下記 DTO だけで持つ（`Transcript`/`Summary` に `recording_id` を足さない）。

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::schemas::{ActionItem, Recording, Segment, SourceType, Speaker, Summary, Transcript};

mod embedding;
mod job;
mod speaker;
mod search;
mod recording;
mod translation;
pub use job::TITLE_JOB_KIND;
pub use translation::{validate_live_translations, SavedLiveTranslation};
use embedding::{blob_to_f32, dot, f32_to_blob, l2_mean};

/// 履歴詳細。`Transcript`/`Summary` に `recording_id` を足さず集約だけ持つ DTO。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingDetail {
    pub recording: Recording,
    pub transcript: Transcript,
    pub summaries: Vec<Summary>,
    /// 話者分離を行った録音の話者一覧（既定ラベル + 改名）。未分離なら空。
    #[serde(default)]
    pub speakers: Vec<Speaker>,
    /// 進行中（pending|running）のジョブ（あれば）。詳細ビューを「処理中」で開くための同梱で、
    /// フロントは 2 重 fetch せずここから初期状態を決める（ADR-0024）。未処理/完了のみなら None。
    #[serde(default)]
    pub active_job: Option<Job>,
}

/// 端末内に登録された話者（人物）。話者ライブラリ（ADR-0018）。
/// `identified_count` は対応づけ済み（speaker_matches）の録音話者数。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibrarySpeaker {
    pub id: String,
    pub name: String,
    pub identified_count: i64,
}

/// 録音話者を話者ライブラリへ照合した結果（サジェスト先行・ADR-0018）。
/// τ で機械確定はせず confidence/margin を返し、UI/ユーザーが採否を決める。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeakerMatchSuggestion {
    pub speaker_id: String,
    /// 既に確定済みの対応づけ（あれば）。
    pub linked_library_id: Option<String>,
    /// 最近接のライブラリ人物（候補）。ライブラリ空なら None。
    pub top_library_id: Option<String>,
    pub top_name: Option<String>,
    /// 最近接の cosine（声紋一致度 0..1）。
    pub confidence: Option<f64>,
    /// 2 位との差（大きいほど曖昧でない）。
    pub margin: Option<f64>,
    /// 声が短すぎて照合対象外（最小エンロール尺ゲート）。
    pub below_enroll_gate: bool,
}

/// 重い処理ジョブの実行時パラメータ（enqueue 時にスナップショットして `jobs.params` に JSON 保存）。
/// キュー待機中に設定が変わってもジョブは投入時の言語・話者分離指定で回る（ワーカーは live 設定を読まない）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobParams {
    /// 文字起こしに話者分離を含めるか（kind='transcribe' のみ意味を持つ）。
    #[serde(default)]
    pub diarize: bool,
    /// whisper への言語ヒント（None=自動判定）。
    #[serde(default)]
    pub stt_lang: Option<String>,
    /// Offline model captured at enqueue time. Old jobs default to turbo.
    #[serde(default)]
    pub transcription_model: String,
    /// 話者ラベル・既定タイトル等のコンテンツ言語（"ja"|"en"）。
    pub lang: String,
}

/// 重い処理ジョブ（文字起こし / 後付け話者分離）。永続キュー（ADR-0024）の 1 行。
/// `recording_id` に紐づき、録音削除で FK CASCADE により消える。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub recording_id: String,
    /// "transcribe" | "diarize" | "title"（[`TITLE_JOB_KIND`]）。
    pub kind: String,
    /// "pending" | "running" | "done" | "failed" | "canceled"。
    pub status: String,
    pub params: JobParams,
    /// 直近の処理ステージ（decode/transcribe/diarization/merge・表示用）。
    #[serde(default)]
    pub stage: Option<String>,
    /// failed 時のキー化メッセージ。
    #[serde(default)]
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// 履歴一覧の 1 行（Issue #109）。一覧で状態と種類を出すための集計を Recording に添える。
///
/// `Recording`（schemas）自体は広げない。MCP サーバーや保存処理も同じ型を使っているため。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingRow {
    pub recording: Recording,
    /// 文字起こしの発言数。0 なら未文字起こし。
    pub segment_count: u32,
    /// 話者の数（会議の「自分」を含む）。0 なら話者分離していない。
    pub speaker_count: u32,
    /// 保存済みの要約（議事録・要約・アクションアイテム）の数。
    pub summary_count: u32,
    /// 直近に投入したジョブ。一度も投入していなければ None。
    pub latest_job: Option<JobBrief>,
}

/// 一覧に出す分だけのジョブ情報。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobBrief {
    pub kind: String,
    pub status: String,
    pub error: Option<String>,
}

/// 全文検索の 1 ヒット。Recording 本体 + マッチ箇所スニペット。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub recording: Recording,
    /// FTS5 `snippet()`（trigram 経路）。LIKE フォールバック時はマッチした本文先頭。
    pub snippet: String,
}

/// SQLite 永続化。接続を 1 本 `Mutex` で持つ（デスクトップ・低並行で十分）。
pub struct SqliteStore {
    conn: Mutex<Connection>,
}

const SCHEMA_VERSION: i64 = 7;

/// 最小エンロール尺（ms）。これ未満の話者は声紋が不安定で照合/登録の対象外（ADR-0018, 暫定）。
/// スパイクで「短い音声では同一人物でも一致が崩れる」ことを観測したため尺でゲートする。
pub const MIN_ENROLL_MS: u64 = 20_000;

const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS recordings (
  id          TEXT    PRIMARY KEY,
  source_type TEXT    NOT NULL,
  title       TEXT,
  duration_ms INTEGER NOT NULL,
  sample_rate INTEGER NOT NULL,
  language    TEXT,
  created_at  TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_recordings_created_at ON recordings(created_at DESC);

CREATE TABLE IF NOT EXISTS segments (
  id           INTEGER PRIMARY KEY,
  recording_id TEXT    NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
  idx          INTEGER NOT NULL,
  start_ms     INTEGER NOT NULL,
  end_ms       INTEGER NOT NULL,
  text         TEXT    NOT NULL,
  speaker_id   TEXT
);
CREATE INDEX IF NOT EXISTS idx_segments_recording ON segments(recording_id, idx);

CREATE TABLE IF NOT EXISTS summaries (
  id           INTEGER PRIMARY KEY,
  recording_id TEXT    NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
  template_id  TEXT    NOT NULL,
  content      TEXT    NOT NULL,
  created_at   TEXT    NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_summaries_recording ON summaries(recording_id);

CREATE TABLE IF NOT EXISTS action_items (
  id         INTEGER PRIMARY KEY,
  summary_id INTEGER NOT NULL REFERENCES summaries(id) ON DELETE CASCADE,
  idx        INTEGER NOT NULL,
  text       TEXT    NOT NULL,
  assignee   TEXT,
  due        TEXT
);
"#;

// v2: 全文検索（FTS5）。1 録音 = 1 行（title + 全 segment.text を改行連結）の standalone FTS。
// 日本語は空白で区切られないため tokenizer は trigram（3 文字以上で部分一致）。
// recording_id は検索キーではなく結果引き当て用なので UNINDEXED。
const DDL_FTS: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS rec_fts USING fts5(
  title,
  body,
  recording_id UNINDEXED,
  tokenize = 'trigram'
);
"#;

// v3: 話者表。話者分離の結果（話者ごとの既定ラベルと、ユーザーが付けた表示名）を保持。
// speaker_id は segments.speaker_id（"S1" 等）と一致するキー。display_name のためだけに
// 存在する（id→label は導出可能だが、行を自己記述的にするため label も保存する）。
// 旧録音（v2 DB）は行が無い → 詳細取得で空 Vec を返し、フロントは既定ラベルにフォールバック。
const DDL_SPEAKERS: &str = r#"
CREATE TABLE IF NOT EXISTS speakers (
  recording_id TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
  speaker_id   TEXT NOT NULL,
  label        TEXT NOT NULL,
  display_name TEXT,
  PRIMARY KEY (recording_id, speaker_id)
);
"#;

// v4: 話者ライブラリ（クロス会議の声紋照合・ADR-0018）。すべて additive・冪等（IF NOT EXISTS）。
// - speaker_embeddings: 話者ごとの声紋（重心, f32 little-endian BLOB, L2 正規化済み）+ 元尺（最小エンロールゲート用）。
//   1:1 で speakers と対応するが、FK は recordings(id) に張り CASCADE で録音削除に追従する。
// - speaker_library: 端末内の登録話者（人物）。id はアプリ層が採番（UUID, recordings と同方針）。
// - speaker_matches: 録音話者 → ライブラリ人物の対応づけ（ユーザー確認 or サジェスト採用）。
// 旧録音は行ゼロのまま（フロントは既定動作にフォールバック）。
const DDL_SPEAKER_LIBRARY: &str = r#"
CREATE TABLE IF NOT EXISTS speaker_embeddings (
  recording_id TEXT    NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
  speaker_id   TEXT    NOT NULL,
  vector       BLOB    NOT NULL,
  model        TEXT    NOT NULL,
  duration_ms  INTEGER NOT NULL,
  PRIMARY KEY (recording_id, speaker_id)
);
CREATE TABLE IF NOT EXISTS speaker_library (
  id         TEXT PRIMARY KEY,
  name       TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE IF NOT EXISTS speaker_matches (
  recording_id TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
  speaker_id   TEXT NOT NULL,
  library_id   TEXT NOT NULL REFERENCES speaker_library(id) ON DELETE CASCADE,
  confidence   REAL NOT NULL,
  created_at   TEXT NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY (recording_id, speaker_id)
);
CREATE INDEX IF NOT EXISTS idx_speaker_matches_library ON speaker_matches(library_id);
"#;

// v5: 重い処理の永続ジョブキュー（ADR-0024）。録音停止/ファイル取込は録音行だけ先に作り、
// STT/話者分離をこのキューへ投入する（キャプチャと重い処理を分離）。ワーカーは同時 1 本で直列実行
// （HEAVY_ML_JOB セマフォと同思想。16GB でのメモリ枯渇クラッシュ回避・ADR-0021）。
// recording_id に FK CASCADE を張るので録音削除でジョブ行も消える（delete_recording 無変更）。
const DDL_JOBS: &str = r#"
CREATE TABLE IF NOT EXISTS jobs (
  id           TEXT PRIMARY KEY,
  recording_id TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
  kind         TEXT NOT NULL,
  status       TEXT NOT NULL,
  params       TEXT NOT NULL,
  stage        TEXT,
  error        TEXT,
  created_at   TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_jobs_status ON jobs(status);
CREATE INDEX IF NOT EXISTS idx_jobs_recording ON jobs(recording_id);
"#;

impl SqliteStore {
    /// ファイル DB を開き、PRAGMA とマイグレーションを適用する。
    pub fn open(db_path: &Path) -> Result<Self> {
        Self::init(Connection::open(db_path)?)
    }

    /// テスト用のインメモリ DB。
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    /// 読み取り専用に DB を開く（MCP サーバ等、アプリ外のリーダー向け）。
    ///
    /// **`open()` と違い `migrate()` も `journal_mode` 設定も行わない**。アプリ本体と
    /// 外部リーダー（mojiroku-mcp）は別々に更新され得る（バージョン skew）ため、
    /// リーダーがアプリ所有の DB を勝手に migrate するのは危険。スキーマは一切触らず、
    /// `user_version` の互換性だけを検査する。
    ///
    /// OS レベルでは read-write ハンドルで開く（書き込みクエリは一切発行しない）。
    /// **理由**: 厳格な `SQLITE_OPEN_READ_ONLY` は WAL の `-shm`/`-wal` を生成できず、
    /// アプリ非起動時に DB を開けないことがあるため。
    ///
    /// DB のスキーマがこのバイナリの理解する `SCHEMA_VERSION` より**新しい**場合は、
    /// 誤読を避けるため明示的に `Err` を返す（古い分には読める範囲で許容）。
    pub fn open_readonly(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        // read-only 性を接続レベルで機械的に強制する（規約だけに依存しない）。
        // query_only は SQL 文レベルの書き込み拒否なので、WAL の -shm/-wal 生成
        // （SQLITE_OPEN_READ_ONLY で不可能だった点）はそのまま許される。
        conn.pragma_update(None, "query_only", "ON")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(CoreError::Db(format!(
                "DB schema version {version} is newer than supported {SCHEMA_VERSION}; \
                 update mojiroku-mcp to match the app"
            )));
        }
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn init(conn: Connection) -> Result<Self> {
        // rusqlite は既定で FK 非強制。CASCADE を効かせるため接続ごとに ON にする（最重要）。
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// 内部接続をロックして返す（各メソッドで重複していた lock().unwrap() 定型を集約）。
    /// desktop 単一プロセスで poison は現実的でないため、poison 時はメッセージ付き panic
    /// （従来の `.unwrap()` と挙動同値）。書き込みは `let mut conn = self.conn();` で使う。
    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().expect("db mutex poisoned")
    }
}

fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;

    // v1: 既存テーブル群（新規 DB・既存 v1 DB ともここを通す。IF NOT EXISTS で冪等）。
    if version < 1 {
        conn.execute_batch(DDL)?;
    }

    // v2: FTS テーブル作成 + 既存データ backfill を 1 トランザクションで。
    // migrate は `&Connection`（`transaction()` は &mut 必要）なので unchecked_transaction を使う。
    if version < 2 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(DDL_FTS)?;
        backfill_fts(&tx)?;
        tx.commit()?;
    }

    // v3: 話者表（additive）。backfill 不要 — 旧録音は話者分離していないので行ゼロのまま。
    if version < 3 {
        conn.execute_batch(DDL_SPEAKERS)?;
    }

    // v4: 話者ライブラリ（声紋照合・ADR-0018, additive）。backfill 不要。
    if version < 4 {
        conn.execute_batch(DDL_SPEAKER_LIBRARY)?;
    }

    // v5: 永続ジョブキュー（jobs, additive）+ summaries.stale 列。
    // ⚠️ この段は他と違い**非再入**（要注意）: summaries.stale は IF NOT EXISTS の効かない
    // `ALTER TABLE ADD COLUMN` なので、失敗して version<5 のまま再実行されると二重 ADD で
    // `duplicate column` になる。→ 列の存在を table_info で確認してから ADD する。jobs 側は
    // IF NOT EXISTS で冪等。両者を 1 トランザクションにまとめ、部分適用を残さない。
    if version < 5 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(DDL_JOBS)?;
        if !column_exists(&tx, "summaries", "stale")? {
            tx.execute_batch("ALTER TABLE summaries ADD COLUMN stale INTEGER NOT NULL DEFAULT 0")?;
        }
        tx.commit()?;
    }

    // v6: recordings.mic_offset_ms (Issue #65): start offset between the mic and system
    // tracks of a meeting recording, in ms (positive = mic started later). NULL for older
    // rows and single-track recordings. ADD COLUMN is not idempotent, so check first (as v5).
    if version < 6 && !column_exists(conn, "recordings", "mic_offset_ms")? {
        conn.execute_batch("ALTER TABLE recordings ADD COLUMN mic_offset_ms INTEGER")?;
    }

    // v7: saved live-caption translations, separate from the final transcript.
    if version < 7 {
        conn.execute_batch(translation::DDL)?;
    }

    // 全段階の後ろで一括 bump。途中失敗時は version<2 のまま再実行され、
    // backfill_fts 先頭の DELETE で二重投入を防ぐ。
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

/// テーブルに指定カラムが存在するか（非冪等な ALTER TABLE ADD COLUMN のガード用）。
fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    // table_info の列 1 が name。
    let exists = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|name| name == column);
    Ok(exists)
}

/// テーブルが存在するか。open_readonly で未 migrate の旧 DB（v5 前・MCP リーダー）を読む際、
/// v5 で追加した `jobs` を無条件に触ると "no such table" で壊れるため、参照前のガードに使う。
fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        params![table],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// 既存 recordings/segments から `rec_fts` を再構築する（マイグレーション用）。
/// 冪等性のため先頭で全削除してから入れ直す（再実行時の重複防止）。
/// body は save_recording と同じく segment.text を idx 昇順で改行連結する。
fn backfill_fts(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM rec_fts", [])?;
    let recs: Vec<(String, String)> = {
        let mut stmt = conn.prepare("SELECT id, COALESCE(title, '') FROM recordings")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let mut seg_stmt =
        conn.prepare("SELECT text FROM segments WHERE recording_id = ?1 ORDER BY idx ASC")?;
    let mut ins =
        conn.prepare("INSERT INTO rec_fts (title, body, recording_id) VALUES (?1, ?2, ?3)")?;
    for (id, title) in recs {
        let body: Vec<String> = seg_stmt
            .query_map(params![id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ins.execute(params![title, body.join("\n"), id])?;
    }
    Ok(())
}

fn row_to_recording(r: &rusqlite::Row) -> rusqlite::Result<Recording> {
    Ok(Recording {
        id: r.get(0)?,
        source_type: source_type_from_str(&r.get::<_, String>(1)?),
        title: r.get(2)?,
        duration_ms: r.get::<_, i64>(3)? as u64,
        sample_rate: r.get::<_, i64>(4)? as u32,
        created_at: r.get(5)?,
    })
}

/// serde の `rename_all = "snake_case"` と一致させる。
fn source_type_str(s: SourceType) -> &'static str {
    match s {
        SourceType::File => "file",
        SourceType::Mic => "mic",
        SourceType::Live => "live",
    }
}

fn source_type_from_str(s: &str) -> SourceType {
    match s {
        "mic" => SourceType::Mic,
        "live" => SourceType::Live,
        _ => SourceType::File,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str) -> Recording {
        Recording {
            id: id.to_string(),
            source_type: SourceType::File,
            title: Some("テスト録音".into()),
            duration_ms: 1234,
            sample_rate: 16000,
            created_at: "2026-06-24T10:00:00Z".into(),
        }
    }

    fn transcript() -> Transcript {
        Transcript {
            language: Some("ja".into()),
            segments: vec![
                Segment { idx: 0, start_ms: 0, end_ms: 1000, text: "あ".into(), speaker_id: None },
                Segment { idx: 0, start_ms: 1000, end_ms: 2000, text: "い".into(), speaker_id: None },
                Segment { idx: 0, start_ms: 2000, end_ms: 3000, text: "う".into(), speaker_id: None },
            ],
        }
    }

    fn summary(template: &str, items: Vec<ActionItem>) -> Summary {
        Summary {
            template_id: template.into(),
            content: format!("{template} の本文"),
            action_items: items,
            stale: false,
        }
    }

    fn emb(id: &str, v: Vec<f32>, dur_ms: u64) -> crate::diarization::SpeakerEmbedding {
        crate::diarization::SpeakerEmbedding {
            speaker_id: id.into(),
            vector: v,
            duration_ms: dur_ms,
        }
    }

    #[test]
    fn speaker_library_identify_cross_recording() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.save_recording(&rec("A"), &transcript(), &[]).unwrap();
        s.save_recording(&rec("B"), &transcript(), &[]).unwrap();
        // A: S1=[1,0] 60s, S2=[0,1] 60s。B: S1=[1,0] 60s（A/S1 と同一人物想定）, S2=[0,1] 10s（尺不足）。
        s.save_speaker_embeddings(
            "A",
            &[emb("S1", vec![1.0, 0.0], 60_000), emb("S2", vec![0.0, 1.0], 60_000)],
            "titanet",
        )
        .unwrap();
        s.save_speaker_embeddings(
            "B",
            &[emb("S1", vec![1.0, 0.0], 60_000), emb("S2", vec![0.0, 1.0], 10_000)],
            "titanet",
        )
        .unwrap();
        // ライブラリ: P1=A/S1, P2=A/S2。
        s.add_library_speaker("P1", "Daichi").unwrap();
        s.add_library_speaker("P2", "Other").unwrap();
        s.link_speaker("A", "S1", "P1", 1.0).unwrap();
        s.link_speaker("A", "S2", "P2", 1.0).unwrap();

        // B を照合 → S1 は P1 に当たる、S2 は尺ゲートで対象外。
        let sug = s.identify_speakers("B").unwrap();
        let s1 = sug.iter().find(|x| x.speaker_id == "S1").unwrap();
        assert_eq!(s1.top_library_id.as_deref(), Some("P1"));
        assert!(s1.confidence.unwrap() > 0.99);
        assert!(s1.margin.unwrap() > 0.9);
        assert!(!s1.below_enroll_gate);
        let s2 = sug.iter().find(|x| x.speaker_id == "S2").unwrap();
        assert!(s2.below_enroll_gate);
        assert!(s2.top_library_id.is_none());

        // 一覧と対応づけ数。
        let lib = s.list_library_speakers().unwrap();
        assert_eq!(lib.len(), 2);
        assert!(lib.iter().all(|l| l.identified_count == 1));

        // leave-one-out: A を照合すると library は A を除外 → 候補なし。ただし確定リンクは返す。
        let sa = s.identify_speakers("A").unwrap();
        assert!(sa.iter().all(|x| x.top_library_id.is_none()));
        assert_eq!(
            sa.iter().find(|x| x.speaker_id == "S1").unwrap().linked_library_id.as_deref(),
            Some("P1")
        );
    }

    #[test]
    fn speaker_library_cascade_on_recording_delete() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.save_recording(&rec("A"), &transcript(), &[]).unwrap();
        s.save_speaker_embeddings("A", &[emb("S1", vec![0.6, 0.8], 30_000)], "titanet").unwrap();
        s.add_library_speaker("P1", "X").unwrap();
        s.link_speaker("A", "S1", "P1", 0.9).unwrap();
        // 録音削除 → speaker_embeddings / speaker_matches も FK CASCADE で消える。
        s.delete_recording("A").unwrap();
        let lib = s.list_library_speakers().unwrap();
        assert_eq!(lib.len(), 1);
        assert_eq!(lib[0].identified_count, 0);
    }

    #[test]
    fn save_and_list() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.save_recording(&rec("r1"), &transcript(), &[]).unwrap();
        let list = s.list_recordings().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "r1");
        assert_eq!(list[0].duration_ms, 1234);
        assert_eq!(list[0].title.as_deref(), Some("テスト録音"));
    }

    #[test]
    fn detail_preserves_segment_order() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.save_recording(&rec("r1"), &transcript(), &[]).unwrap();
        let d = s.get_recording_detail("r1").unwrap().unwrap();
        assert_eq!(d.recording.id, "r1");
        assert_eq!(d.transcript.language.as_deref(), Some("ja"));
        let texts: Vec<&str> = d.transcript.segments.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(texts, vec!["あ", "い", "う"]); // idx 昇順
    }

    #[test]
    fn multiple_summaries_with_action_items() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.save_recording(&rec("r1"), &transcript(), &[]).unwrap();
        s.save_summary(
            "r1",
            &summary(
                "minutes",
                vec![ActionItem { text: "Xをやる".into(), assignee: Some("alice".into()), due: None }],
            ),
        )
        .unwrap();
        s.save_summary("r1", &summary("summary", vec![])).unwrap();
        let d = s.get_recording_detail("r1").unwrap().unwrap();
        assert_eq!(d.summaries.len(), 2);
        assert_eq!(d.summaries[0].template_id, "minutes");
        assert_eq!(d.summaries[0].action_items.len(), 1);
        assert_eq!(d.summaries[0].action_items[0].text, "Xをやる");
        assert_eq!(d.summaries[0].action_items[0].assignee.as_deref(), Some("alice"));
        assert_eq!(d.summaries[1].template_id, "summary");
    }

    #[test]
    fn regenerated_summary_collapses_to_latest_per_template() {
        // 同一 template_id を再生成すると DB 行は累積するが、get_recording_detail は
        // template_id ごと最新 1 件（初出位置を保持）に畳む。エクスポートに旧要約が混ざらない。
        let s = SqliteStore::open_in_memory().unwrap();
        s.save_recording(&rec("r1"), &transcript(), &[]).unwrap();
        s.save_summary("r1", &summary("minutes", vec![])).unwrap(); // 初版
        s.save_summary("r1", &summary("summary", vec![])).unwrap();
        s.save_summary(
            "r1",
            &Summary {
                template_id: "minutes".into(),
                content: "改訂版の議事録".into(), // 再生成（最新）
                action_items: vec![],
                stale: false,
            },
        )
        .unwrap();
        let d = s.get_recording_detail("r1").unwrap().unwrap();
        // 2 件（minutes / summary）に畳まれ、minutes は初出位置のまま最新内容。
        assert_eq!(d.summaries.len(), 2);
        assert_eq!(d.summaries[0].template_id, "minutes");
        assert_eq!(d.summaries[0].content, "改訂版の議事録");
        assert_eq!(d.summaries[1].template_id, "summary");
    }

    #[test]
    fn delete_cascades_children() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.save_recording(&rec("r1"), &transcript(), &[]).unwrap();
        s.save_summary(
            "r1",
            &summary("minutes", vec![ActionItem { text: "x".into(), assignee: None, due: None }]),
        )
        .unwrap();
        s.delete_recording("r1").unwrap();
        assert!(s.get_recording_detail("r1").unwrap().is_none());
        assert_eq!(s.list_recordings().unwrap().len(), 0);
        // CASCADE で子テーブルも 0 件（FK pragma が効いている証明）
        let conn = s.conn.lock().unwrap();
        let count = |t: &str| -> i64 {
            conn.query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0)).unwrap()
        };
        assert_eq!((count("segments"), count("summaries"), count("action_items")), (0, 0, 0));
    }

    #[test]
    fn missing_detail_is_none() {
        let s = SqliteStore::open_in_memory().unwrap();
        assert!(s.get_recording_detail("nope").unwrap().is_none());
    }

    // ---- 話者表（v3） ----

    fn transcript_with_speakers() -> Transcript {
        Transcript {
            language: Some("ja".into()),
            segments: vec![
                Segment { idx: 0, start_ms: 0, end_ms: 1000, text: "おはよう".into(), speaker_id: Some("S1".into()) },
                Segment { idx: 0, start_ms: 1000, end_ms: 2000, text: "はい".into(), speaker_id: Some("S2".into()) },
                Segment { idx: 0, start_ms: 2000, end_ms: 3000, text: "了解".into(), speaker_id: Some("S1".into()) },
            ],
        }
    }

    fn speakers() -> Vec<Speaker> {
        vec![
            Speaker { id: "S1".into(), label: "話者1".into(), display_name: None },
            Speaker { id: "S2".into(), label: "話者2".into(), display_name: None },
        ]
    }

    #[test]
    fn speakers_roundtrip_ids_match_segments() {
        use std::collections::BTreeSet;
        let s = SqliteStore::open_in_memory().unwrap();
        s.save_recording(&rec("r1"), &transcript_with_speakers(), &speakers()).unwrap();
        let d = s.get_recording_detail("r1").unwrap().unwrap();
        assert_eq!(d.speakers.len(), 2);
        assert_eq!(d.speakers[0].id, "S1");
        assert_eq!(d.speakers[0].label, "話者1");
        assert!(d.speakers[0].display_name.is_none());
        // 最重要: segment.speaker_id に、話者表に無い id が現れない（seg_ids ⊆ spk_ids）。
        // 崩れると改名 UI に出ない話者が発言側だけに生まれる。
        //
        // 逆向き（spk_ids ⊆ seg_ids）は**一般には成り立たない**。
        // merge::assign_speakers は各セグメントに「最も重なる turn」だけを割り当てるので、
        // turn は持つが常に他話者に負けるクラスタは**保存直後から**発言ゼロになる。
        // 発言単位の訂正（Issue #19）でも生じる — 最後の 1 件を移しても話者行は残す設計
        // （行を消すと声紋とライブラリ紐づけまで失われ、訂正を戻せなくなる）。
        // ここは全話者に発言があるフィクスチャを save_recording した直後なので、
        // このテストに限り両向きの一致を確かめてよい。
        let seg_ids: BTreeSet<_> =
            d.transcript.segments.iter().filter_map(|x| x.speaker_id.clone()).collect();
        let spk_ids: BTreeSet<_> = d.speakers.iter().map(|x| x.id.clone()).collect();
        assert_eq!(seg_ids, spk_ids);
    }

    #[test]
    fn set_segment_speaker_moves_one_utterance_only() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.save_recording(&rec("r1"), &transcript_with_speakers(), &speakers()).unwrap();

        // 2 番目（idx=1）を S2 → S1 へ訂正する。
        assert!(s.set_segment_speaker("r1", 1, Some("S1")).unwrap(), "変更したので true");

        let d = s.get_recording_detail("r1").unwrap().unwrap();
        let got: Vec<_> = d.transcript.segments.iter().map(|x| x.speaker_id.clone()).collect();
        assert_eq!(
            got,
            vec![Some("S1".into()), Some("S1".into()), Some("S1".into())],
            "指定した 1 件だけが変わる"
        );

        // idx が API に出ていて、配列の添字と一致する。
        let idxs: Vec<u32> = d.transcript.segments.iter().map(|x| x.idx).collect();
        assert_eq!(idxs, vec![0, 1, 2]);

        // 移動元（S2）の話者行は消さない。発言ゼロでも残す（訂正を戻せるように）。
        assert!(d.speakers.iter().any(|x| x.id == "S2"), "S2 の行が残っている");

        // 本文は変わらないので検索は壊れない。
        assert!(!s.search_recordings("はい").unwrap().is_empty());
    }

    #[test]
    fn set_segment_speaker_marks_summaries_stale() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.save_recording(&rec("r1"), &transcript_with_speakers(), &speakers()).unwrap();
        s.save_summary("r1", &summary("minutes", vec![])).unwrap();
        assert!(!s.get_recording_detail("r1").unwrap().unwrap().summaries[0].stale);

        assert!(s.set_segment_speaker("r1", 0, Some("S2")).unwrap());

        // 要約本文に話者名が出るため、話者を訂正したら作り直す価値がある。
        assert!(s.get_recording_detail("r1").unwrap().unwrap().summaries[0].stale);
    }

    #[test]
    fn set_segment_speaker_does_not_touch_other_recordings() {
        // 話者 id は録音ごとに S1/S2 と採番されるので、r1 にも r2 にも "S1" が存在する。
        // つまり話者の実在検証は別録音への誤書き込みを止められず、唯一の防御は
        // UPDATE ... WHERE recording_id = ?1 のスコープだけ。ここを固定する。
        let s = SqliteStore::open_in_memory().unwrap();
        for id in ["r1", "r2"] {
            s.save_recording(&rec(id), &transcript_with_speakers(), &speakers()).unwrap();
            s.save_summary(id, &summary("minutes", vec![])).unwrap();
        }

        s.set_segment_speaker("r1", 1, Some("S1")).unwrap();

        let d2 = s.get_recording_detail("r2").unwrap().unwrap();
        let got: Vec<_> = d2.transcript.segments.iter().map(|x| x.speaker_id.clone()).collect();
        assert_eq!(
            got,
            vec![Some("S1".into()), Some("S2".into()), Some("S1".into())],
            "r2 は無傷"
        );
        assert!(!d2.summaries[0].stale, "r2 の要約も stale にしない");
    }

    #[test]
    fn set_segment_speaker_handles_speakerless_recording() {
        let s = SqliteStore::open_in_memory().unwrap();
        // 話者ゼロの録音（話者分離をしていない）。
        s.save_recording(&rec("r1"), &transcript(), &[]).unwrap();
        s.save_summary("r1", &summary("minutes", vec![])).unwrap();

        // 候補が居ないので、どの話者 id も拒否される。
        assert!(s.set_segment_speaker("r1", 0, Some("S1")).is_err());
        // 元から NULL なので「話者不明へ」は no-op。要約も stale にしない。
        assert!(!s.set_segment_speaker("r1", 0, None).unwrap(), "None → None は no-op");
        assert!(!s.get_recording_detail("r1").unwrap().unwrap().summaries[0].stale);
    }

    #[test]
    fn set_segment_speaker_same_value_is_noop() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.save_recording(&rec("r1"), &transcript_with_speakers(), &speakers()).unwrap();
        s.save_summary("r1", &summary("minutes", vec![])).unwrap();

        // idx=0 は元から S1。同じ話者を選び直しても要約を stale にしない
        // （7B モデルでの作り直しが分単位で走るため、内容が変わっていないのに促すのは害）。
        assert!(!s.set_segment_speaker("r1", 0, Some("S1")).unwrap(), "同値なので false");

        let d = s.get_recording_detail("r1").unwrap().unwrap();
        assert!(!d.summaries[0].stale, "同値なら stale を立てない");
        assert_eq!(d.transcript.segments[0].speaker_id.as_deref(), Some("S1"));
    }

    #[test]
    fn set_segment_speaker_rejects_unknown_speaker_and_missing_segment() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.save_recording(&rec("r1"), &transcript_with_speakers(), &speakers()).unwrap();

        // 当該録音の speakers に無い id は拒否する。許すと speakers の id 集合と
        // segments.speaker_id の集合がズレ、改名 UI に出ない話者が生まれる。
        assert!(s.set_segment_speaker("r1", 0, Some("S99")).is_err());
        // 存在しない発言も拒否する（黙って何もしないと訂正が失われたことに気づけない）。
        assert!(s.set_segment_speaker("r1", 999, Some("S1")).is_err());

        // メッセージは `error.` 始まりの i18n キー。コマンド層の core_err が Display 接頭辞を
        // 外してフロントへ渡すので、キーが文字列の先頭に来ることが条件になる。
        let e1 = s.set_segment_speaker("r1", 0, Some("S99")).unwrap_err();
        assert!(matches!(&e1, crate::error::CoreError::Db(m) if m == "error.speaker.unknown_for_recording"));
        let e2 = s.set_segment_speaker("r1", 999, Some("S1")).unwrap_err();
        assert!(matches!(&e2, crate::error::CoreError::Db(m) if m == "error.segment.not_found"));

        // 拒否されたので中身は無傷。
        let d = s.get_recording_detail("r1").unwrap().unwrap();
        assert_eq!(d.transcript.segments[0].speaker_id.as_deref(), Some("S1"));
    }

    #[test]
    fn set_segment_speaker_can_clear_to_unknown() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.save_recording(&rec("r1"), &transcript_with_speakers(), &speakers()).unwrap();
        assert!(s.set_segment_speaker("r1", 0, None).unwrap());
        let d = s.get_recording_detail("r1").unwrap().unwrap();
        assert!(d.transcript.segments[0].speaker_id.is_none(), "話者不明へ戻せる");
    }

    #[test]
    fn rename_speaker_persists_and_resets() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.save_recording(&rec("r1"), &transcript_with_speakers(), &speakers()).unwrap();
        s.rename_speaker("r1", "S1", Some("田中")).unwrap();
        let d = s.get_recording_detail("r1").unwrap().unwrap();
        let s1 = d.speakers.iter().find(|x| x.id == "S1").unwrap();
        assert_eq!(s1.display_name.as_deref(), Some("田中"));
        // None で既定ラベルへ戻す
        s.rename_speaker("r1", "S1", None).unwrap();
        let d2 = s.get_recording_detail("r1").unwrap().unwrap();
        assert!(d2.speakers.iter().find(|x| x.id == "S1").unwrap().display_name.is_none());
        // 存在しない話者は 0 行更新で no-op（エラーにしない）
        s.rename_speaker("r1", "S9", Some("x")).unwrap();
    }

    #[test]
    fn rename_recording_persists_and_syncs_fts() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.save_recording(&rec("r1"), &transcript(), &[]).unwrap();
        // 改名 → 一覧/詳細に反映
        s.rename_recording("r1", Some("週次定例ミーティング")).unwrap();
        assert_eq!(
            s.get_recording_detail("r1").unwrap().unwrap().recording.title.as_deref(),
            Some("週次定例ミーティング"),
        );
        // FTS も同期（新タイトルでヒットし、旧タイトルでは出ない）
        assert_eq!(s.search_recordings("週次定例").unwrap().len(), 1);
        assert_eq!(s.search_recordings("テスト録音").unwrap().len(), 0);
        // 空白は NULL（既定の無題へ）。本文では引き続きヒットする
        s.rename_recording("r1", Some("   ")).unwrap();
        assert!(s.get_recording_detail("r1").unwrap().unwrap().recording.title.is_none());
        assert_eq!(s.search_recordings("週次定例").unwrap().len(), 0);
        // 存在しない録音は no-op（エラーにしない）
        s.rename_recording("nope", Some("x")).unwrap();
    }

    #[test]
    fn delete_cascades_speakers() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.save_recording(&rec("r1"), &transcript_with_speakers(), &speakers()).unwrap();
        s.delete_recording("r1").unwrap();
        let conn = s.conn.lock().unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM speakers", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0); // CASCADE で speakers も消える
    }

    #[test]
    fn v2_database_migrates_and_loads_with_empty_speakers() {
        // v1+v2 スキーマだけの「旧 DB」を手で用意し user_version=2 に固定 → init(=migrate) を通す。
        // 既存ユーザーの v2 DB が壊れず開き、話者行ゼロで詳細が引けることの回帰テスト。
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn.execute_batch(DDL).unwrap();
        conn.execute_batch(DDL_FTS).unwrap();
        conn.execute(
            "INSERT INTO recordings (id, source_type, title, duration_ms, sample_rate, language, created_at)
             VALUES ('old', 'file', '旧録音', 1000, 16000, 'ja', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO segments (recording_id, idx, start_ms, end_ms, text, speaker_id)
             VALUES ('old', 0, 0, 1000, 'こんにちは', NULL)",
            [],
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 2i64).unwrap();

        let s = SqliteStore::init(conn).unwrap();
        let d = s.get_recording_detail("old").unwrap().unwrap();
        assert!(d.speakers.is_empty(), "v2 録音は話者行ゼロ → 空 Vec（フロントは既定ラベルへ）");
        assert_eq!(d.transcript.segments.len(), 1);
        assert_eq!(d.transcript.segments[0].speaker_id, None);
        // v6 adds recordings.mic_offset_ms; old rows read back as None.
        assert!(column_exists(&s.conn(), "recordings", "mic_offset_ms").unwrap());
        assert_eq!(s.get_mic_offset_ms("old").unwrap(), None);
    }

    /// The meeting start offset round-trips, is None until set, and setting it for a
    /// missing recording is a no-op (Issue #65).
    #[test]
    fn mic_offset_round_trips() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.insert_recording_only(&rec("m")).unwrap();
        assert_eq!(s.get_mic_offset_ms("m").unwrap(), None);
        s.set_mic_offset_ms("m", 640).unwrap();
        assert_eq!(s.get_mic_offset_ms("m").unwrap(), Some(640));
        s.set_mic_offset_ms("m", -25).unwrap();
        assert_eq!(s.get_mic_offset_ms("m").unwrap(), Some(-25));
        s.set_mic_offset_ms("missing", 1).unwrap();
        assert_eq!(s.get_mic_offset_ms("missing").unwrap(), None);
    }

    // ---- 録音のみ挿入 + 再処理（ADR-0024） ----

    #[test]
    fn insert_only_then_replace_transcript_fills_body_and_duration() {
        let s = SqliteStore::open_in_memory().unwrap();
        // file 取込相当: duration=0 で録音行だけ作る。
        let mut r = rec("r1");
        r.duration_ms = 0;
        s.insert_recording_only(&r).unwrap();
        // 詳細は空 transcript で引ける。タイトル検索は可、本文は不可。
        let d = s.get_recording_detail("r1").unwrap().unwrap();
        assert!(d.transcript.segments.is_empty());
        assert!(d.transcript.language.is_none());
        // タイトルは insert 時点で FTS に載る（本文はまだ空）。
        assert_eq!(s.search_recordings("テスト録音").unwrap().len(), 1);
        assert_eq!(s.search_recordings("会議の").unwrap().len(), 0); // 本文はまだ無い
        // 文字起こしジョブ完了 → 差し替え。
        s.replace_transcript("r1", &transcript_jp(), &[]).unwrap();
        let d = s.get_recording_detail("r1").unwrap().unwrap();
        assert_eq!(d.transcript.segments.len(), 2);
        assert_eq!(d.transcript.language.as_deref(), Some("ja"));
        // duration は最終 segment 末尾（2000ms）で埋まる（0 だったので）。
        assert_eq!(d.recording.duration_ms, 2000);
        // 本文検索がヒットするようになる。
        assert_eq!(s.search_recordings("会議の").unwrap().len(), 1);
    }

    #[test]
    fn replace_transcript_preserves_known_duration() {
        // mic/会議相当: duration が既知（!=0）なら最終 segment で上書きしない。
        let s = SqliteStore::open_in_memory().unwrap();
        let mut r = rec("r1");
        r.duration_ms = 999_999;
        s.insert_recording_only(&r).unwrap();
        s.replace_transcript("r1", &transcript(), &[]).unwrap();
        assert_eq!(s.get_recording_detail("r1").unwrap().unwrap().recording.duration_ms, 999_999);
    }

    #[test]
    fn replace_speaker_assignments_carries_names_and_marks_stale() {
        let s = SqliteStore::open_in_memory().unwrap();
        // 初回: 話者 S1/S2 付きで保存 + 声紋 + 要約 + S1 を改名。
        s.save_recording(&rec("r1"), &transcript_with_speakers(), &speakers()).unwrap();
        s.save_speaker_embeddings(
            "r1",
            &[emb("S1", vec![1.0, 0.0], 60_000), emb("S2", vec![0.0, 1.0], 60_000)],
            "titanet",
        )
        .unwrap();
        s.save_summary("r1", &summary("minutes", vec![])).unwrap();
        s.rename_speaker("r1", "S1", Some("田中")).unwrap();

        // 再話者分離: 新話者 N1/N2（順序入替の声紋）。旧声紋を読んで引き継ぎを計算。
        let old = s.get_speaker_embeddings("r1").unwrap();
        assert_eq!(old.len(), 2);
        let new_speakers = vec![
            Speaker { id: "N1".into(), label: "話者1".into(), display_name: None },
            Speaker { id: "N2".into(), label: "話者2".into(), display_name: None },
        ];
        let new_emb =
            [emb("N1", vec![0.0, 1.0], 50_000), emb("N2", vec![1.0, 0.0], 50_000)];
        // N2 の声紋は旧 S1（田中）と一致 → carry。
        let old_pairs: Vec<_> = old
            .iter()
            .map(|(id, v)| {
                (
                    Speaker {
                        id: id.clone(),
                        label: String::new(),
                        display_name: if id == "S1" { Some("田中".into()) } else { None },
                    },
                    v.clone(),
                )
            })
            .collect();
        let new_pairs: Vec<_> =
            new_emb.iter().map(|e| (Speaker { id: e.speaker_id.clone(), label: String::new(), display_name: None }, e.vector.clone())).collect();
        let remap = crate::diarization::carry_display_names(&old_pairs, &new_pairs, 0.7);

        // segments に N1/N2 を割り当てた transcript（text 不変）。
        let mut t = transcript_with_speakers();
        for seg in t.segments.iter_mut() {
            seg.speaker_id = Some(if seg.speaker_id.as_deref() == Some("S1") { "N2".into() } else { "N1".into() });
        }
        s.replace_speaker_assignments("r1", &t, &new_speakers, &new_emb, "titanet", &remap)
            .unwrap();

        let d = s.get_recording_detail("r1").unwrap().unwrap();
        // 話者は N1/N2、N2 が田中を引き継ぐ。
        let n2 = d.speakers.iter().find(|x| x.id == "N2").unwrap();
        assert_eq!(n2.display_name.as_deref(), Some("田中"));
        assert!(d.speakers.iter().find(|x| x.id == "N1").unwrap().display_name.is_none());
        // segment の speaker_id が更新されている。
        let ids: std::collections::BTreeSet<_> =
            d.transcript.segments.iter().filter_map(|x| x.speaker_id.clone()).collect();
        assert_eq!(ids, ["N1".to_string(), "N2".to_string()].into_iter().collect());
        // 既存要約は stale。
        assert!(d.summaries[0].stale);
        // 声紋も差し替わっている（旧 S1/S2 は消え N1/N2 に）。
        let embs: std::collections::BTreeSet<_> =
            s.get_speaker_embeddings("r1").unwrap().into_iter().map(|(id, _)| id).collect();
        assert_eq!(embs, ["N1".to_string(), "N2".to_string()].into_iter().collect());
    }

    /// Re-diarizing a meeting keeps the user's own library link; remote links are recomputed.
    #[test]
    fn replace_speaker_assignments_keeps_the_self_library_link() {
        let s = SqliteStore::open_in_memory().unwrap();
        let me = Speaker { id: crate::merge::SELF_SPEAKER_ID.into(), label: "あなた".into(), display_name: None };
        let guest = Speaker { id: "S1".into(), label: "相手1".into(), display_name: None };
        let mut t = transcript_with_speakers();
        t.segments[0].speaker_id = Some(me.id.clone());
        s.save_recording(&rec("r1"), &t, &[me.clone(), guest.clone()]).unwrap();
        s.add_library_speaker("p1", "本人").unwrap();
        s.add_library_speaker("p2", "相手").unwrap();
        s.link_speaker("r1", &me.id, "p1", 1.0).unwrap();
        s.link_speaker("r1", "S1", "p2", 1.0).unwrap();

        s.replace_speaker_assignments("r1", &t, &[me, guest], &[], "titanet", &[])
            .unwrap();

        let linked: Vec<String> = s
            .conn()
            .prepare("SELECT speaker_id FROM speaker_matches WHERE recording_id = 'r1'")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(linked, vec![crate::merge::SELF_SPEAKER_ID.to_string()]);
    }

    #[test]
    fn migrate_v4_to_v5_adds_jobs_and_stale_idempotent() {
        // v4 スキーマの「旧 DB」を用意し user_version=4 に固定 → migrate で v5 化。
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn.execute_batch(DDL).unwrap();
        conn.execute_batch(DDL_FTS).unwrap();
        conn.execute_batch(DDL_SPEAKERS).unwrap();
        conn.execute_batch(DDL_SPEAKER_LIBRARY).unwrap();
        conn.execute(
            "INSERT INTO recordings (id, source_type, title, duration_ms, sample_rate, language, created_at)
             VALUES ('r1','mic','旧',1000,16000,'ja','2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO summaries (recording_id, template_id, content) VALUES ('r1','minutes','本文')",
            [],
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 4i64).unwrap();

        migrate(&conn).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        // jobs テーブルが存在。
        assert!(column_exists(&conn, "jobs", "status").unwrap());
        // summaries.stale が追加され、既存要約は既定 0。
        assert!(column_exists(&conn, "summaries", "stale").unwrap());
        let stale: i64 = conn
            .query_row("SELECT stale FROM summaries WHERE recording_id='r1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stale, 0);
        // 非再入ガードの検証: user_version を 4 に戻して v5 ブロックを再走させても、
        // column_exists ガードで ALTER TABLE ADD COLUMN が二重に走らず duplicate column で落ちない。
        conn.pragma_update(None, "user_version", 4i64).unwrap();
        migrate(&conn).unwrap();
        assert!(column_exists(&conn, "summaries", "stale").unwrap());
    }

    // ---- 読み取り専用 open（MCP リーダー向け） ----

    /// 一時ファイル DB のパスを返し、既存の main/`-wal`/`-shm` を掃除する。
    fn tmp_db_path(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("mojiroku_ro_test_{name}.db"));
        for suf in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suf}", p.display()));
        }
        p
    }
    fn cleanup_db(p: &std::path::Path) {
        for suf in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suf}", p.display()));
        }
    }

    #[test]
    fn open_readonly_reads_wal_db_written_by_open() {
        // 通常 open（WAL・migrate 済み）で書いた DB を、別接続の open_readonly が読めること。
        // read-only WAL の `-shm`/`-wal` 落とし穴（OS read-write ハンドルで回避）の回帰。
        let path = tmp_db_path("roundtrip");
        {
            let s = SqliteStore::open(&path).unwrap();
            s.save_recording(&rec("r1"), &transcript(), &[]).unwrap();
        }
        let ro = SqliteStore::open_readonly(&path).unwrap();
        let list = ro.list_recordings().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "r1");
        cleanup_db(&path);
    }

    #[test]
    fn open_readonly_does_not_migrate_v2_db() {
        // user_version=2 の「旧 DB」をファイルに用意（speakers 表なし）。
        let path = tmp_db_path("v2_no_migrate");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(DDL).unwrap();
            conn.execute_batch(DDL_FTS).unwrap();
            conn.execute(
                "INSERT INTO recordings (id, source_type, title, duration_ms, sample_rate, language, created_at)
                 VALUES ('old','file','旧録音',1000,16000,'ja','2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO segments (recording_id, idx, start_ms, end_ms, text, speaker_id)
                 VALUES ('old',0,0,1000,'こんにちは',NULL)",
                [],
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 2i64).unwrap();
        }
        let s = SqliteStore::open_readonly(&path).unwrap();
        // 読み取りは成功（話者表なし → 空 Vec フォールバック）。
        let d = s.get_recording_detail("old").unwrap().unwrap();
        assert!(d.speakers.is_empty());
        assert_eq!(d.transcript.segments.len(), 1);
        // 重要: open_readonly は migrate せず user_version を据え置く。
        let v: i64 = s
            .conn
            .lock()
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 2, "open_readonly はスキーマ/バージョンを一切変えない");
        cleanup_db(&path);
    }

    #[test]
    fn open_readonly_rejects_writes() {
        // read-only は規約でなく接続レベルで強制される（PRAGMA query_only）。
        // 将来 MCP にツールを足すとき書き込みが誤って滑り込んでも Err になる。
        let path = tmp_db_path("ro-write");
        {
            let s = SqliteStore::open(&path).unwrap();
            drop(s);
        }
        let s = SqliteStore::open_readonly(&path).unwrap();
        let conn = s.conn();
        let err = conn.execute(
            "INSERT INTO speaker_library (id, name) VALUES ('x', 'y')",
            [],
        );
        assert!(err.is_err(), "query_only 接続からの書き込みは拒否される");
        cleanup_db(&path);
    }

    #[test]
    fn open_readonly_rejects_newer_schema() {
        // 将来バージョンの DB は誤読を避けて Err。
        let path = tmp_db_path("newer");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(DDL).unwrap();
            conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
                .unwrap();
        }
        assert!(
            SqliteStore::open_readonly(&path).is_err(),
            "未来スキーマは Err（mojiroku-mcp 更新を促す）"
        );
        cleanup_db(&path);
    }

    // ---- 全文検索（FTS5） ----

    /// 3 文字以上の日本語を含む transcript（trigram の最小長 3 を満たす）。
    fn transcript_jp() -> Transcript {
        Transcript {
            language: Some("ja".into()),
            segments: vec![
                Segment { idx: 0, start_ms: 0, end_ms: 1000, text: "今日の会議の議題".into(), speaker_id: None },
                Segment { idx: 0, start_ms: 1000, end_ms: 2000, text: "来期の予算について話す".into(), speaker_id: None },
            ],
        }
    }

    fn rec_titled(id: &str, title: Option<&str>) -> Recording {
        Recording {
            id: id.to_string(),
            source_type: SourceType::File,
            title: title.map(|t| t.to_string()),
            duration_ms: 1000,
            sample_rate: 16000,
            created_at: "2026-06-24T10:00:00Z".into(),
        }
    }

    /// 検証ファースト: bundled rusqlite に FTS5/trigram が含まれること自体をテストで担保する。
    #[test]
    fn fts5_trigram_is_available() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE VIRTUAL TABLE t USING fts5(x, tokenize='trigram');")
            .expect("bundled SQLite must support FTS5 trigram");
    }

    #[test]
    fn search_japanese_substring_fts_and_like() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.save_recording(&rec_titled("r1", Some("定例会")), &transcript_jp(), &[]).unwrap();
        // 3 文字（FTS 経路）
        let hits = s.search_recordings("会議の").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].recording.id, "r1");
        // snippet にマッチ語がハイライト区切り [..] 付きで含まれる。
        assert!(hits[0].snippet.contains("[会議の]"), "snippet = {:?}", hits[0].snippet);
        // 2 文字（LIKE フォールバック）
        let hits = s.search_recordings("会議").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].recording.id, "r1");
    }

    #[test]
    fn search_dedupes_per_recording() {
        let s = SqliteStore::open_in_memory().unwrap();
        // "予算" を複数 segment にまたがって含む録音。
        let t = Transcript {
            language: Some("ja".into()),
            segments: vec![
                Segment { idx: 0, start_ms: 0, end_ms: 1000, text: "予算の確認".into(), speaker_id: None },
                Segment { idx: 0, start_ms: 1000, end_ms: 2000, text: "予算の承認".into(), speaker_id: None },
            ],
        };
        s.save_recording(&rec_titled("r1", None), &t, &[]).unwrap();
        let hits = s.search_recordings("予算の").unwrap();
        assert_eq!(hits.len(), 1, "1 録音 = 1 行なので重複しない");
    }

    #[test]
    fn search_matches_title_only() {
        let s = SqliteStore::open_in_memory().unwrap();
        // 本文には "営業" を含まず、タイトルにだけ含む。
        let t = Transcript {
            language: Some("ja".into()),
            segments: vec![Segment {
                idx: 0,
                start_ms: 0,
                end_ms: 1000,
                text: "雑談のみ".into(),
                speaker_id: None,
            }],
        };
        s.save_recording(&rec_titled("r1", Some("営業ミーティング")), &t, &[]).unwrap();
        let hits = s.search_recordings("営業ミ").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].recording.id, "r1");
    }

    #[test]
    fn search_filters_across_recordings() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.save_recording(&rec_titled("r1", None), &transcript_jp(), &[]).unwrap();
        let other = Transcript {
            language: Some("ja".into()),
            segments: vec![Segment {
                idx: 0,
                start_ms: 0,
                end_ms: 1000,
                text: "週末の買い物リスト".into(),
                speaker_id: None,
            }],
        };
        s.save_recording(&rec_titled("r2", None), &other, &[]).unwrap();
        let hits = s.search_recordings("会議の").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].recording.id, "r1");
    }

    #[test]
    fn search_gone_after_delete() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.save_recording(&rec_titled("r1", None), &transcript_jp(), &[]).unwrap();
        assert_eq!(s.search_recordings("会議の").unwrap().len(), 1);
        s.delete_recording("r1").unwrap();
        assert_eq!(s.search_recordings("会議の").unwrap().len(), 0);
        // rec_fts も 0 件（同期が効いている証明）。
        let conn = s.conn.lock().unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM rec_fts", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn search_empty_query_is_empty() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.save_recording(&rec_titled("r1", None), &transcript_jp(), &[]).unwrap();
        assert!(s.search_recordings("").unwrap().is_empty());
        assert!(s.search_recordings("   ").unwrap().is_empty());
    }

    #[test]
    fn search_escapes_match_operators() {
        let s = SqliteStore::open_in_memory().unwrap();
        let t = Transcript {
            language: Some("ja".into()),
            segments: vec![Segment {
                idx: 0,
                start_ms: 0,
                end_ms: 1000,
                text: r#"foo AND "bar" baz"#.into(),
                speaker_id: None,
            }],
        };
        s.save_recording(&rec_titled("r1", None), &t, &[]).unwrap();
        // 演算子/引用符を含むクエリでも構文エラーにならずフレーズ一致する。
        let hits = s.search_recordings(r#"AND "bar""#).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].recording.id, "r1");
    }

    /// 既存ユーザー（user_version=1, データあり）の v1→v2 アップグレード経路。
    /// backfill はこの遷移で 1 度だけ走る。ここが壊れると旧録音が検索から永久に欠落する
    /// （新録音は save_recording で索引されるので破綻に見えにくい）。最重要の経路。
    #[test]
    fn migrate_v1_to_v2_backfills_existing_data() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn.execute_batch(DDL).unwrap(); // v1 スキーマ（rec_fts なし）
        conn.pragma_update(None, "user_version", 1).unwrap(); // 既存ユーザーを模す
        conn.execute(
            "INSERT INTO recordings (id, source_type, title, duration_ms, sample_rate, language, created_at)
             VALUES ('r1', 'file', '会議メモ', 1000, 16000, 'ja', '2026-06-24T10:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO segments (recording_id, idx, start_ms, end_ms, text, speaker_id)
             VALUES ('r1', 0, 0, 1000, '今日の会議の議題', NULL)",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap(); // 実アップグレード分岐

        // migrate は最終バージョンまで一気に上げる（v1→…→現行）。backfill が走ったかは下の MATCH で見る。
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM rec_fts WHERE rec_fts MATCH '\"会議の\"'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "旧録音が backfill で検索可能になること");
    }

    #[test]
    fn backfill_reconstructs_fts() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.save_recording(&rec_titled("r1", Some("会議メモ")), &transcript_jp(), &[]).unwrap();
        let conn = s.conn.lock().unwrap();
        // rec_fts を一旦空にしてから backfill で再構築 → 検索ヒット。
        conn.execute("DELETE FROM rec_fts", []).unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM rec_fts", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
        backfill_fts(&conn).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM rec_fts WHERE rec_fts MATCH '\"会議の\"'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn recording_rows_carry_state_for_the_history_list() {
        let s = SqliteStore::open_in_memory().unwrap();
        // 文字起こし済み + 要約 1 件
        let mut done = rec("done");
        done.created_at = "2026-06-24T12:00:00Z".into();
        s.save_recording(&done, &transcript(), &[]).unwrap();
        s.save_summary("done", &summary("minutes", vec![])).unwrap();
        // 音声だけ保存（ジョブなし）
        let mut bare = rec("bare");
        bare.created_at = "2026-06-24T11:00:00Z".into();
        s.insert_recording_only(&bare).unwrap();
        // 失敗したジョブのあとに再投入して処理中
        let mut retry = rec("retry");
        retry.created_at = "2026-06-24T10:00:00Z".into();
        s.insert_recording_only(&retry).unwrap();
        let p = JobParams { diarize: false, stt_lang: None, transcription_model: String::new(), lang: "ja".into() };
        s.enqueue_job("j1", "retry", "transcribe", &p).unwrap();
        s.set_job_failed("j1", "error.job.no_audio").unwrap();
        s.enqueue_job("j2", "retry", "transcribe", &p).unwrap();
        // 失敗したまま
        let mut failed = rec("failed");
        failed.created_at = "2026-06-24T09:00:00Z".into();
        s.insert_recording_only(&failed).unwrap();
        s.enqueue_job("j3", "failed", "diarize", &p).unwrap();
        s.set_job_failed("j3", "error.job.no_speakers_found").unwrap();
        // タイトル生成（Issue #4）は録音の状態に数えない。
        s.enqueue_job("j4", "done", TITLE_JOB_KIND, &p).unwrap();
        s.enqueue_job("j5", "failed", TITLE_JOB_KIND, &p).unwrap();

        let rows = s.list_recording_rows().unwrap();
        let ids: Vec<_> = rows.iter().map(|r| r.recording.id.as_str()).collect();
        assert_eq!(ids, ["done", "bare", "retry", "failed"], "newest first");
        assert_eq!((rows[0].segment_count, rows[0].summary_count), (3, 1));
        assert_eq!(rows[0].latest_job, None);
        assert_eq!((rows[1].segment_count, rows[1].latest_job.clone()), (0, None));
        let latest = rows[2].latest_job.as_ref().unwrap();
        assert_eq!(latest.status, "pending", "the retry, not the earlier failure");
        let failed = rows[3].latest_job.as_ref().unwrap();
        assert_eq!((failed.kind.as_str(), failed.status.as_str()), ("diarize", "failed"));
        assert_eq!(failed.error.as_deref(), Some("error.job.no_speakers_found"));
    }

}
