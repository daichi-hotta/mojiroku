// 録音の詳細（Studio 04 + 06 + 10 + 11）。最も大きいビュー。
// 中央メイン（AI議事録 + 文字起こし/チャプター）+ 右ペイン 222px（話者 + MCP）。
// 左サイドバーは App が描く。実機能: 取得 / 話者改名 / 話者訂正（発言単位）/ 要約生成 / 共有。
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { getJobStart, getStageStart, markJobStart } from "@/lib/jobClock";
import { setShownJob } from "@/lib/jobFocus";
import { useApp } from "@/lib/app";
import { cx } from "@/lib/cx";
import {
  cancelJob,
  deleteRecording,
  diarizeRecording,
  generateTitle,
  getRecording,
  getSettings,
  listJobs,
  recordingAudioSrc,
  renameRecording,
  transcribeRecording,
  useJobUpdate,
  setSegmentSpeaker,
} from "@/lib/tauri";
import { MOCK_PREVIEW } from "@/lib/mockData";
import { translateError, useI18n } from "@/i18n";
import {
  formatDateTime,
  recordingTitle,
  segmentAt,
  formatDuration,
  type Job,
  type RecordingDetail,
  type Summary,
  type Segment,
  formatTimestamp,
  speakerChipStyle,
  speakerName,
  TITLE_JOB_KIND,
} from "@/lib/types";
import { ConfirmDialog, MenuItem, Modal, ModalHeader, Spinner, Toggle } from "@/components/ui";
import { EmptyState, PreviewTag, TranscriptList, Waveform } from "@/components/composite";
import {
  ArrowUpRightIcon,
  BotIcon,
  CheckIcon,
  PencilIcon,
  PlayIcon,
  RefreshIcon,
  SearchIcon,
  SparklesIcon,
  TrashIcon,
  XIcon,
} from "@/components/icons";
import { SpeakerPanel } from "./SpeakerPanel";
import { SharePopover } from "./SharePopover";
import { TemplateModal } from "./TemplateModal";
import { AskDrawer } from "./AskDrawer";
import { SavedTranslations } from "./SavedTranslations";
import { AudioPlayer, type AudioPlayerHandle } from "./AudioPlayer";
import { Markdown } from "@/lib/markdown";
import { getDiarizePref, setDiarizePref } from "@/lib/prefs";
import { findSummary } from "@/lib/templates";

// チャプターはモック（トピック自動分割は未実装・Studio 15）。
const CHAPTERS = [
  { time: "00:00", color: "var(--spk-1-dot)", grow: 2.1, title: "ベータ配布の状況", dur: "2分", body: "未署名ビルドの初回起動でつまずく人が多く、許可手順に画像を追加。" },
  { time: "02:10", color: "var(--spk-2-dot)", grow: 3.3, title: "オンボーディング刷新", dur: "3.5分", body: "「ローカル完結・基本無料」を初回画面の主役に据える方針で合意。" },
  { time: "05:40", color: "var(--spk-6-dot)", grow: 3.6, title: "モデルDLの統合", dur: "3.6分", body: "ダウンロードを初回フローへ統合。進捗は控えめに、状態は分かるように。" },
  { time: "09:20", color: "var(--spk-3-dot)", grow: 3.1, title: "ネクストと宿題の確認", dur: "3.1分", body: "佐藤=初回画面デザイン案、鈴木=DL進捗UI。次回までに共有。" },
];

const MOCK_TRANSLATION = "（翻訳プレビュー）この発話の日本語訳がここに表示されます。";

function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      className={cx(
        "-mb-px border-b-2 px-3 pb-2 pt-1 text-[13px] transition-colors",
        active ? "border-brand text-ink" : "border-transparent text-muted hover:text-body",
      )}
    >
      {children}
    </button>
  );
}

export function DetailView({ id }: { id: string }) {
  const { navigate, toast, refreshRecents } = useApp();
  const { t, lang } = useI18n();
  // 詳細画面の UI ラベル（テンプレ id → 表示名）。未知 id は id をそのまま出す。
  const templateLabel = (id: string): string => t.detail.templateNames[id] ?? id;
  const [detail, setDetail] = useState<RecordingDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [audioSrc, setAudioSrc] = useState<string | null>(null);
  // 再生と文字起こしの連動（#110）。位置は AudioPlayer から受け取り、行の強調と自動追従に使う。
  const playerRef = useRef<AudioPlayerHandle>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const [playMs, setPlayMs] = useState(0);
  const [playing, setPlaying] = useState(false);
  // 再生中は現在行を追いかけてスクロールする。利用者が自分でスクロールしたら追従をやめ、
  // 「再生位置に戻る」で再開する。
  const [following, setFollowing] = useState(true);
  // 自動追従のスクロール中はこの時刻まで。それ以外のスクロール（ホイール・スクロールバー・キー）は
  // 利用者の操作とみなして追従をやめる。
  const autoScrollUntil = useRef(0);
  // 文字起こし内検索（#111）。⌘F で開き、Enter / Shift+Enter で次 / 前へ。
  const [searchOpen, setSearchOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [matchPos, setMatchPos] = useState(0);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const q = query.trim().toLowerCase();
  const matches =
    searchOpen && q && detail
      ? detail.transcript.segments.filter((s) => s.text.toLowerCase().includes(q)).map((s) => s.idx)
      : [];
  const currentMatchIdx = matches.length ? matches[Math.min(matchPos, matches.length - 1)] : null;
  const moveMatch = (step: number) => {
    if (!matches.length) return;
    setFollowing(false);
    setMatchPos((p) => (Math.min(p, matches.length - 1) + step + matches.length) % matches.length);
  };
  const closeSearch = () => {
    setSearchOpen(false);
    setQuery("");
    setMatchPos(0);
  };
  useEffect(() => {
    if (currentMatchIdx == null) return;
    scrollRef.current
      ?.querySelector<HTMLElement>(`[data-seg-idx="${currentMatchIdx}"]`)
      ?.scrollIntoView({ block: "center", behavior: "smooth" });
  }, [currentMatchIdx]);
  const seekToSegment = useCallback((seg: Segment) => {
    setFollowing(true);
    playerRef.current?.seek(seg.start_ms, true);
  }, []);

  // 現在行が画面外に出たら中央へ寄せる（再生中かつ追従中だけ）。
  const followIdx =
    audioSrc && playing && following && detail
      ? segmentAt(detail.transcript.segments, playMs)
      : null;
  useEffect(() => {
    if (followIdx == null) return;
    const box = scrollRef.current;
    const row = box?.querySelector<HTMLElement>(`[data-seg-idx="${followIdx}"]`);
    if (!box || !row) return;
    const b = box.getBoundingClientRect();
    const r = row.getBoundingClientRect();
    if (r.top < b.top + 40 || r.bottom > b.bottom - 40) {
      autoScrollUntil.current = Date.now() + 800;
      row.scrollIntoView({ block: "center", behavior: "smooth" });
    }
  }, [followIdx]);

  // スペースキーで再生 / 一時停止。入力欄・ダイアログ・ふつうのボタンの中では奪わない
  // （ボタンはスペースで押すのが標準の操作）。ただし時刻ボタン（data-seek）に選択が残っているときは
  // 再生の切り替えにする。押したあと選択がそこに残るので、奪わないと同じ位置へ飛び直してしまう。
  // ←/→ は 5 秒戻る / 15 秒進む（再生バーのボタンと同じ）。⌘F は文字起こし内検索。
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement | null;
      if (e.metaKey && !e.shiftKey && e.key.toLowerCase() === "f") {
        if (el?.closest("[role=dialog]")) return;
        e.preventDefault();
        setSearchOpen(true);
        setTab("transcript");
        requestAnimationFrame(() => searchInputRef.current?.select());
        return;
      }
      if (!audioSrc || e.metaKey || e.ctrlKey || e.altKey) return;
      const onSeekButton = !!el?.closest("[data-seek]");
      const typing = !!el?.closest("input, textarea, select, [contenteditable=true], [role=dialog]");
      if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
        if (typing || el?.closest("[role=slider]")) return;
        e.preventDefault();
        playerRef.current?.skip(e.key === "ArrowLeft" ? -5000 : 15000);
        return;
      }
      if (e.code !== "Space") return;
      if (!onSeekButton && (typing || el?.closest("button, a"))) return;
      e.preventDefault();
      playerRef.current?.toggle();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [audioSrc]);


  const [tab, setTab] = useState<"transcript" | "chapters" | "translations">("transcript");
  const [translateOn, setTranslateOn] = useState(false);
  const [modalOpen, setModalOpen] = useState(false);
  const [presetTemplate, setPresetTemplate] = useState("minutes");
  const [askOpen, setAskOpen] = useState(false);
  const [confirmDel, setConfirmDel] = useState(false);
  const [deleting, setDeleting] = useState(false);
  // タイトルのインライン編集。
  const [editingTitle, setEditingTitle] = useState(false);
  const [titleValue, setTitleValue] = useState("");
  const [generatingTitle, setGeneratingTitle] = useState(false);
  const [confirmCloudTitle, setConfirmCloudTitle] = useState(false);
  const [savingTitle, setSavingTitle] = useState(false);

  // バックグラウンドジョブ（ADR-0024）。detail.active_job を起点に job://update で追う。
  const [job, setJob] = useState<Job | null>(null);
  const [jobProgress, setJobProgress] = useState<{ done: number; total: number | null }>({
    done: 0,
    total: null,
  });
  // 後から文字起こしするときの話者分離もホームと同じ設定を使う（既定 ON・#115）。
  const [transcribeDiarize, setTranscribeDiarizeState] = useState(getDiarizePref);
  const setTranscribeDiarize = (on: boolean) => {
    setTranscribeDiarizeState(on);
    setDiarizePref(on);
  };
  const [canceling, setCanceling] = useState(false);
  const [confirmCancel, setConfirmCancel] = useState(false);
  const [starting, setStarting] = useState(false);
  const processing = job?.status === "pending" || job?.status === "running";
  // このジョブの失敗はこの画面で出す（App のトーストと二重にしない）。
  useEffect(() => {
    setShownJob(job?.id ?? null);
    return () => setShownJob(null);
  }, [job?.id]);
  const jobFailed = job?.status === "failed";
  // 実処理中（順番待ち=pending/queued を除く）だけ「ローディングが進んでいる」実感のため
  // 経過時間を刻む。core は多くの段で incremental % を出さない（stage,0,None のみ）ので、
  // 経過時間が「止まっていない」ことの唯一の可視シグナルになる（ETA/% は別途）。
  const jobActive = job?.status === "running" && job.stage !== "queued";
  // 実処理中は 1 秒ごとに now を刻み、経過時間と ETA（残り約N分）を導出する。now を state に
  // 持つことで、経過は jobClock の起点から、ETA はステージ実進捗アンカーから、同じ now で一貫して
  // 計算できる（起点は jobClock 側に持つので画面遷移で remount してもリセットしない）。
  const [nowMs, setNowMs] = useState(0);

  // 指定テンプレへ preset してモーダルを開く（AIで作成グループ / 再生成 / 空状態 共通）。
  const openModal = (templateId: string) => {
    // A summary sends the transcript and speakers the view holds now; while a job is
    // rewriting them (Issue #102) the result would describe the old speakers.
    if (processing) return;
    setPresetTemplate(templateId);
    setModalOpen(true);
  };

  // 発言単位の話者訂正（Issue #19）。チップを押すと、その発言だけ話者を選び直せる。
  // 改名（onRenamed）がクラスタ全体を変えるのに対し、こちらは 1 行だけを動かす。
  const [fixingSeg, setFixingSeg] = useState<Segment | null>(null);

  // 本文/話者/音声を取り直す（ジョブ完了後の反映用。UI 状態＝タブ等はリセットしない）。
  const reloadDetail = useCallback(() => {
    // 訂正モーダルは押した瞬間の Segment を掴んでいる。再取得で segments が
    // 総入れ替えになると（再文字起こし等）、旧発言の本文を表示したまま新しい idx の
    // 別発言へ書き込む経路ができる。いまは canTranscribe / canDiarize が塞いでいて
    // 到達しないが、条件を緩めた瞬間に開く穴なのでここで捨てる。
    setFixingSeg(null);
    getRecording(id)
      .then((d) => {
        if (d) {
          setDetail(d);
          setJob(d.active_job ?? null);
        }
      })
      .catch(() => {
        /* 再取得失敗は無視（既存表示を保つ） */
      });
    recordingAudioSrc(id)
      .then(setAudioSrc)
      .catch(() => {});
  }, [id]);

  // マウント時の取りこぼし対策（advisor）: 詳細取得〜job://update 購読の隙間に、fast-fail
  // （no_audio / no_transcript 等）の failed が飛ぶと「処理中」で固まる。active_job を
  // 抱えたら一度だけ listJobs で突き合わせて決着を取り直す。
  const reconcileActiveJob = useCallback(
    (jobId: string) => {
      listJobs()
        .then((jobs) => {
          const mine = jobs.find((j) => j.id === jobId);
          // 一覧（pending/running/failed）に居なければ done/canceled で決着済み → 取り直す。
          if (!mine) reloadDetail();
          else setJob(mine);
        })
        .catch(() => {});
    },
    [reloadDetail],
  );

  // job://update 購読。**自分（recording_id===id）宛のみ**反映する（要石: 相関付け）。
  // 完了/失敗トースト・サイドバー更新は App が担うので、ここは自分のビュー更新だけに徹する。
  useJobUpdate((u) => {
    if (u.recording_id !== id) return;
    // タイトル自動生成（Issue #4）は画面の処理中表示に関わらない。付いたら名前だけ取り直す。
    if (u.kind === TITLE_JOB_KIND) {
      if (u.status === "done") {
        getRecording(id)
          .then((d) =>
            setDetail((cur) => (cur && d ? { ...cur, recording: { ...cur.recording, title: d.recording.title } } : cur)),
          )
          .catch(() => {});
      }
      return;
    }
    // 中断を頼んだ直後に処理が終わる・失敗することもある。どの終わり方でも「中断しています…」を外す。
    if (u.status === "done" || u.status === "failed" || u.status === "canceled") setCanceling(false);
    if (u.status === "done") {
      setJob(null);
      setJobProgress({ done: 0, total: null });
      reloadDetail();
    } else if (u.status === "canceled") {
      setJob(null);
      setCanceling(false);
      setJobProgress({ done: 0, total: null });
    } else if (u.status === "failed") {
      setJob((prev) => (prev ? { ...prev, status: "failed", error: u.error } : prev));
    } else {
      // pending / running。stage/進捗を反映（prev が無ければイベントから合成）。
      setJob((prev) => ({
        id: u.job_id,
        recording_id: u.recording_id,
        kind: u.kind,
        status: u.status,
        params: prev?.params ?? { diarize: false, stt_lang: null, transcription_model: "", lang },
        stage: u.stage ?? prev?.stage ?? null,
        error: null,
        created_at: prev?.created_at ?? "",
        updated_at: prev?.updated_at ?? "",
      }));
      setJobProgress({ done: u.done, total: u.total });
    }
  });

  // 経過/ETA タイマー: 1 秒ごとに now を更新するだけ（起点は jobClock が保持）。
  useEffect(() => {
    if (!jobActive || !job) {
      setNowMs(0);
      return;
    }
    markJobStart(job.id); // App 未観測でも自分で種を蒔く（冪等）
    const tick = () => setNowMs(Date.now());
    tick();
    const h = window.setInterval(tick, 1000);
    return () => window.clearInterval(h);
  }, [jobActive, job?.id, job?.stage]);

  // now を起点/アンカーと突き合わせて経過と ETA を導出（render で毎秒再計算）。
  const jobStart = jobActive && job ? getJobStart(job.id) : undefined;
  const elapsedSec = jobStart && nowMs ? Math.max(0, Math.floor((nowMs - jobStart) / 1000)) : 0;
  // ETA は total 付き進捗（＝単一ファイル文字起こしの whisper 0-100%）でのみ出す。会議/話者分離は
  // total が来ない（core が %を出さない）ので自動的に経過時間だけになる。ステージ実進捗アンカーから
  // 線形外挿し、pct が十分に進んで(>5%)かつ未完了のときだけ表示（序盤の暴れと 100% 近傍の 0 を避ける）。
  const pctFrac = jobProgress.total ? jobProgress.done / jobProgress.total : 0;
  const stageStart = jobActive && job?.stage ? getStageStart(job.id, job.stage) : undefined;
  const etaMin =
    stageStart && nowMs && pctFrac > 0.05 && pctFrac < 1
      ? Math.max(1, Math.ceil((((nowMs - stageStart) / 1000) * (1 - pctFrac)) / pctFrac / 60))
      : null;

  // 文字起こしジョブ投入（空 transcript の録音・再文字起こし）。
  const startTranscribe = async () => {
    if (starting || processing) return;
    setStarting(true);
    try {
      const res = await transcribeRecording(id, transcribeDiarize);
      // transcribe_recording は必ずジョブを積む（job_id は非 null）が、型上 optional なので楽観 seed は
      // job_id があるときだけ（無ければ即続く job://update が本物の id で state を埋める）。
      if (res.job_id) {
        setJob({
          id: res.job_id,
          recording_id: id,
          kind: "transcribe",
          status: "pending",
          params: { diarize: transcribeDiarize, stt_lang: null, transcription_model: "", lang },
          stage: null,
          error: null,
          created_at: "",
          updated_at: "",
        });
        setJobProgress({ done: 0, total: null });
      }
    } catch (e) {
      toast(translateError(e, t), "error");
    } finally {
      setStarting(false);
    }
  };

  // 後付け話者分離ジョブ投入（話者未割当の File/Mic、またはやり直し。Issue #102）。
  const startDiarize = async () => {
    if (starting || processing) return;
    setStarting(true);
    try {
      const res = await diarizeRecording(id);
      if (res.job_id) {
        setJob({
          id: res.job_id,
          recording_id: id,
          kind: "diarize",
          status: "pending",
          params: { diarize: false, stt_lang: null, transcription_model: "", lang },
          stage: null,
          error: null,
          created_at: "",
          updated_at: "",
        });
        setJobProgress({ done: 0, total: null });
      }
    } catch (e) {
      toast(translateError(e, t), "error");
    } finally {
      setStarting(false);
    }
  };

  // 順番待ち（pending）のジョブをキャンセル（running は完走）。
  // 実行中の中断（#114）。順番待ちはその場で消え、実行中は処理が止まった時点で
  // job://update（canceled）が届く。それまでは「中断しています…」を出す。
  const onCancelJob = async () => {
    setConfirmCancel(false);
    if (!job) return;
    const running = job.status === "running";
    try {
      const ok = await cancelJob(job.id);
      if (ok && running) {
        setCanceling(true);
      } else if (ok) {
        setJob(null);
        setJobProgress({ done: 0, total: null });
        refreshRecents();
      }
    } catch (e) {
      toast(translateError(e, t), "error");
    }
  };

  // stage キー → 表示名（job.stages。未知キーは transcribe へフォールバック）。
  const stageLabel = (stage: string | null): string =>
    (stage && t.job.stages[stage]) || t.job.stages.transcribe;

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    setTab("transcript");
    setTranslateOn(false);
    setModalOpen(false);
    setAskOpen(false);
    setConfirmDel(false);
    setEditingTitle(false);
    // 訂正モーダルも必ず捨てる。残すと「別録音へ遷移したのにモーダルだけ前の録音の
    // 発言を掴んでいる」状態になり、選んだ瞬間に**別録音の無関係な発言**が書き換わる
    // （話者候補は新録音のものなので、コア側の話者検証も素通りしてしまう）。
    setFixingSeg(null);
    setAudioSrc(null);
    setJob(null);
    setJobProgress({ done: 0, total: null });
    setTranscribeDiarize(false);
    // 再生用の音声 URL（無くても本体表示は止めない・best-effort）。
    recordingAudioSrc(id)
      .then((src) => {
        if (!cancelled) setAudioSrc(src);
      })
      .catch(() => {
        /* 音声が無くても致命的でない */
      });
    getRecording(id)
      .then((d) => {
        if (cancelled) return;
        if (!d) {
          navigate({ view: "history" });
          return;
        }
        setDetail(d);
        setLoading(false);
        // 処理中で開く（active_job）。取りこぼし対策で一度だけ突き合わせる。
        if (d.active_job) {
          setJob(d.active_job);
          reconcileActiveJob(d.active_job.id);
        }
      })
      .catch((e) => {
        if (cancelled) return;
        // 原文（キー）のまま保持し、表示時に translateError する（言語切替にも追従させる）。
        setError(String(e));
        setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [id, navigate, reconcileActiveJob]);

  // 同じテンプレの要約は置換（upsert）。重複カードを増やさない。
  const onCreated = (summary: Summary) =>
    setDetail((prev) => {
      if (!prev) return prev;
      const idx = prev.summaries.findIndex((s) => s.template_id === summary.template_id);
      const summaries =
        idx >= 0
          ? prev.summaries.map((s, i) => (i === idx ? summary : s))
          : [...prev.summaries, summary];
      return { ...prev, summaries };
    });

  const fixSegmentSpeaker = async (segIdx: number, speakerId: string | null) => {
    // detail が prop の id に追いついていない窓（遷移直後）を弾く。
    //
    // ⚠️ **これは id 変更 effect の setFixingSeg(null) の二重化ではない。** 別録音へ遷移すると
    // detail も id も新録音になるので、この条件は成立してしまう。「モーダルが前の録音の発言を
    // 掴んだまま残る」を止めているのは effect 側のリセットだけで、そちらを消すと防御はゼロ。
    // 本当に二重化するなら fixingSeg に録音 id を同梱して比べる必要がある。
    // 選んだ時点でモーダルは必ず閉じる（成否・分岐によらず）。
    setFixingSeg(null);
    if (!detail || detail.recording.id !== id) {
      // 起きないはずの分岐。黙って return すると「二重防御が効いた」のか「壊れた」のか
      // 区別できないので、必ず痕跡を残す。
      console.warn("[mojiroku] 訂正の対象録音が一致しないため中止", {
        expected: id,
        got: detail?.recording.id,
      });
      return;
    }
    let changed: boolean;
    try {
      // 戻り値は「実際に変えたか」。同じ話者を選び直したときは false。
      // ここを見ずに stale を撒くと、DB は無変更なのに画面だけ「要約が古い」になり、
      // 7B モデルでの作り直し（分単位）を無駄に促してしまう。
      changed = await setSegmentSpeaker(id, segIdx, speakerId);
    } catch (e) {
      toast(translateError(e, t), "error");
      return;
    }
    if (!changed) {
      // 同じ話者を選び直した場合。コア側が何も変えていないので、こちらも
      // 要約の stale を撒かない（撒くと 7B 要約の作り直しを無駄に促す）。
      // 「押したのに無反応」に見えないよう、変化が無かったことは伝える。
      toast(t.composite.speakerUnchanged, "info");
      return;
    }
    // 再取得せずローカル state をパッチする（改名・タイトル変更と同じ作法）。
    // await の間に別録音へ遷移していることがあるので、prev 側でも id を確かめる
    // （DB は prop の id で書いているので誤書き込みは起きないが、画面だけズレる）。
    setDetail((prev) =>
      prev && prev.recording.id === id
        ? {
            ...prev,
            transcript: {
              ...prev.transcript,
              segments: prev.transcript.segments.map((x) =>
                x.idx === segIdx ? { ...x, speaker_id: speakerId } : x,
              ),
            },
            // 要約はコア側で stale が立つので、こちらでも印を合わせる。
            summaries: prev.summaries.map((x) => ({ ...x, stale: true })),
          }
        : prev,
    );
    toast(t.composite.speakerFixed, "success");
  };

  const onRenamed = (speakerId: string, displayName: string | null) =>
    setDetail((prev) =>
      prev
        ? {
            ...prev,
            speakers: prev.speakers?.map((s) =>
              s.id === speakerId ? { ...s, display_name: displayName } : s,
            ),
          }
        : prev,
    );

  // 削除（取り消し不可 → 確認後）。成功したら履歴へ戻り、サイドバー最近も更新。
  const onDelete = async () => {
    setDeleting(true);
    try {
      await deleteRecording(id);
      refreshRecents();
      toast(t.history.deleted, "success");
      navigate({ view: "history" });
    } catch (e) {
      toast(translateError(e, t), "error");
      setDeleting(false);
    }
  };

  // タイトル編集開始（現在の生タイトルを初期値に）。
  const beginEditTitle = () => {
    setTitleValue(detail?.recording.title ?? "");
    setEditingTitle(true);
  };

  // 文字起こしからタイトルを生成（Issue #4）。クラウド設定なら送信前に確認する。
  const requestGenerateTitle = async () => {
    if (generatingTitle) return;
    let engine: string = "cloud";
    try {
      engine = (await getSettings()).engine;
    } catch {
      // 読めないときはクラウド扱い（確認を出す）。
    }
    if (engine === "cloud") setConfirmCloudTitle(true);
    else void runGenerateTitle();
  };
  const runGenerateTitle = async () => {
    setConfirmCloudTitle(false);
    setGeneratingTitle(true);
    try {
      const next = await generateTitle(id);
      setDetail((d) => (d ? { ...d, recording: { ...d.recording, title: next } } : d));
      refreshRecents();
      toast(t.history.titleGenerated, "success");
    } catch (e) {
      toast(translateError(e, t), "error");
    } finally {
      setGeneratingTitle(false);
    }
  };

  // タイトル保存（null/空白で既定の「無題」へ）。成功したら表示とサイドバー最近を更新。
  const saveTitle = async () => {
    if (savingTitle) return;
    setSavingTitle(true);
    try {
      await renameRecording(id, titleValue);
      const next = titleValue.trim() || null;
      setDetail((d) => (d ? { ...d, recording: { ...d.recording, title: next } } : d));
      setEditingTitle(false);
      refreshRecents();
      toast(t.history.renamed, "success");
    } catch (e) {
      toast(translateError(e, t), "error");
    } finally {
      setSavingTitle(false);
    }
  };

  if (loading) {
    return (
      <div className="flex min-h-full items-center justify-center">
        <Spinner size={22} />
      </div>
    );
  }

  if (error || !detail) {
    return (
      <div className="flex min-h-full flex-col items-center justify-center gap-3 px-8 text-center">
        <div className="text-[14px] text-sub">{t.detail.loadFailed}</div>
        {error && (
          <div className="max-w-md text-[12px] text-muted">{translateError(error, t)}</div>
        )}
        <button
          onClick={() => navigate({ view: "history" })}
          className="mt-1 text-[13px] text-brand-light hover:text-brand-lighter"
        >
          {t.detail.backToHistory}
        </button>
      </div>
    );
  }

  const rec = detail.recording;
  const title = recordingTitle(rec, lang);
  const meta = [formatDateTime(rec.created_at, lang), formatDuration(rec.duration_ms)];
  const speakers = detail.speakers ?? [];
  const hasTranscript = detail.transcript.segments.length > 0;
  // 再生中の行（二分探索なので毎回求めてよい）。一度も再生していない間は強調しない。
  const activeIdx =
    audioSrc && (playing || playMs > 0) ? segmentAt(detail.transcript.segments, playMs) : null;
  // 要約・議事録は文字起こしがあって処理中でないときだけ作れる（#107）。押せない理由は title で示す。
  const canSummarize = hasTranscript && !processing;
  const summarizeBlockedReason = canSummarize
    ? undefined
    : processing
      ? t.detail.needsJobDone
      : t.detail.needsTranscript;
  // 後付けアクションの可否（ADR-0024）。処理中は隠す。会議は録音時に話者付与済み＝diarize 不可。
  const canTranscribe = !processing && !hasTranscript;
  const canDiarize =
    !processing && hasTranscript && speakers.length === 0 && rec.source_type !== "live";
  // Existing speakers can be re-analysed, including meetings (remote track only; Issue #102).
  // A meeting with no speaker rows (its earlier run found no remote turns) can also retry.
  const canRediarize =
    !processing && hasTranscript && (speakers.length > 0 || rec.source_type === "live");

  return (
    <div className="flex h-full min-h-0">
      {/* 中央メイン */}
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        <header className="shrink-0 border-b border-line px-6 pb-3.5 pt-[18px]">
          <div className="flex items-start justify-between gap-4">
            <div className="min-w-0">
              {editingTitle ? (
                <div className="flex items-center gap-2">
                  <input
                    autoFocus
                    value={titleValue}
                    onChange={(e) => setTitleValue(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") {
                        e.preventDefault();
                        void saveTitle();
                      } else if (e.key === "Escape") {
                        e.preventDefault();
                        setEditingTitle(false);
                      }
                    }}
                    placeholder={recordingTitle({ ...rec, title: null }, lang)}
                    className="min-w-0 flex-1 rounded-tag border border-border-3 bg-surface-2 px-2.5 py-1 text-[18px] font-bold text-ink outline-none focus:border-brand"
                  />
                  <button
                    onClick={() => void saveTitle()}
                    disabled={savingTitle}
                    aria-label={t.common.save}
                    title={t.common.save}
                    className="flex h-7 w-7 shrink-0 items-center justify-center rounded-tag text-green transition-colors hover:bg-surface-2 disabled:opacity-50"
                  >
                    {savingTitle ? <Spinner size={14} /> : <CheckIcon size={16} />}
                  </button>
                  <button
                    onClick={() => setEditingTitle(false)}
                    disabled={savingTitle}
                    aria-label={t.common.cancel}
                    title={t.common.cancel}
                    className="flex h-7 w-7 shrink-0 items-center justify-center rounded-tag text-dim transition-colors hover:bg-surface-2 hover:text-body disabled:opacity-50"
                  >
                    <XIcon size={15} />
                  </button>
                </div>
              ) : (
                <div className="group/title flex items-center gap-2">
                  <h1 className="truncate text-[18px] font-bold text-ink">{title}</h1>
                  <button
                    onClick={beginEditTitle}
                    aria-label={t.history.renameTitle}
                    title={t.history.renameTitle}
                    className="flex h-7 w-7 shrink-0 items-center justify-center rounded-tag text-dim opacity-0 transition-all hover:bg-surface-2 hover:text-body group-hover/title:opacity-100"
                  >
                    <PencilIcon size={14} />
                  </button>
                  {hasTranscript && !processing && (
                    <button
                      onClick={() => void requestGenerateTitle()}
                      disabled={generatingTitle}
                      aria-label={t.history.generateTitle}
                      title={t.history.generateTitle}
                      className={cx(
                        "flex h-7 w-7 shrink-0 items-center justify-center rounded-tag text-dim transition-all hover:bg-surface-2 hover:text-body",
                        generatingTitle ? "opacity-100" : "opacity-0 group-hover/title:opacity-100",
                      )}
                    >
                      {generatingTitle ? <Spinner size={14} /> : <SparklesIcon size={14} />}
                    </button>
                  )}
                </div>
              )}
              <div className="mt-0.5 font-mono text-[12px] text-faint">{meta.join(" · ")}</div>
            </div>
            <div className="flex shrink-0 items-center gap-2">
              {/* 質問する = ローカル RAG（未実装）のモック。配布時(MOCK_PREVIEW=false)は隠す。 */}
              {MOCK_PREVIEW && (
                <button
                  onClick={() => setAskOpen(true)}
                  className="inline-flex h-7 items-center gap-1.5 rounded-tag border border-border-3 bg-surface-2 px-2.5 text-[12px] text-body transition-colors hover:bg-hover"
                >
                  <SparklesIcon size={13} className="text-brand-lighter" />
                  質問する
                </button>
              )}
              <SharePopover detail={detail} disabled={!hasTranscript && detail.summaries.length === 0} />
              <button
                onClick={() => setConfirmDel(true)}
                aria-label={t.common.delete}
                title={t.common.delete}
                className="inline-flex h-7 w-7 items-center justify-center rounded-tag text-dim transition-colors hover:bg-red/12 hover:text-red-light"
              >
                <TrashIcon size={15} />
              </button>
            </div>
          </div>

          {/* 再生バー。原本があれば実再生（File/Mic/会議の結合 <id>.wav）、無ければ控えめな装飾。 */}
          <div className="mt-3.5">
            {audioSrc ? (
              <AudioPlayer
                ref={playerRef}
                src={audioSrc}
                fallbackDurationMs={rec.duration_ms}
                onTime={setPlayMs}
                onPlayingChange={setPlaying}
              />
            ) : (
              <>
                <div className="flex cursor-default items-center gap-3">
                  <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full border border-border-2 bg-surface-2 text-muted">
                    <PlayIcon size={15} />
                  </div>
                  <Waveform active={false} bars={30} height={42} className="flex-1" />
                  <span className="shrink-0 font-mono text-[12px] text-muted tnum">
                    0:00 / {formatDuration(rec.duration_ms)}
                  </span>
                </div>
                <div className="mt-1 text-right text-[11px] text-dim">
                  {t.detail.noAudio}
                </div>
              </>
            )}
          </div>
        </header>

        <div
          ref={scrollRef}
          className="relative flex-1 overflow-y-auto px-6 py-4"
          // 利用者が自分でスクロールしたら追従をやめる。自動追従のスクロールは autoScrollUntil で除く。
          onScroll={() => {
            if (playing && Date.now() > autoScrollUntil.current) setFollowing(false);
          }}
        >
          {/* 処理中（ADR-0024）: ステージ + 進捗。pending はキャンセル可（running は完走）。 */}
          {processing && job && (
            <div className="mb-4 rounded-card border border-border-2 bg-surface-2 px-4 py-3.5">
              <div className="flex items-center gap-3">
                <Spinner size={18} />
                <div className="min-w-0 flex-1">
                  <div className="text-[13px] font-semibold text-ink">{t.job.processing}</div>
                  <div className="mt-0.5 text-[12px] text-muted">
                    {/* pending（まだワーカーが取っていない＝重い処理は同時 1 本なので前段の完了待ち）と
                        running+stage=queued（permit 待ち）は「順番待ち」。stage=null の pending を
                        stageLabel の transcribe フォールバックで「文字起こし」と誤表示しない（ADR-0024）。 */}
                    {job.status === "pending" || job.stage === "queued"
                      ? t.job.queued
                      : stageLabel(job.stage)}
                    {jobProgress.total
                      ? ` · ${Math.min(100, Math.round((jobProgress.done / jobProgress.total) * 100))}%`
                      : ""}
                    {jobActive ? ` · ${t.job.elapsed} ${formatDuration(elapsedSec * 1000)}` : ""}
                    {etaMin != null ? ` · ${t.job.remaining(etaMin)}` : ""}
                  </div>
                </div>
                <button
                  onClick={() => (job.status === "pending" ? void onCancelJob() : setConfirmCancel(true))}
                  disabled={canceling}
                  className="shrink-0 rounded-tag border border-border-3 px-2.5 py-1 text-[12px] text-muted transition-colors hover:bg-hover hover:text-body disabled:opacity-60"
                >
                  {canceling ? t.job.canceling : t.job.cancel}
                </button>
              </div>
              {jobProgress.total ? (
                <div className="mt-2.5 h-1 overflow-hidden rounded-full bg-border-2">
                  <div
                    className="h-full rounded-full bg-brand transition-[width]"
                    style={{
                      width: `${Math.min(100, Math.round((jobProgress.done / jobProgress.total) * 100))}%`,
                    }}
                  />
                </div>
              ) : null}
            </div>
          )}

          {/* 失敗（ADR-0024）: キー化メッセージを翻訳表示。下の実行ボタンで再試行できる。 */}
          {/* 失敗はここに 1 回だけ出す（この画面を開いている間は App のトーストを出さない・#107）。 */}
          {jobFailed && job && (
            <div
              role="alert"
              className="mb-4 flex items-center justify-between gap-3 rounded-card border border-red/40 bg-red/8 px-4 py-3"
            >
              <span className="text-[13px] text-red-light">
                {job.error ? translateError(job.error, t) : t.job.failedToast}
              </span>
              {/* 文字起こしの失敗は下の「文字起こしを実行」がそのまま再試行になるので、ここには出さない。 */}
              {!canTranscribe && (
                <button
                  onClick={() => void (job.kind === "diarize" ? startDiarize() : startTranscribe())}
                  disabled={starting}
                  className="h-8 shrink-0 rounded-ctl border border-red/40 px-3 text-[12px] font-medium text-red-light transition-colors hover:bg-red/12 disabled:opacity-50"
                >
                  {t.common.retry}
                </button>
              )}
            </div>
          )}

          {/* 後付け文字起こし（空 transcript の録音）。 */}
          {canTranscribe && (
            <div className="mb-4 rounded-card border border-border-2 bg-surface-2 px-4 py-4">
              <div className="text-[14px] font-semibold text-ink">{t.detail.runTranscribe}</div>
              <div className="mt-0.5 text-[12px] text-muted">{t.detail.runTranscribeDesc}</div>
              <div className="mt-3 flex items-center gap-2 text-[12px] text-body">
                <Toggle
                  checked={transcribeDiarize}
                  onChange={setTranscribeDiarize}
                  label={t.detail.runTranscribeDiarize}
                />
                <span>{t.detail.runTranscribeDiarize}</span>
              </div>
              <button
                onClick={() => void startTranscribe()}
                disabled={starting}
                className="mt-3 h-9 rounded-ctl bg-brand px-4 text-[13px] font-semibold text-white transition-[filter] hover:brightness-110 disabled:opacity-50"
              >
                {starting ? <Spinner size={14} /> : t.detail.runTranscribe}
              </button>
            </div>
          )}

          {/* 後付け話者分離（transcript 済み・話者未割当の File/Mic）。 */}
          {canDiarize && (
            <div className="mb-4 flex items-center justify-between gap-3 rounded-card border border-border-2 bg-surface-2 px-4 py-3.5">
              <div className="min-w-0">
                <div className="text-[13px] font-semibold text-ink">{t.detail.runDiarize}</div>
                <div className="mt-0.5 text-[12px] text-muted">{t.detail.runDiarizeDesc}</div>
              </div>
              <button
                onClick={() => void startDiarize()}
                disabled={starting}
                className="inline-flex h-9 shrink-0 items-center gap-1.5 rounded-ctl border border-border-2 px-3.5 text-[13px] font-medium text-body transition-colors hover:bg-hover disabled:opacity-50"
              >
                {starting ? <Spinner size={14} /> : t.detail.runDiarize}
              </button>
            </div>
          )}

          {/* AI議事録。文字起こしが無い間は作れないので、作成の案内も出さない（#107）。 */}
          {detail.summaries.length > 0 ? (
            <div className="mb-4 flex flex-col gap-3">
              {detail.summaries.map((s, i) => (
                <div
                  key={i}
                  className="rounded-card border border-border-2 border-l-[3px] border-l-brand bg-surface-2 px-[17px] py-[15px]"
                >
                  <div className="mb-2 flex items-center justify-between gap-2">
                    <span className="flex min-w-0 items-center gap-2">
                      <span className="truncate text-[12px] font-bold tracking-[0.03em] text-brand-light">
                        {t.detail.summaryCardLabel(templateLabel(s.template_id))}
                      </span>
                      {/* 後付け話者分離などで古くなった要約に注意（ADR-0024）。 */}
                      {s.stale && (
                        <span
                          title={t.detail.summaryStaleTitle}
                          className="shrink-0 rounded-full bg-amber/15 px-2 py-0.5 text-[11px] font-medium text-amber"
                        >
                          {t.detail.summaryStale}
                        </span>
                      )}
                    </span>
                    <button
                      onClick={() => openModal(s.template_id)}
                      disabled={processing}
                      className="inline-flex disabled:opacity-50 shrink-0 items-center gap-1 text-[11px] text-dim transition-colors hover:text-sub"
                      title={t.detail.regenerate}
                    >
                      <RefreshIcon size={13} />
                      {t.detail.regenerate}
                    </button>
                  </div>
                  <Markdown text={s.content} />
                  {s.action_items.length > 0 && (
                    <ul className="mt-3 flex flex-col gap-1.5">
                      {s.action_items.map((a, j) => (
                        <li key={j} className="flex gap-2 text-[13px] text-body">
                          <CheckIcon size={14} className="mt-0.5 shrink-0 text-green" />
                          <span>
                            {a.text}
                            {a.assignee ? ` — ${a.assignee}` : ""}
                            {a.due ? `（${a.due}）` : ""}
                          </span>
                        </li>
                      ))}
                    </ul>
                  )}
                </div>
              ))}
            </div>
          ) : hasTranscript && !processing ? (
            <button
              onClick={() => openModal("minutes")}
              className="mb-4 flex w-full items-center gap-3 rounded-card border border-dashed border-border-3 bg-surface-2 px-4 py-4 text-left transition-colors hover:bg-hover"
            >
              <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-ctl bg-brand/15 text-brand-lighter">
                <SparklesIcon size={17} />
              </span>
              <span className="min-w-0">
                <span className="block text-[14px] font-semibold text-ink">
                  {t.detail.createMinutesCta}
                </span>
                <span className="mt-0.5 block text-[12px] text-muted">
                  {t.detail.createMinutesCtaDesc}
                </span>
              </span>
            </button>
          ) : null}

          {/* シリーズ横断ダイジェスト（モック画面へ）。配布時は隠す。 */}
          {MOCK_PREVIEW && (
            <button
              onClick={() => navigate({ view: "digest" })}
              className="mb-4 inline-flex items-center gap-1 text-[12px] text-brand-light transition-colors hover:text-brand-lighter"
            >
              シリーズの横断ダイジェスト
              <ArrowUpRightIcon size={13} />
            </button>
          )}

          {/* タブ */}
          <div className="mb-3 flex items-center border-b border-line">
            <TabButton active={tab === "transcript"} onClick={() => setTab("transcript")}>
              {t.detail.tabs.transcript}
            </TabButton>
            {/* ライブ翻訳は会議モードにしか無い（#107）。他の録音では空のタブになるだけなので出さない。 */}
            {rec.source_type === "live" && (
              <TabButton active={tab === "translations"} onClick={() => setTab("translations")}>
                {t.meeting.translation.savedTitle}
              </TabButton>
            )}
            {hasTranscript && (
              <button
                onClick={() => {
                  setSearchOpen(true);
                  setTab("transcript");
                  requestAnimationFrame(() => searchInputRef.current?.focus());
                }}
                aria-label={t.detail.find.open}
                title={`${t.detail.find.open}（⌘F）`}
                className="ml-auto mb-1 rounded-tag p-1.5 text-muted transition-colors hover:bg-hover hover:text-ink"
              >
                <SearchIcon size={14} />
              </button>
            )}
            {/* チャプター/翻訳はモック（未実装）。配布時(MOCK_PREVIEW=false)は丸ごと隠し、
                実録音に固定ダミーが出ないようにする。 */}
            {MOCK_PREVIEW && (
              <TabButton active={tab === "chapters"} onClick={() => setTab("chapters")}>
                チャプター
              </TabButton>
            )}
            {tab === "transcript" && MOCK_PREVIEW && (
              <div className="ml-auto flex items-center gap-2 pb-1.5">
                <PreviewTag />
                <span className="text-[12px] text-muted">日本語に翻訳</span>
                <Toggle
                  checked={translateOn}
                  onChange={setTranslateOn}
                  label="日本語に翻訳"
                />
              </div>
            )}
          </div>

          {searchOpen && tab === "transcript" && (
            <div className="sticky top-0 z-10 -mx-1 mb-2 flex items-center gap-2 rounded-ctl border border-border-2 bg-surface px-2.5 py-1.5 shadow-pop">
              <SearchIcon size={14} className="shrink-0 text-muted" />
              <input
                ref={searchInputRef}
                value={query}
                onChange={(e) => {
                  setQuery(e.target.value);
                  setMatchPos(0);
                }}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    moveMatch(e.shiftKey ? -1 : 1);
                  } else if (e.key === "Escape") {
                    e.preventDefault();
                    closeSearch();
                  }
                }}
                placeholder={t.detail.find.placeholder}
                aria-label={t.detail.find.placeholder}
                className="min-w-0 flex-1 bg-transparent text-[13px] text-ink placeholder:text-dim"
              />
              <span className="shrink-0 font-mono text-[11px] text-muted tnum" aria-live="polite">
                {q ? t.detail.find.count(matches.length ? Math.min(matchPos, matches.length - 1) + 1 : 0, matches.length) : ""}
              </span>
              <button
                onClick={() => moveMatch(-1)}
                disabled={!matches.length}
                aria-label={t.detail.find.prev}
                title={`${t.detail.find.prev}（Shift+Enter）`}
                className="rounded-tag px-1.5 py-0.5 text-[12px] text-sub hover:bg-hover disabled:opacity-40"
              >
                ↑
              </button>
              <button
                onClick={() => moveMatch(1)}
                disabled={!matches.length}
                aria-label={t.detail.find.next}
                title={`${t.detail.find.next}（Enter）`}
                className="rounded-tag px-1.5 py-0.5 text-[12px] text-sub hover:bg-hover disabled:opacity-40"
              >
                ↓
              </button>
              <button
                onClick={closeSearch}
                aria-label={t.common.close}
                className="rounded-tag p-1 text-muted hover:bg-hover hover:text-ink"
              >
                <XIcon size={13} />
              </button>
            </div>
          )}

          {tab === "translations" ? <SavedTranslations key={id} id={id} /> : tab === "chapters" && MOCK_PREVIEW ? (
            <div>
              <div className="mb-3 flex items-center justify-between">
                <div className="flex items-center gap-2">
                  <span className="text-[13px] text-sub">{CHAPTERS.length} チャプター</span>
                  <PreviewTag />
                </div>
                <span className="inline-flex items-center gap-1.5 rounded-tag bg-brand/14 px-2.5 py-1 text-[11px] text-brand-lighter">
                  <SparklesIcon size={12} />
                  AIが自動生成
                </span>
              </div>
              {/* タイムラインバー */}
              <div className="flex h-[9px] gap-[3px] overflow-hidden rounded-tag">
                {CHAPTERS.map((c) => (
                  <div
                    key={c.time}
                    className="rounded-tag"
                    style={{ flexGrow: c.grow, background: c.color }}
                  />
                ))}
              </div>
              <div className="mb-5 mt-1.5 flex justify-between font-mono text-[11px] text-dim">
                <span>00:00</span>
                <span>{formatDuration(rec.duration_ms)}</span>
              </div>
              <div className="flex flex-col gap-2.5">
                {CHAPTERS.map((c) => (
                  <div
                    key={c.time}
                    className="flex gap-3.5 rounded-card border border-border bg-surface px-4 py-3.5"
                  >
                    <div className="shrink-0 text-center">
                      <span className="font-mono text-[12px]" style={{ color: c.color }}>
                        {c.time}
                      </span>
                      <div
                        className="mx-auto mt-1.5 h-[9px] w-[9px] rounded-full"
                        style={{ background: c.color }}
                      />
                    </div>
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center justify-between gap-2">
                        <span className="text-[14px] font-bold text-ink">{c.title}</span>
                        <span className="shrink-0 text-[11px] text-dim">{c.dur}</span>
                      </div>
                      <div className="mt-1.5 text-[12px] leading-relaxed text-muted">{c.body}</div>
                    </div>
                  </div>
                ))}
              </div>
            </div>
          ) : detail.transcript.segments.length > 0 ? (
            <TranscriptList
              segments={detail.transcript.segments}
              speakers={detail.speakers}
              translate={
                translateOn && MOCK_PREVIEW ? () => MOCK_TRANSLATION : undefined
              }
              // 話者が 1 人も居ない録音（話者分離していない）では訂正の選択肢が無いので出さない。
              // 処理中（話者の再検出など）は、ジョブ完了で上書きされる訂正を受け付けない（Issue #102）。
              onSpeakerClick={
                speakers.length > 0 && !processing ? setFixingSeg : undefined
              }
              activeIdx={activeIdx}
              onSeek={audioSrc ? seekToSegment : undefined}
              query={searchOpen ? query : ""}
              currentMatchIdx={currentMatchIdx}
            />
          ) : processing ? (
            <EmptyState title={t.detail.transcriptPendingTitle} hint={t.detail.transcriptPendingHint} />
          ) : (
            <EmptyState title={t.detail.noTranscriptTitle} hint={t.detail.noTranscriptHint} />
          )}

          {/* 追従をやめている間だけ、再生位置へ戻るボタンを下に浮かべる。 */}
          {playing && !following && tab === "transcript" && (
            <div className="pointer-events-none sticky bottom-2 flex justify-center">
              <button
                onClick={() => setFollowing(true)}
                className="pointer-events-auto rounded-full border border-border-3 bg-popover px-3.5 py-1.5 text-[12px] font-medium text-body shadow-pop transition-colors hover:bg-hover"
              >
                {t.detail.audio.follow}
              </button>
            </div>
          )}
        </div>
      </div>

      {/* 右ペイン */}
      <aside className="flex w-[222px] shrink-0 flex-col gap-4 overflow-y-auto border-l border-border bg-surface px-[15px] py-4">
        {/* AIで作成（常設）。話者が多くても押し出されないよう、話者一覧より上に置く（#115）。 */}
        <div>
          <div className="mb-2.5 text-[11px] font-bold tracking-[0.08em] text-dim">
            {t.detail.aiCreate}
          </div>
          {summarizeBlockedReason && (
            <p className="mb-2 text-[12px] text-muted">{summarizeBlockedReason}</p>
          )}
          <div className="flex flex-col gap-1.5">
            {/* 主ボタンは「まだ議事録が無い」ときだけ。作成済みなら他と同じ副ボタンに下げ、
                画面の主役を本文に譲る（#106）。 */}
            <button
              onClick={() => openModal("minutes")}
              disabled={!canSummarize}
              title={summarizeBlockedReason}
              className={cx(
                "h-9 w-full rounded-ctl text-[13px] transition-colors disabled:opacity-50",
                findSummary(detail.summaries, "minutes")
                  ? "border border-border-2 text-body hover:bg-hover"
                  : "bg-brand-2 font-semibold text-white hover:brightness-110",
              )}
            >
              {t.detail.createMinutes}
            </button>
            <button
              onClick={() => openModal("summary")}
              disabled={!canSummarize}
              title={summarizeBlockedReason}
              className="h-9 w-full disabled:opacity-50 rounded-ctl border border-border-2 text-[12px] text-body transition-colors hover:bg-hover"
            >
              {t.detail.createSummary}
            </button>
            <button
              onClick={() => openModal("action_items")}
              disabled={!canSummarize}
              title={summarizeBlockedReason}
              className="h-9 w-full disabled:opacity-50 rounded-ctl border border-border-2 text-[12px] text-body transition-colors hover:bg-hover"
            >
              {t.detail.createActionItems}
            </button>
          </div>
        </div>

        {speakers.length > 0 && (
          // Renames and library links made during a re-detection would be replaced when it
          // finishes, so the panel is inert while a job runs (Issue #102).
          <div inert={processing} className={processing ? "opacity-60" : undefined}>
            <SpeakerPanel speakers={speakers} recordingId={id} onRenamed={onRenamed} />
          </div>
        )}
        {canRediarize && (
          <button
            onClick={() => void startDiarize()}
            disabled={starting}
            title={t.detail.rerunDiarizeDesc}
            className="-mt-2 inline-flex h-8 items-center justify-center gap-1.5 rounded-ctl border border-border-2 px-3 text-[12px] font-medium text-body transition-colors hover:bg-hover disabled:opacity-50"
          >
            {starting ? <Spinner size={13} /> : t.detail.rerunDiarize}
          </button>
        )}

        <div className="mt-auto rounded-card border border-border bg-surface-2 px-3 py-2.5">
          <div className="flex items-start gap-2">
            <BotIcon size={14} className="mt-px shrink-0 text-brand-light" />
            <span className="text-[11px] leading-relaxed text-sub">{t.detail.mcpNote}</span>
          </div>
        </div>
      </aside>

      <TemplateModal
        key={`tpl-${id}`}
        open={modalOpen}
        onClose={() => setModalOpen(false)}
        recordingId={id}
        transcript={detail.transcript}
        onCreated={onCreated}
        presetTemplate={presetTemplate}
        replaces={!!findSummary(detail.summaries, presetTemplate)}
      />
      <AskDrawer key={`ask-${id}`} open={askOpen} onClose={() => setAskOpen(false)} title={title} />
      <ConfirmDialog
        open={confirmCancel}
        title={t.job.cancelConfirmTitle}
        body={t.job.cancelConfirmBody}
        confirmLabel={t.job.cancelConfirm}
        onConfirm={() => void onCancelJob()}
        onCancel={() => setConfirmCancel(false)}
      />
      <ConfirmDialog
        open={confirmCloudTitle}
        title={t.history.generateTitleCloudTitle}
        body={t.history.generateTitleCloudBody}
        confirmLabel={t.history.generateTitleCloudConfirm}
        tone="primary"
        onConfirm={() => void runGenerateTitle()}
        onCancel={() => setConfirmCloudTitle(false)}
      />
      <ConfirmDialog
        open={confirmDel}
        title={t.history.deleteConfirmTitle}
        body={t.history.deleteConfirmBody(title)}
        busy={deleting}
        onConfirm={onDelete}
        onCancel={() => setConfirmDel(false)}
      />

      {/* 発言単位の話者訂正（Issue #19）。押した発言だけを別の話者へ移す。 */}
      <Modal open={!!fixingSeg} onClose={() => setFixingSeg(null)} width={380}>
        {fixingSeg && (
          <>
            <ModalHeader
              title={t.composite.fixSpeakerHeading}
              onClose={() => setFixingSeg(null)}
            />
            <div className="px-5 py-4">
              {/* どの発言を直そうとしているかを示す（押し間違いに気づけるように）。 */}
              <p className="mb-3 rounded-btn bg-surface-2 px-3 py-2 text-[13px] leading-6 text-sub">
                <span className="mr-2 font-mono text-[11px] text-dim tnum">
                  {formatTimestamp(fixingSeg.start_ms)}
                </span>
                {fixingSeg.text}
              </p>
              <div className="flex flex-col gap-0.5">
                {speakers.map((sp) => (
                  <MenuItem
                    key={sp.id}
                    onClick={() => void fixSegmentSpeaker(fixingSeg.idx, sp.id)}
                    icon={
                      <span
                        className="inline-block h-2.5 w-2.5 rounded-full"
                        style={{ background: speakerChipStyle(sp.id).color }}
                      />
                    }
                    hint={sp.id === fixingSeg.speaker_id ? "✓" : undefined}
                  >
                    {speakerName(sp.id, speakers, lang)}
                  </MenuItem>
                ))}
                <MenuItem
                  onClick={() => void fixSegmentSpeaker(fixingSeg.idx, null)}
                  hint={fixingSeg.speaker_id === null ? "✓" : undefined}
                >
                  {t.composite.fixSpeakerToUnknown}
                </MenuItem>
              </div>
            </div>
          </>
        )}
      </Modal>
    </div>
  );
}
