// mojiroku-core の schemas / Tauri コマンド戻り値に対応する型と、表示ヘルパー。
// （旧 features/transcription/types.ts を lib に昇格・ダークテーマ化したもの）
import { dicts, type Locale } from "@/i18n";

export interface Segment {
  /**
   * DB 上の並び順。**1 発言を指す識別子**（発言単位の話者訂正がこれで対象を指す）。
   * 読み出し時は配列の添字と一致するが、依存せずこの値を使うこと。
   */
  idx: number;
  start_ms: number;
  end_ms: number;
  text: string;
  speaker_id: string | null;
}

export interface Transcript {
  language: string | null;
  segments: Segment[];
}

/** 話者。Rust 側 schemas::Speaker に対応。 */
export interface Speaker {
  id: string;
  label: string;
  display_name: string | null;
}

export interface ActionItem {
  text: string;
  assignee: string | null;
  due: string | null;
}

export interface Summary {
  template_id: string;
  content: string;
  action_items: ActionItem[];
  /** 元の文字起こし/話者が後から更新され、この要約が古くなったか（後付け話者分離で立つ・ADR-0024）。
   *  旧クライアント/旧データ互換のため optional（未定義は false 扱い）。 */
  stale?: boolean;
}

// ── バックグラウンドジョブ（ADR-0024） ─────────────────────────────────────

/** ジョブ実行時パラメータ（enqueue 時のスナップショット）。Rust 側 store::JobParams に対応。 */
export interface JobParams {
  diarize: boolean;
  stt_lang: string | null;
  transcription_model: string;
  lang: string;
}

/** 重い処理ジョブ（文字起こし / 後付け話者分離）。Rust 側 store::Job に対応。 */
export interface Job {
  id: string;
  recording_id: string;
  /** "transcribe" | "diarize"。 */
  kind: string;
  /** "pending" | "running" | "done" | "failed" | "canceled"。 */
  status: string;
  params: JobParams;
  /** 直近の処理ステージ（decode/transcribe/diarization/merge/queued 等・表示用）。 */
  stage: string | null;
  /** 失敗時のキー化メッセージ（translateError 対象）。 */
  error: string | null;
  created_at: string;
  updated_at: string;
}

/** 録音停止・ファイル取込・ジョブ投入系コマンドの戻り。Rust 側 StartJobResult に対応。 */
export interface StartJobResult {
  recording_id: string;
  /** 投入したジョブ id。録音のみ保存（音声だけ保存）ではジョブを作らないので null。 */
  job_id: string | null;
}

/** 文字起こし後のタイトル自動生成ジョブ（Issue #4）。裏方なので通知や処理中の表示に出さない。 */
export const TITLE_JOB_KIND = "title";

/** job://update イベントのペイロード。Rust 側 JobUpdate に対応（相関付けに job_id/recording_id）。 */
export interface JobUpdate {
  job_id: string;
  recording_id: string;
  kind: string;
  status: string;
  stage: string | null;
  done: number;
  total: number | null;
  error: string | null;
}

export interface Progress {
  stage:
    | "download"
    | "decode"
    | "transcribe"
    | "diarization"
    | "merge"
    | "download_llm"
    | "summarize"
    | string;
  done: number;
  total: number | null;
}

export type SourceType = "file" | "mic" | "live";

/** 録音/セッション（履歴のルート）。Rust 側 schemas::Recording に対応。 */
export interface Recording {
  id: string;
  source_type: SourceType;
  title: string | null;
  duration_ms: number;
  sample_rate: number;
  created_at: string;
}

/** 履歴詳細。Rust 側 store::RecordingDetail に対応。 */
export interface RecordingDetail {
  recording: Recording;
  transcript: Transcript;
  summaries: Summary[];
  /** 話者分離を行った録音のみ非空。旧録音は空配列 → speakerLabelFromId にフォールバック
   *（Rust は #[serde(default)] Vec で常に配列を返す＝undefined にはならない）。 */
  speakers: Speaker[];
  /** 進行中（pending|running）のジョブ（あれば）。詳細ビューを「処理中」で開くための同梱（ADR-0024）。
   *  未処理/完了のみなら null。旧 DB 互換のため optional。 */
  active_job?: Job | null;
}

/** 履歴一覧の 1 行（#109）。Rust 側 store::RecordingRow に対応。 */
export interface RecordingRow {
  recording: Recording;
  segment_count: number;
  speaker_count: number;
  summary_count: number;
  latest_job: { kind: string; status: string; error: string | null } | null;
}

/** 一覧に出す録音の状態（#109）。処理中 > 失敗 > 未文字起こし > 要約済み > 文字起こし済み の順で 1 つ。 */
export type RecordingState =
  | "processing"
  | "failed"
  | "untranscribed"
  | "noSpeech"
  | "summarized"
  | "transcribed";

export function recordingState(row: RecordingRow): RecordingState {
  const job = row.latest_job;
  if (job && (job.status === "pending" || job.status === "running")) return "processing";
  if (job && job.status === "failed") return "failed";
  if (row.segment_count === 0) {
    // 文字起こしは終わったが発話が無かった（VAD が無音と判定すると空の文字起こしを保存する・ADR-0031）。
    return job && job.kind === "transcribe" && job.status === "done" ? "noSpeech" : "untranscribed";
  }
  return row.summary_count > 0 ? "summarized" : "transcribed";
}

/** 全文検索の 1 ヒット。Rust 側 store::SearchHit に対応。 */
export interface SearchHit {
  recording: Recording;
  snippet: string;
}

/**
 * 永続化されるアプリ設定。Rust 側 `settings::Settings` に対応（フィールド名は snake_case で一致）。
 * シークレット（API キー）は含まない — それはキーチェーン（set_secret/has_secret）管轄。
 */
export interface Settings {
  /** "local"（同梱モデル, 既定） | "cloud"（BYOK）。 */
  engine: "local" | "cloud";
  /** "anthropic" | "openai"（cloud のとき有効）。 */
  provider: "anthropic" | "openai";
  /** モデル名の上書き。空なら provider 既定。 */
  model: string;
  /** 録音原本を保存するか（既定 ON）。 */
  save_recordings: boolean;
  /** 匿名の使用状況送信（既定 OFF）。 */
  send_usage: boolean;
  /** Notion 連携の親ページ ID または URL（空なら未設定）。トークンはキーチェーン管轄。 */
  notion_parent_id: string;
  /** アプリ言語（UI・要約出力・話者ラベル・エクスポート見出し）。"" = 未設定（初回起動で解決）。 */
  language: "" | "ja" | "en";
  /**
   * 文字起こし言語。"" = アプリ言語に追従（既定）、"auto" = whisper 自動判定。
   * Issue #66 supersedes the rule above: "" is a legacy value that now behaves like "auto".
   * "mixed" re-detects language at speech pauses and is explicitly opt-in.
   */
  transcribe_language: "" | "auto" | "ja" | "en" | "mixed";
  /** 会議開始時に録音を促す通知を出すか（既定 OFF＝オプトイン・ADR-0026）。カレンダー連携が前提。 */
  auto_record_prompt: boolean;
  /**
   * Explicit local summary model (catalog file name). "" = automatic, chosen from the
   * Mac's memory and the models already on disk (ADR-0030). Distinct from the BYOK `model`.
   */
  local_summary_model: string;
  /** Offline Whisper model. Empty/unknown = turbo; live always uses turbo. */
  transcription_model: string;
}

/**
 * BYOK API キーのキーチェーン account 名（provider 別にスロットを分ける）。
 * Rust 側 secrets::byok_key_name と一致させる。別 provider の鍵を取り違えて送らないため。
 */
export const byokKeyName = (provider: Settings["provider"]) => `byok_api_key_${provider}`;

/**
 * Notion 連携トークンのキーチェーン account 名（Rust 側 NOTION_TOKEN_KEY と一致）。
 * 値（トークン）は JS へ読み戻さない — set/has/delete のみで「保存済みか」だけを扱う。
 */
export const NOTION_TOKEN_KEY = "notion_token";

/**
 * Notion の書き出し先候補ページ（Rust 側 export::NotionPage に対応）。
 * OAuth 連携で共有を許可したページが返り、選択した id を settings の notion_parent_id に保存する。
 */
export interface NotionPage {
  id: string;
  title: string;
}

/**
 * Slack Incoming Webhook URL のキーチェーン account 名（Rust 側 SLACK_WEBHOOK_KEY と一致）。
 * webhook URL 自体が秘密。値は JS へ読み戻さない — set/has/delete のみ。
 * チャンネルは URL に内包されるため settings に別フィールドは持たない。
 */
export const SLACK_WEBHOOK_KEY = "slack_webhook_url";

/**
 * 限定公開 iCal URL のキーチェーン account 名（Rust 側 CALENDAR_ICAL_KEY と一致）。
 * URL 自体が秘密（カレンダー読み取りのクレデンシャル）。値は JS へ読み戻さない — set/has/delete のみ。
 * 旧方式。新規連携は OAuth（下記 GOOGLE_*）。既存ユーザー互換のため残す。
 */
export const CALENDAR_ICAL_KEY = "calendar_ical_url";

/**
 * Google OAuth トークンのキーチェーン account 名（Rust 側 oauth.rs の GOOGLE_*_KEY と一致）。
 * 値（トークン）は JS へ読み戻さない — has/delete のみで「連携済みか」だけを扱う。
 */
export const GOOGLE_OAUTH_ACCESS_KEY = "google_oauth_access";
export const GOOGLE_OAUTH_REFRESH_KEY = "google_oauth_refresh";
export const GOOGLE_TOKEN_EXPIRY_KEY = "google_token_expiry";

/**
 * カレンダーの予定 1 件（Rust 側 calendar::CalendarEvent と対応）。繰り返しは発生ごとに 1 件。
 * start/end はローカル壁時計の "YYYY-MM-DDTHH:MM:SS"（オフセットなし＝`new Date` がローカル解釈）。
 */
export interface CalendarEvent {
  id: string;
  title: string;
  start: string;
  end: string | null;
  location: string | null;
}

/**
 * スケジューラが「今始まった」と判定した会議（Rust 側 scheduler::StartingMeeting と対応・ADR-0026）。
 * meeting://starting イベントと get_pending_meeting の戻り値。録音プロンプトの対象。
 */
export interface StartingMeeting {
  id: string;
  title: string;
  start: string;
  /**
   * 終了（`YYYY-MM-DDTHH:MM:SS` のローカル壁時計）。元データに DTEND が無ければ null。
   * これを使った「まだ進行中か」の判定は Rust の resolve_meeting_title が行う。
   * フロントで時刻計算をしないこと（scheduler.rs の窓と二重定義になってズレる）。
   */
  end: string | null;
}

// ── 話者の配色 ────────────────────────────────────────────────────────
// 「文字=濃色 / 地=その 14〜15% 透過 / ドット=中間色」。実際の色は index.css の --spk-N-*
// （ライト/ダークで切り替わる）。speaker_id の採番 S1.. を順に割り当てる。
export interface SpeakerInk {
  text: string;
  bg: string;
  dot: string;
}

const SPEAKER_COLORS = 8;
export const SPEAKER_PALETTE: SpeakerInk[] = Array.from({ length: SPEAKER_COLORS }, (_, i) => ({
  text: `var(--spk-${i + 1}-text)`,
  bg: `var(--spk-${i + 1}-bg)`,
  dot: `var(--spk-${i + 1}-dot)`,
}));

/** speaker_id（"S1" 等）→ パレットの添字。解析できなければ id ハッシュで散らす。 */
export function speakerIndex(id: string): number {
  const n = /^S(\d+)$/.exec(id);
  const idx = n
    ? parseInt(n[1], 10) - 1
    : id.split("").reduce((a, c) => a + c.charCodeAt(0), 0);
  const len = SPEAKER_PALETTE.length;
  return ((idx % len) + len) % len;
}

export function speakerInk(id: string): SpeakerInk {
  return SPEAKER_PALETTE[speakerIndex(id)];
}

/**
 * 実際に発言している話者の表示名だけを並べる（**書き出しヘッダー専用**）。
 *
 * 発言ゼロの話者行は**保存直後から存在しうる**。`merge::assign_speakers` は各セグメントに
 * 「最も重なる turn」だけを割り当てるので、turn は持つが常に他話者に負けるクラスタは
 * 発言ゼロになる。加えて発言単位の訂正（Issue #19）でも生じる — 最後の 1 件を移しても
 * `speakers` 行は残す設計（訂正を戻せるように）。
 * どちらにせよ `detail.speakers` をそのまま並べると 1 件も喋っていない人が載る。
 *
 * ⚠️ **一貫性を理由に他の話者リストへ広げないこと。** 訂正モーダルと SpeakerPanel は
 * 未フィルタの全話者を出す必要がある（発言ゼロになった話者を選び直せないと訂正が戻せない）。
 */
export function speakingSpeakerNames(detail: RecordingDetail): string[] {
  const spoke = new Set(
    detail.transcript.segments.map((x) => x.speaker_id).filter((x): x is string => !!x),
  );
  return (detail.speakers ?? [])
    .filter((s) => spoke.has(s.id))
    .map((s) => s.display_name ?? s.label);
}

/** 話者チップの inline style（文字色 + 地色）。 */
export function speakerChipStyle(id: string | null): { color: string; background: string } {
  // 話者不明は色を割り当てない（特定の人に見えてしまうため）。控えめな中間色。
  if (id === null) return { color: "var(--color-sub)", background: "var(--spk-none-bg)" };
  const ink = speakerInk(id);
  return { color: ink.text, background: ink.bg };
}

/**
 * 表示言語。i18n/index.tsx の Locale の別名（値は同一）。
 * lib/ の純関数は文言を dicts[lang]（format / output 名前空間）から引く —
 * 翻訳の正は辞書 1 箇所（ja.ts）で、lib 側に文言を持たない。
 * ja.ts / en.ts は import を持たないため循環は生じない。
 */
export type Lang = Locale;

/** speaker_id（"S1"）→ 既定ラベル（"話者1" / "Speaker 1"）。話者表が無い履歴表示で使う。 */
export function speakerLabelFromId(id: string, lang: Lang): string {
  const n = /^S(\d+)$/.exec(id);
  if (!n) return id;
  return dicts[lang].format.speakerLabel(n[1]);
}

/** speakers 表から表示名を引く。無ければ既定ラベルへフォールバック。 */
export function speakerName(id: string, speakers: Speaker[] | undefined, lang: Lang): string {
  const sp = speakers?.find((s) => s.id === id);
  if (sp) return sp.display_name ?? sp.label;
  return speakerLabelFromId(id, lang);
}

/**
 * 再生位置 ms にあたる発言の idx（#110）。開始が ms 以下で最も遅い発言。最初の発言より前なら null。
 * segments は start_ms 昇順（store が並べて返す）を前提に二分探索する。
 */
export function segmentAt(segments: Segment[], ms: number): number | null {
  let lo = 0;
  let hi = segments.length - 1;
  let found = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (segments[mid].start_ms <= ms) {
      found = mid;
      lo = mid + 1;
    } else {
      hi = mid - 1;
    }
  }
  return found < 0 ? null : segments[found].idx;
}

// ── 時刻・日時フォーマット ─────────────────────────────────────────────

/** ms → mm:ss（タイムスタンプ用）。 */
export function formatTimestamp(ms: number): string {
  const totalSec = Math.floor(ms / 1000);
  const m = Math.floor(totalSec / 60);
  const s = totalSec % 60;
  return `${m.toString().padStart(2, "0")}:${s.toString().padStart(2, "0")}`;
}

/**
 * 録音開始時刻（`Date.now()`）からの経過秒数。
 *
 * ⚠️ **`setInterval` の発火回数を数える方式にしないこと**（Issue #6 の原因）。
 * `setInterval` は「1000ms ちょうど」ではなく「**最短でも** 1000ms 後」に発火するため、
 * tick 回数 ≤ 実経過秒数となり、誤差が過小方向へ単調累積する。ウィンドウが隠れて
 * tick が間引かれると、累積方式はそこから復帰できない。
 *
 * 表示は必ず壁時計との差分から算出し、`setInterval` は**再描画のトリガにのみ**使う。
 *
 * `startedAt` が null（未開始）なら 0。システム時刻の巻き戻しでも負を返さない。
 */
export function elapsedSeconds(startedAt: number | null, now: number): number {
  if (startedAt === null) return 0;
  return Math.max(0, Math.floor((now - startedAt) / 1000));
}

/** ms → 尺表記。1 時間以上は h:mm:ss、未満は m:ss。 */
export function formatDuration(ms: number): string {
  const totalSec = Math.floor(ms / 1000);
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  const s = totalSec % 60;
  if (h > 0) {
    return `${h}:${m.toString().padStart(2, "0")}:${s.toString().padStart(2, "0")}`;
  }
  return `${m}:${s.toString().padStart(2, "0")}`;
}

/** ms → 「24分」「1時間12分」/ "24 min" "1 hr 12 min" のような人間向け尺。一覧のメタ表示に。 */
export function formatDurationHuman(ms: number, lang: Lang): string {
  const f = dicts[lang].format;
  const totalMin = Math.round(ms / 60000);
  if (totalMin < 60) return f.durationMin(totalMin);
  const h = Math.floor(totalMin / 60);
  const m = totalMin % 60;
  return m === 0 ? f.durationHour(h) : f.durationHourMin(h, m);
}

/** Lang → toLocale* に渡す BCP 47 ロケール（OS 設定でなくアプリ言語に揃える）。 */
const bcp47 = (lang: Lang) => (lang === "ja" ? "ja-JP" : "en-US");

/** RFC3339(UTC) → アプリ言語のローカル日時表記（秒は出さない・#105）。 */
export function formatDateTime(iso: string, lang: Lang): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString(bcp47(lang), {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/**
 * 録音の表示名。タイトルが無い（または既定名の「録音」「会議」）なら「マイク録音（9月21日 10:00）」の
 * ように種類と日時で作る（#105）。
 * 一覧・詳細・削除確認で同じ名前を出すため、表示はすべてここを通す。
 */
// バックエンドがタイトル未指定のマイク録音・会議に付ける既定名（commands/recording.rs）。
// 名前として意味を持たないので、未設定と同じく種類と日時で表示する（#105 レビュー）。
const DEFAULT_TITLES = new Set(["録音", "Recording", "会議", "Meeting"]);

export function recordingTitle(rec: Recording, lang: Lang): string {
  const title = rec.title?.trim();
  if (title && !DEFAULT_TITLES.has(title)) return title;
  const f = dicts[lang].format;
  const d = new Date(rec.created_at);
  const when = Number.isNaN(d.getTime())
    ? rec.created_at
    : d.toLocaleString(bcp47(lang), { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
  return f.untitledTitle(f.sourceKind[rec.source_type] ?? f.sourceKind.file, when);
}

/** RFC3339(UTC) → 「6月27日」/ "June 27" のような短い日付。 */
export function formatDateShort(iso: string, lang: Lang): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleDateString(bcp47(lang), { month: "long", day: "numeric" });
}

/**
 * カレンダー予定の開始時刻を一覧向けに整形
 * （ja:「今日 15:00」「明日 11:00」「金 14:00」「6/30 14:00」/ en: "Today 15:00" "Fri 14:00" "6/30 14:00"）。
 * `start` はローカル壁時計表記なので `new Date` のローカル解釈で良い。
 */
export function formatEventTime(start: string, lang: Lang): string {
  const d = new Date(start);
  if (Number.isNaN(d.getTime())) return start;
  const f = dicts[lang].format;
  const now = new Date();
  const dayOnly = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const days = Math.round((dayOnly(d) - dayOnly(now)) / 86_400_000);
  const time = `${d.getHours().toString().padStart(2, "0")}:${d
    .getMinutes()
    .toString()
    .padStart(2, "0")}`;
  if (days === 0) return f.eventToday(time);
  if (days === 1) return f.eventTomorrow(time);
  if (days >= 2 && days <= 6) {
    return `${f.weekdays[d.getDay()]} ${time}`;
  }
  return `${d.getMonth() + 1}/${d.getDate()} ${time}`;
}
