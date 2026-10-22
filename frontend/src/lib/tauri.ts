import type { SavedLiveTranslation } from "./liveTranslation";
// Tauri コマンド/イベントの型付きラッパー。UI からはここだけを経由する。
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef } from "react";
import type {
  CalendarEvent,
  Job,
  JobUpdate,
  NotionPage,
  Progress,
  Recording,
  RecordingDetail,
  RecordingRow,
  SearchHit,
  Settings,
  StartJobResult,
  StartingMeeting,
  Summary,
  Transcript,
} from "./types";

// ── コマンド ───────────────────────────────────────────────────────────

export const health = () => invoke<string>("health");

/** 設定画面に出す「この端末が使う要約モデル」。端末のメモリで変わる（ADR-0030）ので、
 *  UI に固定文字列で書かず必ずここから引く。 */
export type SummaryModelInfo = {
  /**
   * What automatic picks (model already on disk first, then tier). Runs when
   * `settings.local_summary_model` is "" and is shown the moment the user switches back
   * to automatic. A full entry because auto may resolve to a model not in `choices`.
   */
  auto: SummaryModelChoice;
  /** Switch targets: adopted models only, in ascending tier order. */
  choices: SummaryModelChoice[];
};
export type SummaryModelChoice = {
  file: string;
  label: string;
  size: string;
  downloaded: boolean;
  tier: "small" | "medium" | "large";
  /** Above this Mac's tier. Still selectable, but the UI attaches a warning (Issue #30). */
  exceeds_tier: boolean;
};
export const summaryModelInfo = () => invoke<SummaryModelInfo>("summary_model_info");

export type TranscriptionModelInfo = {
  default_file: string;
  live_ready: boolean;
  choices: { file: string; label: string; size: string; downloaded: boolean }[];
};
export const transcriptionModelInfo = () => invoke<TranscriptionModelInfo>("transcription_model_info");
export const downloadLiveTranscriptionModels = () => invoke<void>("download_live_transcription_models");

/**
 * 音声ファイル → 原本コピー確定 → 文字起こしジョブを投入して即返す（ADR-0024）。
 * diarize で話者分離。STT はワーカーが回し、進捗は `job://update` で届く。
 * recordOnly=true（音声だけ保存）なら録音行だけ作りジョブは積まない（後で文字起こし）。
 */
export const transcribeFile = (path: string, diarize: boolean, recordOnly = false) =>
  invoke<StartJobResult>("transcribe_file", { path, diarize, recordOnly });

/** マイク録音開始（cpal, default device）。 */
export const startMicRecording = () => invoke<void>("start_mic_recording");
/** マイク録音を保存せずに止める（#113）。 */
export const cancelMicRecording = () => invoke<void>("cancel_mic_recording");

/**
 * マイク録音停止 → WAV 保存確定 → 文字起こしジョブを投入して即返す（ADR-0024）。diarize で話者分離。
 * title は「記録を準備」（カレンダー連携）由来の予定タイトル。未指定/空なら既定の「録音」。
 * recordOnly=true（音声だけ保存）なら録音行だけ作りジョブは積まない（後で文字起こし）。
 */
export const stopMicRecording = (
  diarize: boolean,
  title?: string | null,
  recordOnly = false,
) =>
  invoke<StartJobResult>("stop_mic_recording", {
    diarize,
    title: title ?? null,
    recordOnly,
  });

/** 要約生成（sidecar mojiroku-llm or BYOK）。 */
export const summarize = (
  transcript: Transcript,
  recordingId: string,
  templateId: string,
) => invoke<Summary>("summarize", { transcript, recordingId, templateId });

/** 履歴一覧（created_at 降順）。 */
export const listRecordings = () => invoke<Recording[]>("list_recordings");

/** 履歴一覧（状態つき・#109）。Rust 側 store::RecordingRow に対応。 */
export const listRecordingRows = () => invoke<RecordingRow[]>("list_recording_rows");

/** FTS5 全文検索（title + 本文）。 */
export const searchRecordings = (query: string) =>
  invoke<SearchHit[]>("search_recordings", { query });

/** 詳細取得（Recording + Transcript + 全要約 + 話者）。削除済みは null。 */
export const getRecording = (id: string) =>
  invoke<RecordingDetail | null>("get_recording", { id });

/** 録音削除（FK CASCADE で関連削除・録音原本も消える）。 */
export const deleteRecording = (id: string) => invoke<void>("delete_recording", { id });

// ── バックグラウンドジョブ（ADR-0024） ─────────────────────────────────────

/**
 * 既存録音を（再）文字起こしするジョブを投入する（即返し）。diarize で話者分離も含める。
 * 実処理はワーカーが 1 本ずつ直列で行い、進捗は job://update（useJobUpdate）で届く。
 */
export const transcribeRecording = (recordingId: string, diarize: boolean) =>
  invoke<StartJobResult>("transcribe_recording", { recordingId, diarize });

/**
 * 既存録音に後から話者分離を掛けるジョブを投入する（即返し）。文字起こし済みが前提。
 * 会議（Live）は取得時に話者付与済みのため拒否される（error.job.already_diarized）。
 */
export const diarizeRecording = (recordingId: string) =>
  invoke<StartJobResult>("diarize_recording", { recordingId });

/** 進行中・要注意なジョブ一覧（pending/running/failed）。バッジ・処理中判定に使う。 */
export const listJobs = () => invoke<Job[]>("list_jobs");

/** pending ジョブをキャンセル（running は完走）。キャンセルできたら true。 */
export const cancelJob = (jobId: string) => invoke<boolean>("cancel_job", { jobId });

/** 録音タイトル変更（null/空白で既定の「無題」へ戻す）。全文検索も同期される。 */
/** 文字起こしからタイトルを生成して保存する（Issue #4）。要約と同じエンジン設定に従う。 */
export const generateTitle = (recordingId: string) =>
  invoke<string>("generate_title", { recordingId });

export const renameRecording = (id: string, title: string | null) =>
  invoke<void>("rename_recording", { id, title: title?.trim() || null });

/**
 * 録音の再生用音声 URL（asset://…）。原本が無ければ null。
 * バックエンドが recordings/<id>.<ext>（会議は結合ミックス <id>.wav）の絶対パスを返し、
 * convertFileSrc で webview が読めるアセット URL 化する（assetProtocol scope は setup で許可済み）。
 */
export const recordingAudioSrc = async (id: string): Promise<string | null> => {
  const path = await invoke<string | null>("recording_audio_src", { id });
  return path ? convertFileSrc(path) : null;
};

/**
 * 発言 1 件の話者を差し替える（発言単位の手動訂正・Issue #19）。
 * `segmentIdx` は Segment.idx。`speakerId` が null なら「話者不明」に戻す。
 * 改名（renameSpeaker）がクラスタ全体を変えるのに対し、こちらは 1 発言だけを動かす。
 *
 * **戻り値は「実際に変えたか」。** 同じ話者を選び直したときは false。
 * 呼び出し側はこれを見て「要約が古い」の表示を出し分ける（UI 側で現在値と比較しない）。
 */
export const setSegmentSpeaker = (
  recordingId: string,
  segmentIdx: number,
  speakerId: string | null,
) =>
  invoke<boolean>("set_segment_speaker", {
    recordingId,
    segmentIdx,
    speakerId,
  });

/** 話者改名（null で既定ラベルに戻す）。クラスタ全体が対象。 */
export const renameSpeaker = (
  recordingId: string,
  speakerId: string,
  displayName: string | null,
) => invoke<void>("rename_speaker", { recordingId, speakerId, displayName });

// ── 話者ライブラリ（クロス会議の声紋照合・ADR-0018） ──────────────────────

/** 端末内の登録話者（人物）。identified_count は対応づけ済み録音話者数。 */
export interface LibrarySpeaker {
  id: string;
  name: string;
  identified_count: number;
}

/** 録音話者をライブラリへ照合した結果（サジェスト先行）。confidence/margin で UI が判断。 */
export interface SpeakerMatchSuggestion {
  speaker_id: string;
  linked_library_id: string | null;
  top_library_id: string | null;
  top_name: string | null;
  confidence: number | null;
  margin: number | null;
  below_enroll_gate: boolean;
}

/** 登録話者の一覧（名前昇順・対応づけ数つき）。 */
export const listSpeakerLibrary = () =>
  invoke<LibrarySpeaker[]>("list_speaker_library");

/** ライブラリに人物を新規登録し、採番 id を返す。 */
export const addSpeakerToLibrary = (name: string) =>
  invoke<string>("add_speaker_to_library", { name });

/** 登録話者の改名。 */
export const renameSpeakerLibrary = (id: string, name: string) =>
  invoke<void>("rename_speaker_library", { id, name });

/** 登録話者の削除（対応づけも CASCADE で消える）。 */
export const deleteSpeakerLibrary = (id: string) =>
  invoke<void>("delete_speaker_library", { id });

/** 録音の各話者をライブラリへ 1:N 照合（サジェスト先行）。 */
export const identifySpeakers = (recordingId: string) =>
  invoke<SpeakerMatchSuggestion[]>("identify_speakers", { recordingId });

/** 録音話者をライブラリ人物へ対応づけ（サジェスト採用・確定）。 */
export const linkSpeaker = (
  recordingId: string,
  speakerId: string,
  libraryId: string,
  confidence: number,
) => invoke<void>("link_speaker", { recordingId, speakerId, libraryId, confidence });

/** 録音話者の対応づけを解除。 */
export const unlinkSpeaker = (recordingId: string, speakerId: string) =>
  invoke<void>("unlink_speaker", { recordingId, speakerId });

// ── 設定 / シークレット ─────────────────────────────────────────────────

/** アプリ設定を読む（無ければ既定）。シークレットは含まない。 */
export const getSettings = () => invoke<Settings>("get_settings");

/** アプリ設定を保存（settings.json）。 */
export const setSettings = (settings: Settings) =>
  invoke<void>("set_settings", { settings });

/** シークレット（API キー等）をキーチェーンへ保存。値は保存専用で読み戻さない。 */
export const setSecret = (name: string, value: string) =>
  invoke<void>("set_secret", { name, value });

/** シークレットを削除（未登録でも成功）。 */
export const deleteSecret = (name: string) => invoke<void>("delete_secret", { name });

/** シークレットが保存済みか（値は返らない。バッジ表示用）。 */
export const hasSecret = (name: string) => invoke<boolean>("has_secret", { name });

/**
 * テキストをファイルへ書き出す。保存ダイアログは Rust 側が開き、書き込み先は
 * 必ずユーザー選択パスに限定される。戻り値 false はキャンセル（トースト不要）。
 */
export const exportTextFile = (
  defaultName: string,
  ext: string,
  filterName: string,
  content: string,
) => invoke<boolean>("export_text_file", { defaultName, ext, filterName, content });

/**
 * Notion へ議事録ページを送信（要約 + 文字起こし）。戻り値は作成された Notion ページ URL。
 * ⚠️ データが Notion のサーバへ送られる（ローカル要約でも送信）。ユーザー操作起点のみで呼ぶ。
 */
export const exportToNotion = (recordingId: string) =>
  invoke<string>("export_to_notion", { recordingId });

/**
 * Notion 連携トークンでアクセスできる書き出し先候補ページを返す（OAuth 同意で共有したページ）。
 * 「書き出し先ページ」ドロップダウンに使う。トークン自体は返らない。未連携なら誘導エラー。
 */
export const notionAccessiblePages = () =>
  invoke<NotionPage[]>("notion_accessible_pages");

/**
 * Slack の設定チャンネルへ要約を投稿（Incoming Webhook）。文字起こしは送らない。
 * ⚠️ 要約が Slack のサーバへ送られる（ローカル要約でも送信）。ユーザー操作起点のみで呼ぶ。
 */
export const exportToSlack = (recordingId: string) =>
  invoke<void>("export_to_slack", { recordingId });

/**
 * 限定公開 iCal URL（キーチェーン保管）から直近の予定を取り込む（読み取り専用・$0・OAuth 不要）。
 * 未設定ならコマンドが誘導エラーを返す。表示時にこちらから GET するだけで、外部送信は無い。
 */
export const listCalendarEvents = () => invoke<CalendarEvent[]>("list_calendar_events");

/** 直近に「今始まった」と判定された会議（あれば録音プロンプトを出す・ADR-0026）。無ければ null。 */
export const getPendingMeeting = () =>
  invoke<StartingMeeting | null>("get_pending_meeting");

/** 保留中の会議プロンプトを消す（録音開始 or 却下の後に呼ぶ）。 */
export const clearPendingMeeting = () => invoke<void>("clear_pending_meeting");

/**
 * 会議モードの録音に付ける予定タイトルを解決する。まだ進行中の予定があればその題名、
 * 無ければ null（呼び出し側は既定の「会議」にフォールバックする）。
 *
 * ⚠️ 「まだ進行中か」の判定は Rust 側にしかない。フロントで start / end を比べないこと
 * （scheduler.rs の窓と二重定義になっていずれズレる）。
 */
export const resolveMeetingTitle = () =>
  invoke<string | null>("resolve_meeting_title");

/**
 * 外部サービスと OAuth 連携する（loopback 受け口・$0）。既定ブラウザで同意ページが開き、許可すると
 * トークン/Webhook がこの Mac のキーチェーンに保存される。
 * - "slack" / "notion": mojiroku.com の Worker ブローカー経由（Worker が client_secret を保持し token 交換）。
 * - "google": loopback + PKCE 直接フロー（Desktop 型・Worker 不要）。
 * 完了/失敗まで解決しない（同意が終わるまで待つ）ので、呼び出し側で busy 表示を出すこと。
 */
export const oauthConnect = (provider: "slack" | "google" | "notion") =>
  invoke<void>("oauth_connect", { provider });

// ── 会議モード（システム音声 / ADR-0017） ──────────────────────────────────

/**
 * 会議モード: システム音声収録（「画面とシステムオーディオの収録」TCC）の許可状態。true=許可。
 * 起動時プリフライト・更新後の失効検出に使う。注意: 許可があっても録れているかは別途 peak_rms で確認。
 */
export const checkSystemAudioPermission = () =>
  invoke<boolean>("check_system_audio_permission");

/**
 * 会議モード: 会議録音開始（マイク＝自分 ＋ システム音声＝相手 を同時にキャプチャ）。
 * システム音声の許可が無ければ誘導エラーを返す。
 */
export const startMeetingRecording = () => invoke<void>("start_meeting_recording");

/** 会議モード: 会議録音を破棄停止（文字起こし/保存しない）。会議画面からの離脱時の解放用。 */
export const cancelMeetingRecording = () => invoke<void>("cancel_meeting_recording");

/**
 * 会議モード: 会議録音停止 → 両トラックを WAV 保存 → デュアルトラック文字起こし → Live 保存。
 * system（相手）は STT＋話者分離、mic（自分）は STT のみ、ソースで合成（mic=あなた）。
 * title はカレンダー由来の予定タイトル。両トラック無音ならコマンドが誘導エラーを返す。
 */
export const stopMeetingRecording = (title?: string | null, translations: SavedLiveTranslation[] = []) =>
  invoke<StartJobResult>("stop_meeting_recording", { title: title ?? null, translations });

export const listLiveTranslations = (id: string) =>
  invoke<SavedLiveTranslation[]>("list_live_translations", { id });

// ── イベント ───────────────────────────────────────────────────────────

/**
 * Tauri イベントを購読する汎用フック。handler は ref 経由で呼ぶので、
 * インラインのクロージャを渡しても再購読は起きない（マウント中は 1 回だけ listen）。
 */
export function useTauriEvent<T>(event: string, handler: (payload: T) => void) {
  const ref = useRef(handler);
  ref.current = handler;
  useEffect(() => {
    let active = true;
    const unlistenP = listen<T>(event, (e) => {
      if (active) ref.current(e.payload);
    });
    return () => {
      active = false;
      unlistenP.then((un) => un());
    };
  }, [event]);
}

/** 要約進捗（download_llm / summarize）。 */
export const useSummarizeProgress = (handler: (p: Progress) => void) =>
  useTauriEvent<Progress>("summarize://progress", handler);

/**
 * バックグラウンドジョブの更新（ADR-0024）。ライフサイクル遷移（status）も stage 進捗も
 * 1 ペイロードで届く。**payload.recording_id / job_id で自分宛だけ描く**こと（複数録音の
 * 進捗が混ざらないよう相関付け必須）。
 */
export const useJobUpdate = (handler: (u: JobUpdate) => void) =>
  useTauriEvent<JobUpdate>("job://update", handler);

/** ライブ文字起こしの1行。committed=確定（以後不変）、false=未確定 tail（書き換わりうる）。 */
export interface LiveLine {
  id: number;
  text: string;
  committed: boolean;
}

/**
 * 会議モードのライブ文字起こし（増分C）。ワーカーが mic＋system の直近音声を周期的に起こして
 * 現在の表示行一式を送る。**使い捨てプレビュー**で、保存される文字起こしは停止時のデュアル
 * トラック結果が権威。payload.lines は確定行＋未確定 tail の現在ビュー全体。
 */
export interface LiveSnapshot {
  session_id: string;
  lines: LiveLine[];
}

export const useMeetingLiveSnapshot = (handler: (snapshot: LiveSnapshot) => void) =>
  useTauriEvent<LiveSnapshot>("meeting://live", handler);

export const useMeetingLive = (handler: (lines: LiveLine[]) => void) =>
  useMeetingLiveSnapshot((p) => handler(p.lines));

/**
 * 会議開始スケジューラの発火（ADR-0026）。予定の開始時刻にバックエンドが発行する。
 * ウィンドウが閉じていて取りこぼした分は getPendingMeeting で初期化時に拾う。
 */
export const useMeetingStarting = (handler: (m: StartingMeeting) => void) =>
  useTauriEvent<StartingMeeting>("meeting://starting", handler);
