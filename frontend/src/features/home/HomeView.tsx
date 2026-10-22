// ホーム（#115）。始め方（会議 / マイク / ファイル）→ 今日の予定 → 最近の録音。
// 以前は配布サイトのような売り文句（特長カード・「Mac の中だけ」の 2 回表示）が並んでいた。
// 会議録音はアプリ全体の状態（useApp().meeting）。録音中は二重録音を避けてここからは開始させない。
import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useApp } from "@/lib/app";
import { ModelSetupCard } from "@/features/setup/ModelSetup";
import { cx } from "@/lib/cx";
import { translateError, useI18n } from "@/i18n";
import { listCalendarEvents, listRecordingRows, startMicRecording, transcribeFile, useJobUpdate } from "@/lib/tauri";
import { getDiarizePref, setDiarizePref } from "@/lib/prefs";
import {
  formatDateShort,
  formatDurationHuman,
  formatEventTime,
  recordingState,
  recordingTitle,
  type CalendarEvent,
  type RecordingRow,
} from "@/lib/types";
import { Toggle } from "@/components/ui";
import { PrivacyBar, RecordingStateBadge, SourceIcon } from "@/components/composite";
import { CalendarIcon, FileAudioIcon, MicIcon, VideoIcon } from "@/components/icons";

const RECENT_COUNT = 5;
/** 終了時刻の無い予定は、開始からこの時間は「進行中」とみなす。 */
const OPEN_ENDED_MS = 60 * 60 * 1000;

/** これから・進行中の予定（終わったものは除く）。 */
export function upcomingEvents(evs: CalendarEvent[], now: number): CalendarEvent[] {
  return evs.filter((e) => {
    const start = new Date(e.start).getTime();
    const end = e.end ? new Date(e.end).getTime() : start + OPEN_ENDED_MS;
    return end >= now;
  });
}

// 足すときは Rust 側の取り込み許可リスト（src-tauri/src/commands/mod.rs の IMPORT_EXTENSIONS）にも足す。
const AUDIO_EXT = ["mp3", "wav", "m4a", "aac", "aiff", "flac", "ogg"];
const isAudio = (p: string) => AUDIO_EXT.some((x) => p.toLowerCase().endsWith(`.${x}`));

export function HomeView() {
  const { navigate, toast, refreshRecents, meeting, startMeeting } = useApp();
  const { t, lang } = useI18n();
  // 話者分離は既定 ON で、最後の選択を覚える（#115）。会議モードは常に話者分離つき。
  const [diarize, setDiarizeState] = useState(getDiarizePref);
  const setDiarize = (on: boolean) => {
    setDiarizeState(on);
    setDiarizePref(on);
  };
  const [recent, setRecent] = useState<RecordingRow[] | null>(null);
  const [events, setEvents] = useState<CalendarEvent[]>([]);
  const loadRecent = useCallback(() => {
    listRecordingRows()
      .then((rows) => setRecent(rows.slice(0, RECENT_COUNT)))
      .catch(() => setRecent([]));
  }, []);
  // 処理が終わると状態（処理中 → 要約あり など）が変わるので取り直す。
  useJobUpdate((u) => {
    if (u.status === "done" || u.status === "failed" || u.status === "canceled") loadRecent();
  });
  useEffect(() => {
    loadRecent();
    // カレンダー未連携はエラーで返る。そのときは予定の欄ごと出さない。
    listCalendarEvents()
      .then((evs) => {
        setEvents(upcomingEvents(evs, Date.now()).slice(0, 3));
      })
      .catch(() => setEvents([]));
  }, [loadRecent]);
  // 音声だけ保存（後から文字起こし・ADR-0024 増分5）。ON のとき停止/取込は録音行だけ作りジョブは積まない。
  const [recordOnly, setRecordOnly] = useState(false);
  const [busy, setBusy] = useState(false);
  const [dragOver, setDragOver] = useState(false);

  // drag&drop のクロージャから最新 diarize / recordOnly / busy / 会議状態を読むための ref
  const diarizeRef = useRef(diarize);
  diarizeRef.current = diarize;
  const recordOnlyRef = useRef(recordOnly);
  recordOnlyRef.current = recordOnly;
  const busyRef = useRef(busy);
  busyRef.current = busy;
  const meetingActiveRef = useRef(meeting.status !== "idle");
  meetingActiveRef.current = meeting.status !== "idle";

  // 会議モード（主役）: idle なら開始 → 会議画面へ。録音中なら会議画面へ戻る。
  const beginMeeting = useCallback(async (title?: string) => {
    if (meeting.status !== "idle") {
      navigate({ view: "meeting" });
      return;
    }
    const r = await startMeeting(title);
    // started → 録音中ビュー、denied → 開始画面で許可を誘導（startMeeting がトースト済み）。
    if (r !== "error") navigate({ view: "meeting" });
  }, [meeting.status, startMeeting, navigate]);

  const runFile = useCallback(
    async (path: string) => {
      if (meetingActiveRef.current) {
        toast(t.home.meetingBusy, "info");
        return;
      }
      // ref を同期更新して二重起動を防ぐ（state 経由の再レンダー同期だけだと、
      // 同一イベント内に複数ハンドラが発火した場合に両方すり抜ける）。
      if (busyRef.current) return;
      busyRef.current = true;
      setBusy(true);
      try {
        const res = await transcribeFile(
          path,
          diarizeRef.current,
          recordOnlyRef.current,
        );
        refreshRecents();
        navigate({ view: "detail", id: res.recording_id });
      } catch (e) {
        toast(translateError(e, t), "error");
      } finally {
        busyRef.current = false;
        setBusy(false);
      }
    },
    [navigate, toast, refreshRecents, t],
  );

  const pickFile = useCallback(async () => {
    const selected = await open({
      multiple: false,
      directory: false,
      filters: [{ name: t.home.audioFilterName, extensions: AUDIO_EXT }],
    });
    if (typeof selected === "string") runFile(selected);
  }, [runFile, t]);

  // OS のドラッグ&ドロップ（Tauri webview イベント）。
  // unlisten は Promise で返るため、cleanup が解決前に走っても解除漏れしないよう
  // active フラグ + Promise 経由の解除にする（lib/tauri.ts の useTauriEvent と同じパターン）。
  useEffect(() => {
    let active = true;
    const unlistenP = getCurrentWebview().onDragDropEvent((e) => {
      if (!active) return;
      if (e.payload.type === "over" || e.payload.type === "enter") setDragOver(true);
      else if (e.payload.type === "leave") setDragOver(false);
      else if (e.payload.type === "drop") {
        setDragOver(false);
        if (busyRef.current) return; // 処理中はドロップを無視（二重起動防止）
        const p = e.payload.paths.find(isAudio);
        if (p) runFile(p);
        else if (e.payload.paths.length > 0)
          toast(t.home.unsupportedFile, "error");
      }
    });
    return () => {
      active = false;
      unlistenP.then((un) => un());
    };
  }, [runFile, toast, t]);

  const startMic = useCallback(async () => {
    if (meeting.status !== "idle") {
      toast(t.home.meetingBusy, "info");
      return;
    }
    try {
      await startMicRecording();
      navigate({ view: "recording", diarize, recordOnly });
    } catch (e) {
      toast(translateError(e, t), "error");
    }
  }, [navigate, toast, diarize, recordOnly, meeting.status, t]);

  const recording = meeting.status !== "idle";

  const actionCard =
    "flex items-center gap-3 rounded-card border border-border-2 bg-surface-2 px-4 py-3.5 text-left transition-colors hover:bg-hover disabled:opacity-45";

  return (
    <div className="mx-auto flex max-w-[760px] flex-col gap-6 px-8 py-10">
      <header>
        <h1 className="text-[22px] font-bold text-ink">{t.home.title}</h1>
        <p className="mt-1 text-[13px] text-muted">{t.home.subtitle}</p>
      </header>

      {/* 初回だけ: 文字起こしモデルの準備（#112）。揃っていれば何も出さない。 */}
      <ModelSetupCard variant="home" />

      {/* 始め方。会議が主役、マイクとファイルはその下に並べる。 */}
      <section className="flex flex-col gap-3">
        <button
          onClick={() => void beginMeeting()}
          className="group flex items-center gap-4 rounded-win border border-brand/30 bg-linear-135 from-brand/18 to-brand-2/10 px-5 py-5 text-left transition-[filter] hover:brightness-[1.06]"
        >
          <span className="flex h-12 w-12 shrink-0 items-center justify-center rounded-card bg-brand/20 text-brand-light">
            <VideoIcon size={24} />
          </span>
          <span className="min-w-0 flex-1">
            <span className="block text-[15px] font-bold text-ink">{t.home.meetingCard.title}</span>
            <span className="mt-1 block text-[13px] leading-relaxed text-muted">
              {t.home.meetingCard.desc}
            </span>
          </span>
          <span className="bg-brand-gradient inline-flex h-10 shrink-0 items-center gap-2 rounded-btn px-5 text-[13px] font-medium text-white">
            {recording ? (
              <>
                <span className="h-2 w-2 animate-mjpulse rounded-full bg-white/90" />
                {t.app.meetingBar.backToMeeting}
              </>
            ) : (
              <>
                <span className="h-2.5 w-2.5 rounded-full bg-white/90" />
                {t.home.meetingCard.start}
              </>
            )}
          </span>
        </button>

        <div className="grid grid-cols-2 gap-3">
          <button onClick={startMic} disabled={busy} className={actionCard}>
            <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-ctl bg-red/12 text-red-light">
              <MicIcon size={18} />
            </span>
            <span className="min-w-0">
              <span className="block text-[14px] font-semibold text-ink">{t.home.recordMic}</span>
              <span className="block text-[12px] text-muted">{t.home.recordMicDesc}</span>
            </span>
          </button>
          <button
            onClick={pickFile}
            disabled={busy}
            className={cx(actionCard, dragOver && "border-brand bg-selected")}
          >
            <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-ctl bg-brand/12 text-brand-light">
              <FileAudioIcon size={18} />
            </span>
            <span className="min-w-0">
              <span className="block text-[14px] font-semibold text-ink">{t.home.chooseFile}</span>
              <span className="block text-[12px] text-muted">{t.home.dropHint}</span>
            </span>
          </button>
        </div>

        {/* マイク録音とファイル取り込みの設定（会議モードには効かない）。 */}
        <div className="flex flex-wrap items-center gap-x-6 gap-y-2 px-1 text-[13px] text-body">
          <label className="flex items-center gap-2" title={t.home.diarize.desc}>
            <Toggle
              checked={diarize && !recordOnly}
              onChange={setDiarize}
              disabled={busy || recordOnly}
              label={t.home.diarize.label}
            />
            {t.home.diarize.title}
          </label>
          <label className="flex items-center gap-2" title={t.home.recordOnly.desc}>
            <Toggle
              checked={recordOnly}
              onChange={setRecordOnly}
              disabled={busy}
              label={t.home.recordOnly.label}
            />
            {t.home.recordOnly.title}
          </label>
        </div>
      </section>

      {/* 今日の予定（カレンダー連携時だけ）。予定名で会議の記録を始められる。 */}
      {events.length > 0 && (
        <section>
          <h2 className="mb-2 flex items-center gap-2 text-[13px] font-semibold text-sub">
            <CalendarIcon size={14} />
            {t.home.upcoming}
          </h2>
          <ul className="overflow-hidden rounded-card border border-border bg-surface-2">
            {events.map((ev, i) => (
              <li
                key={ev.id + ev.start}
                className={cx("flex items-center gap-3 px-4 py-2.5", i > 0 && "border-t border-line")}
              >
                <span className="w-24 shrink-0 font-mono text-[12px] text-muted tnum">
                  {formatEventTime(ev.start, lang)}
                </span>
                <span className="min-w-0 flex-1 truncate text-[14px] text-ink">{ev.title}</span>
                <button
                  onClick={() => void beginMeeting(ev.title)}
                  disabled={recording}
                  className="h-8 shrink-0 rounded-ctl border border-border-2 px-3 text-[12px] text-body transition-colors hover:bg-hover disabled:opacity-45"
                >
                  {t.home.recordThisMeeting}
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}

      {/* 最近の録音 */}
      {recent && recent.length > 0 && (
        <section>
          <div className="mb-2 flex items-center justify-between">
            <h2 className="text-[13px] font-semibold text-sub">{t.home.recent}</h2>
            <button
              onClick={() => navigate({ view: "history" })}
              className="text-[12px] text-brand-light hover:underline"
            >
              {t.home.seeAll}
            </button>
          </div>
          <ul className="overflow-hidden rounded-card border border-border bg-surface-2">
            {recent.map((row, i) => (
              <li key={row.recording.id} className={cx(i > 0 && "border-t border-line")}>
                <button
                  onClick={() => navigate({ view: "detail", id: row.recording.id })}
                  className="flex w-full items-center gap-3 px-4 py-2.5 text-left transition-colors hover:bg-hover"
                >
                  <SourceIcon type={row.recording.source_type} />
                  <span className="min-w-0 flex-1 truncate text-[14px] text-ink">
                    {recordingTitle(row.recording, lang)}
                  </span>
                  <RecordingStateBadge state={recordingState(row)} />
                  <span className="shrink-0 font-mono text-[12px] text-muted tnum">
                    {formatDateShort(row.recording.created_at, lang)} · {formatDurationHuman(row.recording.duration_ms, lang)}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}

      <PrivacyBar>{t.home.privacy}</PrivacyBar>
    </div>
  );
}
