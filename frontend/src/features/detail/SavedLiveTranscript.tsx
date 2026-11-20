import { useEffect, useState } from "react";
import { useI18n } from "@/i18n";
import { listLiveCaptions } from "@/lib/tauri";
import type { SavedLiveCaption } from "@/lib/types";

export function SavedLiveCaptionRows({ rows }: { rows: SavedLiveCaption[] }) {
  const { t } = useI18n();
  const copy = t.detail.liveTranscript;
  return <div>
    <p className="mb-4 text-[11px] text-muted">{copy.hint}</p>
    {rows.length === 0 && <p className="text-[13px] text-muted">{copy.empty}</p>}
    {rows.map((row) => <p key={row.caption_id} className="border-b border-line py-2.5 whitespace-pre-wrap text-[14px] leading-relaxed text-speech">
      {row.text}
      {!row.committed && <span className="ml-2 text-[11px] text-faint">{copy.unfinished}</span>}
    </p>)}
  </div>;
}

/** The live transcript saved with a meeting; read-only and separate from the final transcript (ADR-0048). */
export function SavedLiveTranscript({ id }: { id: string }) {
  const { t } = useI18n();
  const [rows, setRows] = useState<SavedLiveCaption[] | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let active = true;
    setRows(null);
    setFailed(false);
    listLiveCaptions(id).then((value) => { if (active) setRows(value); })
      .catch(() => { if (active) setFailed(true); });
    return () => { active = false; };
  }, [id]);
  if (failed) return <p role="alert" className="text-[12px] text-amber">{t.detail.liveTranscript.failed}</p>;
  if (rows === null) return <p role="status" className="text-[12px] text-muted">{t.common.loading}</p>;
  return <SavedLiveCaptionRows rows={rows} />;
}
