import { useCallback, useEffect, useRef, useState } from "react";
import { AppCtx, type MeetingState, type MeetingStartResult, type Route, type ToastKind } from "@/lib/app";
import { I18nCtx, detectLocale, dicts, resolveLocale, translateError, useI18n, type Locale } from "@/i18n";
import { cx } from "@/lib/cx";
import { clearJobStart, markJobStart, markStageStart } from "@/lib/jobClock";
import { isJobShown } from "@/lib/jobFocus";
import {
  cancelMeetingRecording,
  checkSystemAudioPermission,
  clearPendingMeeting,
  getPendingMeeting,
  getSettings,
  listJobs,
  listRecordingRows,
  resolveMeetingTitle,
  setSettings,
  startMeetingRecording,
  startMicRecording,
  stopMeetingRecording,
  useJobUpdate,
  useMeetingStarting,
  useMeetingLiveSnapshot,
  type LiveSnapshot,
} from "@/lib/tauri";
import {
  elapsedSeconds,
  formatTimestamp,
  TITLE_JOB_KIND,
  type RecordingRow,
  type StartingMeeting,
} from "@/lib/types";
import { Sidebar } from "@/components/Sidebar";
import { CheckIcon, StopIcon, VideoIcon, XIcon } from "@/components/icons";

import { HomeView } from "@/features/home/HomeView";
import { RecordingView } from "@/features/recording/RecordingView";
import { HistoryView } from "@/features/history/HistoryView";
import { DetailView } from "@/features/detail/DetailView";
import { SettingsView } from "@/features/settings/SettingsView";
import { MeetingView } from "@/features/meeting/MeetingView";
import { SpeakersView } from "@/features/speakers/SpeakersView";
import { IntegrationsView } from "@/features/integrations/IntegrationsView";
import { DigestView } from "@/features/digest/DigestView";
import { useLiveTranslation } from "@/lib/useLiveTranslation";
import type { TranslationTarget } from "@/lib/liveTranslation";
import { UpdateBanner } from "@/features/update/UpdateBanner";

interface ToastItem {
  id: number;
  message: string;
  kind: ToastKind;
}

function App() {
  const [route, setRoute] = useState<Route>({ view: "home" });
  const [recents, setRecents] = useState<RecordingRow[]>([]);
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const toastSeq = useRef(0);
  // UI 言語。真実は settings.json（起動時に load）。未確定の間は OS 言語で描画し、
  // 確定後に上書きする（ちらつき最小化）。
  const [lang, setLangState] = useState<Locale>(detectLocale);
  // Provider の内側でない App 本体（toast 文言など）用に辞書を直接引く。
  const t = dicts[lang];
  // 会議録音はアプリ全体の状態（録音実体はバックエンドに常駐し、画面遷移で消えない）。
  const [meeting, setMeeting] = useState<MeetingState>({ status: "idle", startedAt: null, title: null });
  const [liveSnapshot, setLiveSnapshot] = useState<LiveSnapshot | null>(null);
  const [translationTarget, setTranslationTarget] = useState<TranslationTarget>(lang);
  const translation = useLiveTranslation(meeting.status === "capturing", liveSnapshot);
  useMeetingLiveSnapshot((snapshot) => {
    if (meetingRef.current.status === "capturing") setLiveSnapshot(snapshot);
  });
  const meetingRef = useRef(meeting);
  meetingRef.current = meeting;
  // 開始処理は許可確認→開始の間に await を挟むので、status が capturing になる前に
  // 連打されると二重に start_meeting_recording を呼びうる。同期 ref で多重起動を防ぐ。
  const startingRef = useRef(false);
  // 常設バーをユーザーが一時的に閉じている間 true（サイドバーの録音中ドットは残る）。
  // 新しい会議を開始するたびに復帰させる。
  const [barDismissed, setBarDismissed] = useState(false);

  // 進行中ジョブ（pending|running）を持つ録音 id の集合。サイドバー最近の処理中ドット用（ADR-0024）。
  const [activeJobIds, setActiveJobIds] = useState<Set<string>>(new Set());

  // 会議開始の自動録音プロンプト（ADR-0026）。スケジューラが発火した会議を保持し、あれば
  // フロートカードで「録音する?」を出す。ライブ購読＋初期化時の取りこぼし回収の二段構え。
  const [pendingMeeting, setPendingMeeting] = useState<StartingMeeting | null>(null);
  const promptBusyRef = useRef(false);
  useMeetingStarting((m) => setPendingMeeting(m));
  useEffect(() => {
    // 通知が発火した時にウィンドウが購読していなくても、開いた時点で拾う。
    getPendingMeeting()
      .then((m) => {
        if (m) setPendingMeeting(m);
      })
      .catch(() => {
        /* スケジューラ未初期化などは無視 */
      });
  }, []);

  const refreshRecents = useCallback(() => {
    listRecordingRows()
      .then(setRecents)
      .catch(() => {
        /* 履歴未初期化などは無視（サイドバーは空のまま） */
      });
  }, []);

  useEffect(() => {
    refreshRecents();
  }, [refreshRecents]);

  // 起動時に進行中ジョブを拾って処理中ドットの初期状態にする（バックグラウンド継続の可視化）。
  useEffect(() => {
    listJobs()
      .then((jobs) =>
        setActiveJobIds(
          new Set(
            jobs
              .filter((j) => j.status === "pending" || j.status === "running")
              .map((j) => j.recording_id),
          ),
        ),
      )
      .catch(() => {});
  }, []);

  // 起動時に settings.json から言語を確定。未設定（初回起動 / 旧バージョンからの更新）なら
  // OS 言語で解決して書き戻す（以後 navigator.language に依存しない）。
  useEffect(() => {
    let active = true;
    (async () => {
      try {
        const s = await getSettings();
        const resolved = resolveLocale(s.language);
        if (!active) return;
        setLangState(resolved);
        if (s.language !== resolved) {
          await setSettings({ ...s, language: resolved });
        }
      } catch {
        /* 設定が読めなくても OS 言語のまま動作継続 */
      }
    })();
    return () => {
      active = false;
    };
  }, []);

  // 言語切替（設定画面から呼ばれる）。state を即反映し、settings.json へは
  // 「読み直し → language だけ差し替え」の read-modify-write で保存（他フィールドの巻き戻し防止）。
  const setLang = useCallback((next: Locale) => {
    setLangState(next);
    void (async () => {
      try {
        const cur = await getSettings();
        await setSettings({ ...cur, language: next });
      } catch {
        /* 保存失敗でも表示言語は切り替わる。次回起動時は旧言語に戻る */
      }
    })();
  }, []);

  const navigate = useCallback((next: Route) => {
    setRoute(next);
    // メイン領域を上端へ
    document.getElementById("mj-main")?.scrollTo({ top: 0 });
  }, []);

  const toast = useCallback((message: string, kind: ToastKind = "info") => {
    const id = ++toastSeq.current;
    setToasts((t) => [...t, { id, message, kind }]);
    window.setTimeout(() => {
      setToasts((t) => t.filter((x) => x.id !== id));
    }, 2200);
  }, []);

  // アプリ全体の job://update 購読（ADR-0024）。完了/失敗トースト＋最近更新＋処理中ドットの増減を
  // 一手に扱う。DetailView は自分のビュー更新に専念し、トーストはここへ集約（二重通知を避ける）。
  useJobUpdate((u) => {
    // タイトル自動生成は裏方。付いたらサイドバーの名前だけ更新する。
    if (u.kind === TITLE_JOB_KIND) {
      if (u.status === "done") refreshRecents();
      return;
    }
    const active = u.status === "pending" || u.status === "running";
    setActiveJobIds((prev) => {
      const next = new Set(prev);
      if (active) next.add(u.recording_id);
      else next.delete(u.recording_id);
      return next;
    });
    // 経過時間の起点を画面から独立して確定/破棄（DetailView がどの画面から開いても継続表示できる）。
    // 実処理に入った段（queued=permit 待ちを除く running）で種を蒔き、終了で捨てる。
    if (u.status === "running" && u.stage !== "queued") {
      markJobStart(u.job_id);
      // ETA の起点は「実進捗が出た最初の tick」(done>0 かつ total あり)。ステージ入場では
      // なくこの瞬間を起点にして固定オーバーヘッド(モデル読込等)を残り時間から除く。
      if (u.stage && u.total != null && u.done > 0) {
        markStageStart(u.job_id, u.stage);
      }
    } else if (u.status === "done" || u.status === "failed" || u.status === "canceled") {
      clearJobStart(u.job_id);
    }
    // 状態（失敗など）をサイドバーに反映する。
    if (u.status === "done" || u.status === "failed" || u.status === "canceled") refreshRecents();
    if (u.status === "canceled") {
      toast(t.job.canceledToast, "info");
    } else if (u.status === "done") {
      toast(u.kind === "diarize" ? t.job.diarizeCompleted : t.job.transcribeCompleted, "success");
    } else if (u.status === "failed") {
      // 詳細画面がこのジョブを表示しているなら、そちらの赤枠（再試行つき）だけにする（#107）。
      if (!isJobShown(u.job_id)) {
        toast(u.error ? translateError(u.error, t) : t.job.failedToast, "error");
      }
    }
  });

  // 会議録音: 許可確認 → キャプチャ開始。画面遷移は呼び出し側に任せる（idle のときだけ開始）。
  const startMeeting = useCallback(async (title?: string | null): Promise<MeetingStartResult> => {
    if (meetingRef.current.status !== "idle" || startingRef.current) return "started";
    startingRef.current = true;
    try {
      let granted = false;
      try {
        granted = await checkSystemAudioPermission();
      } catch {
        /* 取得失敗時は未許可として扱う */
      }
      if (!granted) {
        toast(t.app.systemAudioDenied, "error");
        return "denied";
      }
      // 呼び出し側が題名を指定しなければ、進行中のカレンダー予定から解決する。
      // 「まだ進行中か」の判定は Rust（resolve_meeting_title）が持つ。ここで
      // start / end を比べると scheduler.rs の窓と二重定義になるので、比べない。
      // 解決に失敗しても録音は始める（題名は既定の「会議」に落ちるだけ）。
      let resolved = title ?? null;
      if (resolved === null) {
        try {
          resolved = await resolveMeetingTitle();
        } catch {
          /* 題名が無いだけなので録音は続ける */
        }
      }
      await startMeetingRecording();
      translation.reset();
      setLiveSnapshot(null);
      setMeeting({ status: "capturing", startedAt: Date.now(), title: resolved });
      if (resolved !== null) {
        // 予定を消費したのでプロンプトを畳む（recordPendingMeeting と同じ扱い）。
        // 残すと、この録音を止めた後に同じ予定のプロンプトがまた出る。
        setPendingMeeting(null);
        void clearPendingMeeting();
      }
      setBarDismissed(false); // 新しい会議では常設バーを復帰
      return "started";
    } catch (e) {
      toast(translateError(e, t), "error");
      return "error";
    } finally {
      startingRef.current = false;
    }
  }, [toast, t]);

  // 会議録音: 停止 → デュアルトラック文字起こし保存 → 詳細へ。
  const stopMeeting = useCallback(async () => {
    if (meetingRef.current.status !== "capturing") return;
    translation.stop();
    const translations = translation.completed();
    meetingRef.current = { ...meetingRef.current, status: "stopping" };
    setMeeting(meetingRef.current);
    try {
      const res = await stopMeetingRecording(meetingRef.current.title, translations);
      setMeeting({ status: "idle", startedAt: null, title: null });
      refreshRecents();
      navigate({ view: "detail", id: res.recording_id });
    } catch (e) {
      if (String(e).startsWith("translation.history_invalid:")) {
        meetingRef.current = { ...meetingRef.current, status: "capturing" };
        setMeeting(meetingRef.current);
        toast(t.meeting.translation.saveInvalid, "error");
        navigate({ view: "meeting" });
        return;
      }
      toast(translateError(e, t), "error");
      setMeeting({ status: "idle", startedAt: null, title: null });
      navigate({ view: "home" });
    }
  }, [toast, refreshRecents, navigate, t]);

  // 会議録音: 破棄（保存しない）。誤開始のやり直し用。
  const discardMeeting = useCallback(async () => {
    if (meetingRef.current.status === "idle") return;
    translation.reset();
    setLiveSnapshot(null);
    setMeeting({ status: "idle", startedAt: null, title: null });
    try {
      await cancelMeetingRecording();
    } catch {
      /* 既に解放済みなどは無視 */
    }
  }, []);

  // 長尺ガード（メモリ保護・遷移しても効くようアプリ全体で監視）。
  // 90 分で一度だけ警告、3 時間で自動的に「停止して保存」（データは失わない）。
  const longWarnedRef = useRef(false);
  useEffect(() => {
    if (meeting.status !== "capturing" || meeting.startedAt == null) {
      longWarnedRef.current = false;
      return;
    }
    const started = meeting.startedAt;
    const timer = window.setInterval(() => {
      const sec = elapsedSeconds(started, Date.now());
      if (sec >= 3 * 60 * 60) {
        toast(t.app.autoStopAtLimit, "info");
        void stopMeeting();
      } else if (sec >= 90 * 60 && !longWarnedRef.current) {
        longWarnedRef.current = true;
        toast(t.app.longRecordingWarn, "info");
      }
    }, 5000);
    return () => clearInterval(timer);
  }, [meeting.status, meeting.startedAt, toast, stopMeeting, t]);

  // プロンプトの「録音する」= 既存「記録を準備」と同じ（マイク録音開始＋予定タイトル＋話者分離 ON）。
  const recordPendingMeeting = useCallback(async () => {
    if (promptBusyRef.current || !pendingMeeting) return;
    promptBusyRef.current = true;
    const title = pendingMeeting.title;
    try {
      await startMicRecording();
      navigate({ view: "recording", diarize: true, title });
      setPendingMeeting(null);
      void clearPendingMeeting();
    } catch (e) {
      toast(translateError(e, t), "error");
    } finally {
      promptBusyRef.current = false;
    }
  }, [pendingMeeting, navigate, toast, t]);

  const dismissPendingMeeting = useCallback(() => {
    setPendingMeeting(null);
    void clearPendingMeeting();
  }, []);

  const recordingElsewhere =
    meeting.status !== "idle" && route.view !== "meeting" && !barDismissed;
  // 既に録音中（会議モード or マイク録音画面）のときはプロンプトを出さない（二重録音の誘発を避ける）。
  const showMeetingPrompt =
    !!pendingMeeting && meeting.status === "idle" && route.view !== "recording";

  return (
    <I18nCtx.Provider value={{ lang, t: dicts[lang], setLang }}>
    <AppCtx.Provider
      value={{ route, navigate, toast, refreshRecents, meeting, startMeeting, stopMeeting, discardMeeting, liveSnapshot, translation, translationTarget, setTranslationTarget }}
    >
      <div className="flex h-screen w-screen overflow-hidden bg-bg text-body">
        <Sidebar recents={recents} activeJobIds={activeJobIds} />
        <main id="mj-main" className="min-w-0 flex-1 overflow-y-auto">
          <Router route={route} />
        </main>
      </div>

      {/* 会議録音中に別画面へ移ったときの常設インジケータ（戻る/停止の導線） */}
      {recordingElsewhere && (
        <MeetingBar
          startedAt={meeting.startedAt}
          stopping={meeting.status === "stopping"}
          onReturn={() => navigate({ view: "meeting" })}
          onStop={() => void stopMeeting()}
          onDismiss={() => setBarDismissed(true)}
        />
      )}

      {/* 会議開始の自動録音プロンプト（ADR-0026）。通知の受け皿＝クリックで録音開始。 */}
      {showMeetingPrompt && pendingMeeting && (
        <MeetingStartPrompt
          title={pendingMeeting.title}
          onRecord={() => void recordPendingMeeting()}
          onDismiss={dismissPendingMeeting}
        />
      )}

      {/* アップデート通知（起動時チェック → あればフロート表示） */}
      <UpdateBanner />

      {/* トースト */}
      <div className="pointer-events-none fixed bottom-5 left-1/2 z-[60] flex -translate-x-1/2 flex-col items-center gap-2">
        {toasts.map((t) => (
          <div
            key={t.id}
            className={cx(
              "animate-mjfade flex items-center gap-2 rounded-btn border px-3.5 py-2 text-[13px] shadow-pop",
              t.kind === "error"
                ? "border-red/40 bg-surface text-red-light"
                : t.kind === "success"
                  ? "border-green/40 bg-surface text-green-light"
                  : "border-border-3 bg-surface text-body",
            )}
          >
            {t.kind === "success" && <CheckIcon size={15} className="text-green" />}
            {t.kind === "error" && <XIcon size={15} className="text-red-light" />}
            {t.message}
          </div>
        ))}
      </div>
    </AppCtx.Provider>
    </I18nCtx.Provider>
  );
}

/** 会議録音中に別画面へ移ったときの常設バー（上端中央フロート）。経過時間 + 戻る/停止。 */
function MeetingBar({
  startedAt,
  stopping,
  onReturn,
  onStop,
  onDismiss,
}: {
  startedAt: number | null;
  stopping: boolean;
  onReturn: () => void;
  onStop: () => void;
  onDismiss: () => void;
}) {
  const { t } = useI18n();
  const [elapsed, setElapsed] = useState(() => elapsedSeconds(startedAt, Date.now()));
  useEffect(() => {
    const t = window.setInterval(() => {
      setElapsed(elapsedSeconds(startedAt, Date.now()));
    }, 1000);
    return () => clearInterval(t);
  }, [startedAt]);

  return (
    <div className="pointer-events-none fixed left-1/2 top-4 z-[55] flex -translate-x-1/2">
      <div className="pointer-events-auto flex items-center gap-3 rounded-full border border-border-3 bg-surface/95 py-2 pl-3.5 pr-2 shadow-pop backdrop-blur">
        <span className="flex items-center gap-2 text-[13px] text-body">
          <span className="h-2 w-2 animate-mjpulse rounded-full bg-red ring-[3px] ring-red/20" />
          {t.app.meetingBar.recording}
          <span className="font-mono text-ink tnum">{formatTimestamp(elapsed * 1000)}</span>
        </span>
        <button
          onClick={onReturn}
          className="inline-flex items-center gap-1.5 rounded-full border border-border-2 bg-surface-2 px-3 py-1.5 text-[12px] text-body transition-colors hover:bg-hover"
        >
          <VideoIcon size={14} className="text-brand-light" />
          {t.app.meetingBar.backToMeeting}
        </button>
        <button
          onClick={onStop}
          disabled={stopping}
          className="inline-flex items-center gap-1.5 rounded-full px-3 py-1.5 text-[12px] font-medium bg-danger text-on-brand transition-[filter] hover:brightness-110 disabled:opacity-60"
        >
          <StopIcon size={13} />
          {stopping ? t.app.meetingBar.saving : t.app.meetingBar.stopAndSave}
        </button>
        {/* 一時的に閉じる（録音は継続。サイドバーの録音中ドットで気づける）。 */}
        <button
          onClick={onDismiss}
          disabled={stopping}
          aria-label={t.app.meetingBar.dismiss}
          title={t.app.meetingBar.dismissHint}
          className="flex h-6 w-6 items-center justify-center rounded-full text-dim transition-colors hover:bg-hover hover:text-body disabled:opacity-40"
        >
          <XIcon size={14} />
        </button>
      </div>
    </div>
  );
}

/**
 * 会議開始の自動録音プロンプト（ADR-0026）。予定開始時にフロート表示し、
 * 「録音する」で既存の「記録を準備」フローに入る。通知（バナー）と同じ受け皿を UI 側にも置く。
 */
function MeetingStartPrompt({
  title,
  onRecord,
  onDismiss,
}: {
  title: string;
  onRecord: () => void;
  onDismiss: () => void;
}) {
  const { t } = useI18n();
  return (
    <div className="fixed bottom-5 right-5 z-[58] w-[320px] max-w-[calc(100vw-2.5rem)]">
      <div className="rounded-win border border-border-3 bg-surface/95 p-4 shadow-pop backdrop-blur">
        <div className="flex items-start gap-3">
          <span className="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-brand/15 text-brand-light">
            <VideoIcon size={16} />
          </span>
          <div className="min-w-0">
            <p className="text-[13px] font-semibold text-ink">{t.app.meetingStartPrompt.heading}</p>
            <p className="mt-0.5 truncate text-[13px] text-body">
              {t.app.meetingStartPrompt.body(title)}
            </p>
          </div>
        </div>
        <div className="mt-3.5 flex justify-end gap-2">
          <button
            onClick={onDismiss}
            className="rounded-btn border border-border-2 bg-surface-2 px-3 py-1.5 text-[12px] text-body transition-colors hover:bg-hover"
          >
            {t.app.meetingStartPrompt.dismiss}
          </button>
          <button
            onClick={onRecord}
            className="inline-flex items-center gap-1.5 rounded-btn px-3 py-1.5 text-[12px] font-medium bg-danger text-on-brand transition-[filter] hover:brightness-110"
          >
            <VideoIcon size={13} />
            {t.app.meetingStartPrompt.record}
          </button>
        </div>
      </div>
    </div>
  );
}

function Router({ route }: { route: Route }) {
  switch (route.view) {
    case "home":
      return <HomeView />;
    case "recording":
      return (
        <RecordingView
          diarize={route.diarize ?? false}
          title={route.title}
          recordOnly={route.recordOnly ?? false}
        />
      );
    case "history":
      return <HistoryView />;
    case "detail":
      // key で録音ごとに作り直す。再生位置・追従・検索・中断中などの画面内の状態を、
      // 別の録音へ持ち越さないため（#110 レビュー）。
      return route.id ? <DetailView key={route.id} id={route.id} /> : <HomeView />;
    case "settings":
      return <SettingsView />;
    case "meeting":
      return <MeetingView />;
    case "speakers":
      return <SpeakersView />;
    case "integrations":
      return <IntegrationsView />;
    case "digest":
      return <DigestView />;
    default:
      return <HomeView />;
  }
}

export default App;
