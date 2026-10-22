"""Score speaker order and attribution around turn changes (Issue #65).

Compares the app's speaker-labelled transcript segments against the exact turns
written by make_dialogue.py.

Metrics
- straddling segments: segments that cover two or more reference turns by at
  least MIN_OVERLAP each, i.e. one line holding "A's tail + B's reply".
- mislabelled time: share of reference speech whose segment carries the wrong
  speaker (predicted ids are mapped to A/B by largest total overlap).
- turn order: edit distance between the reference speaker sequence and the
  predicted one, after merging consecutive same-speaker lines, divided by the
  reference length (0 = same order).
- short replies: reference turns under 1.2 s that got their own line with the
  right speaker.
- text recall: share of reference characters found, in order, in the output
  (punctuation and spaces ignored).

Usage:
    python3 eval/diarization/turns/score.py out/ja.json out/ja.pred.json [...]
    (pairs of reference and prediction files)
"""
from __future__ import annotations

import difflib
import json
import sys
import unicodedata
from collections import defaultdict

MIN_OVERLAP = 0.2  # seconds a segment must cover of each of two turns to count as straddling
SHORT_TURN = 1.2


def overlap(a0: float, a1: float, b0: float, b1: float) -> float:
    return max(0.0, min(a1, b1) - max(a0, b0))


def norm(text: str) -> str:
    return "".join(
        c.lower()
        for c in unicodedata.normalize("NFKC", text)
        if not unicodedata.category(c).startswith(("P", "Z", "S"))
    )


def edit_distance(a: list[str], b: list[str]) -> int:
    prev = list(range(len(b) + 1))
    for i, x in enumerate(a, 1):
        cur = [i]
        for j, y in enumerate(b, 1):
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (x != y)))
        prev = cur
    return prev[-1]


def score(ref_path: str, pred_path: str) -> dict:
    turns = json.load(open(ref_path))["turns"]
    segs = [
        {"start": s["start_ms"] / 1000, "end": s["end_ms"] / 1000, "spk": s.get("speaker_id"), "text": s["text"]}
        for s in json.load(open(pred_path))
    ]

    # Map predicted speaker ids to reference speakers by largest total overlap.
    ov = defaultdict(lambda: defaultdict(float))
    for s in segs:
        for t in turns:
            ov[s["spk"]][t["speaker"]] += overlap(s["start"], s["end"], t["start"], t["end"])
    mapping = {p: max(r, key=r.get) for p, r in ov.items() if p is not None and r and max(r.values()) > 0}
    for s in segs:
        s["ref_spk"] = mapping.get(s["spk"])

    straddling = 0
    for s in segs:
        covered = [t for t in turns if overlap(s["start"], s["end"], t["start"], t["end"]) >= MIN_OVERLAP]
        if len({id(t) for t in covered}) >= 2:
            straddling += 1

    speech = sum(t["end"] - t["start"] for t in turns)
    wrong = 0.0
    for t in turns:
        for s in segs:
            o = overlap(s["start"], s["end"], t["start"], t["end"])
            if o > 0 and s["ref_spk"] != t["speaker"]:
                wrong += o

    def runs(labels: list[str | None]) -> list[str]:
        out: list[str] = []
        for x in labels:
            x = x or "?"
            if not out or out[-1] != x:
                out.append(x)
        return out

    ref_runs = runs([t["speaker"] for t in turns])
    pred_runs = runs([s["ref_spk"] for s in sorted(segs, key=lambda s: s["start"])])
    order_err = edit_distance(ref_runs, pred_runs) / len(ref_runs)

    short = [t for t in turns if t["end"] - t["start"] < SHORT_TURN]
    short_ok = 0
    for t in short:
        own = [
            s
            for s in segs
            if overlap(s["start"], s["end"], t["start"], t["end"]) >= 0.5 * (t["end"] - t["start"])
            and sum(overlap(s["start"], s["end"], u["start"], u["end"]) for u in turns if u is not t) < MIN_OVERLAP
        ]
        if any(s["ref_spk"] == t["speaker"] for s in own):
            short_ok += 1

    ref_text = norm("".join(t["text"] for t in turns))
    out_text = norm("".join(s["text"] for s in sorted(segs, key=lambda s: s["start"])))
    matched = sum(b.size for b in difflib.SequenceMatcher(None, ref_text, out_text, autojunk=False).get_matching_blocks())

    return {
        "segments": len(segs),
        "straddling": straddling,
        "mislabelled_time": round(wrong / speech, 3),
        "turn_order_error": round(order_err, 3),
        "short_replies_ok": f"{short_ok}/{len(short)}",
        "text_recall": round(matched / max(1, len(ref_text)), 3),
        "speakers_found": len({s["spk"] for s in segs if s["spk"]}),
    }


if __name__ == "__main__":
    args = sys.argv[1:]
    if not args or len(args) % 2:
        sys.exit(__doc__)
    for ref, pred in zip(args[::2], args[1::2]):
        print(pred, json.dumps(score(ref, pred), ensure_ascii=False))
