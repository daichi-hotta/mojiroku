"""Compare a saved mojiroku recording with a private reference transcript.

The reference is usually a transcript exported from the meeting tool (Zoom VTT/TXT).
Everything stays on the machine that runs this script. The report contains only
numbers, time positions, mojiroku speaker ids, and anonymous reference speaker labels
(R1, R2, ...). It never prints transcript text, speaker names, titles of other
recordings, or file contents, so it can be shared to tune the pipeline on meetings
that must stay private.

Scoring uses the Japanese CER normaliser in metrics.py. Long meetings are aligned with
unique character n-gram anchors, then exact edit distance between anchors, so the error
count is an upper bound on the true edit distance (usually equal in practice).
"""

from __future__ import annotations

import argparse
import bisect
from collections import Counter, defaultdict
import json
from pathlib import Path
import re
import sqlite3
import statistics
import sys

from metrics import NORMALIZATION, tokens

REPORT_VERSION = "reference-compare-v1"
DP_LIMIT = 1_500_000
CLIP_MARGIN_MS = 10_000
ANCHOR_SIZES = (12, 8, 5, 3)
TIME = r"\d{1,2}:\d{2}(?::\d{2})?(?:[.,]\d{1,3})?"
# Whole edit runs made only of these are fillers/backchannels, not content errors.
FILLER = re.compile(
    r"^(?:え[えー]*っ?と?|えっと|あ[あー]+|あの[ー]*|そ[のー]+ー|ま[あー]+|う[ー]*ん|ん[ー]+|"
    r"はい|ええ|うん|おー|ほー|へー|なんか|こう|ね[ー]*|ねえ)+$"
)
PARTICLES = set("はがをにでともへやかねよなのだ")
KANJI_DIGITS = set("〇一二三四五六七八九十百千万億")
# Whisper's well-known silence hallucinations (ADR-0031); counted, never printed.
HALLUCINATIONS = ("ご視聴ありがとうございました", "チャンネル登録", "字幕", "おやすみなさい")


class Unit:
    """One normalised character with the time (ms) and speaker it belongs to."""

    __slots__ = ("char", "ms", "speaker", "group")

    def __init__(self, char: str, ms: float, speaker: str | None, group: int = -1):
        # group: index of the reference cue or mojiroku segment the character came from.
        self.char, self.ms, self.speaker, self.group = char, ms, speaker, group


def spread(text: str, start_ms: float, end_ms: float, speaker: str | None, group: int = -1) -> list[Unit]:
    chars = tokens(text, "ja")
    if not chars:
        return []
    step = max(end_ms - start_ms, 0) / len(chars)
    return [Unit(c, start_ms + step * (n + 0.5), speaker, group) for n, c in enumerate(chars)]


# ── reference parsing ────────────────────────────────────────────────────────

def parse_time(value: str) -> float:
    parts = value.replace(",", ".").split(":")
    seconds = float(parts[-1])
    minutes = int(parts[-2]) if len(parts) >= 2 else 0
    hours = int(parts[-3]) if len(parts) >= 3 else 0
    return ((hours * 60 + minutes) * 60 + seconds) * 1000


def shape(line: str) -> str:
    """Line structure without content: digits→9, letters→a, CJK/kana→J."""
    out = []
    for c in line[:48]:
        if c.isdigit():
            out.append("9")
        elif c.isascii() and c.isalpha():
            out.append("a")
        elif c.isspace():
            out.append(" ")
        elif c in ":[]()-><.,：":
            out.append(c)
        else:
            out.append("J")
    return "".join(out)


def _split_speaker(body: str) -> tuple[str | None, str]:
    voice = re.match(r"^<v(?:\.[^ >]*)?\s+([^>]+)>(.*)$", body)
    if voice:
        return voice.group(1).strip(), voice.group(2)
    prefix = re.match(r"^([^:：<>]{1,40}?)\s*[:：]\s*(.*)$", body)
    if prefix:
        return prefix.group(1).strip(), prefix.group(2)
    return None, body


def _accept_speakers(cues: list[dict]) -> list[dict]:
    """A colon prefix is a speaker if it recurs, or if nearly every cue has one
    (a speaker-labelled export); otherwise it was part of the text."""
    counts = Counter(c["prefix"] for c in cues if c["prefix"])
    labelled = sum(counts.values()) >= 0.8 * len(cues) and any(n >= 2 for n in counts.values())
    current = None
    for cue in cues:
        if cue["prefix"] and (labelled or counts[cue["prefix"]] >= 2):
            current = cue["prefix"]
        elif cue["prefix"]:
            cue["text"] = f"{cue['prefix']}:{cue['text']}"
        cue["speaker"] = current
    return cues


def parse_reference(raw: str) -> tuple[list[dict], str]:
    """Return cues [{start, end, speaker, text}] and the detected format name."""
    raw = raw.lstrip("﻿").replace("\r\n", "\n").replace("\r", "\n")
    cue_line = re.compile(rf"^\s*({TIME})\s*-->\s*({TIME})")
    if any(cue_line.match(line) for line in raw.splitlines()):
        cues = []
        for block in re.split(r"\n\s*\n", raw):
            lines = [line.strip() for line in block.split("\n") if line.strip()]
            at = next((n for n, line in enumerate(lines) if cue_line.match(line)), None)
            if at is None:
                continue
            match = cue_line.match(lines[at])
            body = " ".join(lines[at + 1:])
            prefix, text = _split_speaker(body)
            text = re.sub(r"<[^>]+>", "", text)
            cues.append({"start": parse_time(match.group(1)), "end": parse_time(match.group(2)),
                         "prefix": prefix, "text": text})
        return _accept_speakers(cues), "vtt/srt"

    header_forms = [
        ("bracket-name-time", re.compile(rf"^\[(?P<speaker>[^\]]+)\]\s+(?P<time>{TIME})\s*(?P<text>.*)$")),
        ("time-name-colon", re.compile(rf"^(?P<time>{TIME})\s+(?P<speaker>[^:：]{{1,40}}?)\s*[:：]\s*(?P<text>.*)$")),
        ("name-time", re.compile(rf"^(?P<speaker>[^\d\s][^:：]{{0,39}}?)\s+(?P<time>{TIME})\s*$")),
        ("time-only", re.compile(rf"^(?P<time>{TIME})\s*$")),
    ]
    lines = [line.strip() for line in raw.split("\n")]
    best = max(header_forms, key=lambda form: sum(bool(form[1].match(line)) for line in lines))
    name, pattern = best
    cues: list[dict] = []
    for line in lines:
        if not line:
            continue
        match = pattern.match(line)
        if match:
            groups = match.groupdict()
            cues.append({"start": parse_time(groups["time"]), "end": None,
                         "speaker": (groups.get("speaker") or "").strip() or None,
                         "text": groups.get("text") or ""})
        elif cues:
            body = line
            if name == "time-only":
                speaker, body = _split_speaker(line)
                if speaker and not cues[-1]["text"]:
                    cues[-1]["speaker"] = speaker
            cues[-1]["text"] += body
    if not cues:
        return [], "unknown"
    # Wall-clock exports (for example 13:02:11) can cross midnight; keep them monotonic.
    for previous, cue in zip(cues, cues[1:]):
        while cue["start"] < previous["start"] - 6 * 3600 * 1000:
            cue["start"] += 24 * 3600 * 1000
    for cue, following in zip(cues, cues[1:] + [None]):
        limit = cue["start"] + 30_000
        cue["end"] = min(following["start"], limit) if following else limit
    return cues, name


def reference_units(cues: list[dict]) -> tuple[list[Unit], dict]:
    totals = Counter()
    for cue in cues:
        totals[cue["speaker"]] += len(tokens(cue["text"], "ja"))
    # Anonymous labels by amount of speech: R1 spoke the most.
    named = sorted((s for s in totals if s is not None), key=lambda s: (-totals[s], s))
    labels = {speaker: f"R{n}" for n, speaker in enumerate(named, 1)}
    labels[None] = "R?"
    units = []
    for n, cue in enumerate(cues):
        units += spread(cue["text"], cue["start"], cue["end"], labels[cue["speaker"]], n)
    return units, {labels[s]: totals[s] for s in totals}


# ── mojiroku side ─────────────────────────────────────────────────────────────

def open_readonly(db: Path) -> sqlite3.Connection:
    return sqlite3.connect(f"{db.resolve().as_uri()}?mode=ro", uri=True)


def find_recording(conn: sqlite3.Connection, title: str | None, recording_id: str | None) -> dict:
    columns = "id, source_type, duration_ms, language, created_at"
    if recording_id:
        rows = conn.execute(f"SELECT {columns} FROM recordings WHERE id = ?", (recording_id,)).fetchall()
    else:
        rows = conn.execute(f"SELECT {columns} FROM recordings WHERE title = ?", (title,)).fetchall()
        if not rows:
            rows = conn.execute(f"SELECT {columns} FROM recordings WHERE instr(title, ?) > 0",
                                (title,)).fetchall()
    if len(rows) != 1:
        # Only ids and dates, never other titles.
        listing = ", ".join(f"{r[0]} ({r[4]})" for r in rows[:10])
        raise SystemExit(f"{len(rows)} recordings matched; pass --recording-id. {listing}")
    keys = ("id", "source_type", "duration_ms", "language", "created_at")
    recording = dict(zip(keys, rows[0]))
    try:
        offset = conn.execute("SELECT mic_offset_ms FROM recordings WHERE id = ?",
                              (recording["id"],)).fetchone()[0]
    except sqlite3.OperationalError:
        offset = None
    recording["mic_offset_ms"] = offset
    jobs = conn.execute("SELECT kind, status, params FROM jobs WHERE recording_id = ? ORDER BY created_at",
                        (recording["id"],)).fetchall()
    recording["jobs"] = []
    for kind, status, params in jobs:
        try:
            values = json.loads(params)
        except ValueError:
            values = {}
        safe = {k: values.get(k) for k in ("stt_lang", "lang", "transcription_model", "diarize")}
        recording["jobs"].append({"kind": kind, "status": status, **safe})
    return recording


def saved_segments(conn: sqlite3.Connection, recording_id: str) -> list[dict]:
    rows = conn.execute("SELECT start_ms, end_ms, text, speaker_id FROM segments "
                        "WHERE recording_id = ? ORDER BY idx", (recording_id,)).fetchall()
    return [{"start_ms": a, "end_ms": b, "text": t, "speaker_id": s} for a, b, t, s in rows]


def hypothesis_units(segments: list[dict]) -> list[Unit]:
    units = []
    for n, segment in enumerate(segments):
        units += spread(segment["text"], segment["start_ms"], segment["end_ms"],
                        segment.get("speaker_id") or "-", n)
    return units


# ── alignment ────────────────────────────────────────────────────────────────

def _dp(ref: str, hyp: str, a: int, b: int, c: int, d: int) -> list[tuple]:
    n, m = b - a, d - c
    rows = [list(range(m + 1))]
    for i in range(1, n + 1):
        previous, current, expected = rows[-1], [i], ref[a + i - 1]
        for j in range(1, m + 1):
            current.append(min(previous[j - 1] + (expected != hyp[c + j - 1]),
                               previous[j] + 1, current[j - 1] + 1))
        rows.append(current)
    ops, i, j = [], n, m
    while i or j:
        here = rows[i][j]
        if i and j and here == rows[i - 1][j - 1] + (ref[a + i - 1] != hyp[c + j - 1]):
            ops.append(("=" if ref[a + i - 1] == hyp[c + j - 1] else "S", a + i - 1, c + j - 1))
            i, j = i - 1, j - 1
        elif i and here == rows[i - 1][j] + 1:
            ops.append(("D", a + i - 1, None))
            i -= 1
        else:
            ops.append(("I", None, c + j - 1))
            j -= 1
    ops.reverse()
    return ops


def _lis(pairs: list[tuple[int, int]]) -> list[tuple[int, int]]:
    """Longest chain increasing in both coordinates (pairs sorted by the first)."""
    tails, tail_at, parent = [], [], [None] * len(pairs)
    for n, (_, j) in enumerate(pairs):
        k = bisect.bisect_left(tails, j)
        if k == len(tails):
            tails.append(j)
            tail_at.append(n)
        else:
            tails[k], tail_at[k] = j, n
        parent[n] = tail_at[k - 1] if k else None
    chain, n = [], tail_at[-1] if tail_at else None
    while n is not None:
        chain.append(pairs[n])
        n = parent[n]
    return chain[::-1]


def _anchors(ref: str, hyp: str, a: int, b: int, c: int, d: int, k: int) -> list[tuple[int, int, int]]:
    def unique(text: str, start: int, end: int) -> dict:
        seen: dict[str, int] = {}
        for i in range(start, end - k + 1):
            gram = text[i:i + k]
            seen[gram] = -1 if gram in seen else i
        return {g: i for g, i in seen.items() if i >= 0}

    if b - a < k or d - c < k:
        return []
    in_ref, in_hyp = unique(ref, a, b), unique(hyp, c, d)
    chain = _lis(sorted((in_ref[g], in_hyp[g]) for g in in_ref.keys() & in_hyp.keys()))
    blocks: list[list[int]] = []
    for i, j in chain:
        if blocks:
            bi, bj, length = blocks[-1]
            if i - bi == j - bj and i <= bi + length:
                blocks[-1][2] = max(length, i + k - bi)
                continue
            if i < bi + length or j < bj + length:
                continue
        blocks.append([i, j, k])
    return [tuple(block) for block in blocks]


def align(ref: str, hyp: str) -> tuple[list[tuple], int]:
    """Return edit ops (op, ref_index, hyp_index) and the number of unaligned units."""
    ops: list[tuple] = []
    unaligned = 0
    # Explicit stack instead of recursion: ranges to align and ready ops, popped in order.
    pending: list = [("range", 0, len(ref), 0, len(hyp))]
    while pending:
        item = pending.pop()
        if item[0] == "ops":
            ops.extend(item[1])
            continue
        _, a, b, c, d = item
        head = []
        while a < b and c < d and ref[a] == hyp[c]:
            head.append(("=", a, c))
            a, c = a + 1, c + 1
        tail = []
        while a < b and c < d and ref[b - 1] == hyp[d - 1]:
            b, d = b - 1, d - 1
            tail.append(("=", b, d))
        tail.reverse()
        ops.extend(head)
        if a == b or c == d or (b - a) * (d - c) <= DP_LIMIT:
            ops.extend(_dp(ref, hyp, a, b, c, d))
            ops.extend(tail)
            continue
        blocks = []
        for k in ANCHOR_SIZES:
            blocks = _anchors(ref, hyp, a, b, c, d, k)
            if blocks:
                break
        if not blocks:
            unaligned += (b - a) + (d - c)
            ops.extend([("D", i, None) for i in range(a, b)] + [("I", None, j) for j in range(c, d)])
            ops.extend(tail)
            continue
        work = []
        pi, pj = a, c
        for i, j, length in blocks:
            work.append(("range", pi, i, pj, j))
            work.append(("ops", [("=", i + t, j + t) for t in range(length)]))
            pi, pj = i + length, j + length
        work.append(("range", pi, b, pj, d))
        work.append(("ops", tail))
        pending.extend(reversed(work))
    return ops, unaligned


# ── analysis ─────────────────────────────────────────────────────────────────

def char_class(c: str) -> str:
    if "぀" <= c <= "ゟ":
        return "hiragana"
    if "゠" <= c <= "ヿ" or "ㇰ" <= c <= "ㇿ":
        return "katakana"
    if c in KANJI_DIGITS or c.isdigit():
        return "number"
    if "一" <= c <= "鿿" or "㐀" <= c <= "䶿" or c in "々〆":
        return "kanji"
    if c.isascii() and c.isalpha():
        return "latin"
    return "other"


CATEGORY_LABELS = {
    "missing_speech": "Long stretch only in the reference (mojiroku dropped speech?)",
    "extra_text": "Long stretch only in mojiroku (hallucination, or reference dropped it)",
    "filler": "Filler words or backchannels (え-, あの, はい, うん...)",
    "number": "Numbers (digits or kanji numerals)",
    "latin": "English words / Latin letters",
    "katakana": "Katakana words (loanwords, names, terms)",
    "particle": "Single particle added, dropped, or changed",
    "kana_vs_kanji": "Same spot written in kana on one side and kanji on the other",
    "kanji": "Different kanji/words on both sides",
    "kana": "Kana-only differences (endings, grammar, short words)",
    "other": "Other",
}


def categorise(ref_part: str, hyp_part: str) -> str:
    r, h = len(ref_part), len(hyp_part)
    if r >= 10 and h <= r * 0.2:
        return "missing_speech"
    if h >= 10 and r <= h * 0.2:
        return "extra_text"
    if all(not part or FILLER.match(part) for part in (ref_part, hyp_part)):
        return "filler"
    classes_r = {char_class(c) for c in ref_part}
    classes_h = {char_class(c) for c in hyp_part}
    both = classes_r | classes_h
    if "number" in both:
        return "number"
    if "latin" in both:
        return "latin"
    if any(c != "ー" and char_class(c) == "katakana" for c in ref_part + hyp_part):
        return "katakana"
    if r <= 1 and h <= 1 and set(ref_part + hyp_part) <= PARTICLES:
        return "particle"
    if ("kanji" in classes_r) != ("kanji" in classes_h) and "hiragana" in both:
        return "kana_vs_kanji"
    if "kanji" in classes_r and "kanji" in classes_h:
        return "kanji"
    if both <= {"hiragana", "other"}:
        return "kana"
    return "other"


def _one_sided(ref_len: int, hyp_len: int) -> bool:
    longer = max(ref_len, hyp_len)
    return longer >= 10 and min(ref_len, hyp_len) <= 0.35 * longer


def edit_runs(ops: list[tuple], bridge: int = 2) -> list[dict]:
    """Group consecutive edits. A dropped or invented stretch often keeps a few chance
    matches (a lone は or い); runs separated by at most `bridge` matches are joined
    when the joined region is still clearly one-sided, so it is counted once."""
    spans, start = [], None
    for n, (op, _, _) in enumerate(ops):
        if op != "=" and start is None:
            start = n
        elif op == "=" and start is not None:
            spans.append([start, n])
            start = None
    if start is not None:
        spans.append([start, len(ops)])

    def sides(a: int, b: int) -> tuple[int, int]:
        part = ops[a:b]
        return sum(i is not None for _, i, _ in part), sum(j is not None for _, _, j in part)

    merged: list[list[int]] = []
    for span in spans:
        if merged and span[0] - merged[-1][1] <= bridge and _one_sided(*sides(merged[-1][0], span[1])):
            merged[-1][1] = span[1]
        else:
            merged.append(span)

    runs = []
    for a, b in merged:
        before = next((ops[n] for n in range(a - 1, -1, -1) if ops[n][0] == "="), None)
        part = ops[a:b]
        runs.append({
            "ref": [i for _, i, _ in part if i is not None],
            "hyp": [j for _, _, j in part if j is not None],
            "cost": sum(op != "=" for op, _, _ in part),
            "ref_anchor": before[1] if before else None,
            "hyp_anchor": before[2] if before else None,
        })
    return runs


def mmss(ms: float) -> str:
    seconds = max(int(ms // 1000), 0)
    return f"{seconds // 3600:d}:{seconds // 60 % 60:02d}:{seconds % 60:02d}"


def _median_offset(ops: list[tuple], ref_units: list[Unit], hyp_units: list[Unit]) -> list[float]:
    return [ref_units[i].ms - hyp_units[j].ms for op, i, j in ops if op == "="]


def analyse(ref_units: list[Unit], hyp_units: list[Unit], window_min: int = 5,
            segments: list[dict] | None = None, cues: list[dict] | None = None, clip: bool = True) -> dict:
    ref = "".join(u.char for u in ref_units)
    if not ref:
        raise SystemExit("reference has no characters after normalisation")
    # Score only the time both transcripts cover: an export that stops early must not count
    # the rest of the recording as invented text. The first pass finds the clock offset.
    outside: Counter = Counter()
    if clip and hyp_units:
        first_ops, _ = align(ref, "".join(u.char for u in hyp_units))
        first = _median_offset(first_ops, ref_units, hyp_units)
        if first:
            shift = statistics.median(first)
            lo, hi = ref_units[0].ms - CLIP_MARGIN_MS, ref_units[-1].ms + CLIP_MARGIN_MS
            kept = []
            for unit in hyp_units:
                if lo <= unit.ms + shift <= hi:
                    kept.append(unit)
                else:
                    outside[unit.speaker] += 1
            hyp_units = kept
    hyp = "".join(u.char for u in hyp_units)
    ops, unaligned = align(ref, hyp)
    counts = Counter(op for op, _, _ in ops)
    errors = counts["S"] + counts["D"] + counts["I"]

    # Clock offset between the reference and mojiroku timelines, from matched characters.
    offsets = _median_offset(ops, ref_units, hyp_units)
    offset = statistics.median(offsets) if offsets else 0.0
    quartiles = statistics.quantiles(offsets, n=4) if len(offsets) >= 4 else [offset] * 3
    ref_start = ref_units[0].ms

    def hyp_time_of(run: dict) -> float:
        if run["hyp"]:
            return hyp_units[run["hyp"][0]].ms
        if run["ref"]:
            return ref_units[run["ref"][0]].ms - offset
        return hyp_units[run["hyp_anchor"]].ms if run["hyp_anchor"] is not None else 0.0

    def ref_index_of(run: dict) -> int:
        if run["ref"]:
            return run["ref"][0]
        return run["ref_anchor"] if run["ref_anchor"] is not None else 0

    runs = edit_runs(ops)
    categories: dict[str, dict] = defaultdict(lambda: {"runs": 0, "errors": 0, "ref_chars": 0, "hyp_chars": 0})
    long_events = []
    per_ref_speaker: dict[str, Counter] = defaultdict(Counter)
    per_hyp_speaker: dict[str, Counter] = defaultdict(Counter)
    windows: dict[int, Counter] = defaultdict(Counter)
    run_lengths = Counter()
    for run in runs:
        ref_part = "".join(ref[i] for i in run["ref"])
        hyp_part = "".join(hyp[j] for j in run["hyp"])
        category = categorise(ref_part, hyp_part)
        entry = categories[category]
        entry["runs"] += 1
        entry["errors"] += run["cost"]
        entry["ref_chars"] += len(ref_part)
        entry["hyp_chars"] += len(hyp_part)
        size = run["cost"]
        run_lengths["1" if size == 1 else "2-3" if size <= 3 else "4-9" if size <= 9 else "10-29" if size <= 29 else "30+"] += 1
        at = ref_index_of(run)
        speaker = ref_units[at].speaker if ref_units else "R?"
        per_ref_speaker[speaker]["errors"] += run["cost"]
        window = int((ref_units[at].ms - ref_start) // (window_min * 60_000))
        windows[window]["errors"] += run["cost"]
        if run["hyp"]:
            per_hyp_speaker[hyp_units[run["hyp"][0]].speaker]["inserted_or_changed"] += len(run["hyp"])
        if category in ("missing_speech", "extra_text"):
            long_events.append({
                "kind": category,
                "mojiroku_time": mmss(hyp_time_of(run)),
                "reference_speaker": speaker,
                "mojiroku_speaker": hyp_units[run["hyp"][0]].speaker if run["hyp"] else None,
                "ref_chars": len(ref_part),
                "hyp_chars": len(hyp_part),
            })
    for unit in ref_units:
        per_ref_speaker[unit.speaker]["ref_chars"] += 1
        windows[int((unit.ms - ref_start) // (window_min * 60_000))]["ref_chars"] += 1
    for unit in hyp_units:
        per_hyp_speaker[unit.speaker]["hyp_chars"] += 1

    # Which mojiroku speaker said each reference speaker's matched characters.
    confusion: dict[str, Counter] = defaultdict(Counter)
    for op, i, j in ops:
        if op in ("=", "S"):
            confusion[ref_units[i].speaker][hyp_units[j].speaker] += 1
    paired = sum(sum(row.values()) for row in confusion.values())
    by_hyp: dict[str, Counter] = defaultdict(Counter)
    for ref_speaker, row in confusion.items():
        for hyp_speaker, n in row.items():
            by_hyp[hyp_speaker][ref_speaker] += n
    best_match = sum(max(row.values()) for row in by_hyp.values())

    def rate(e: int, n: int) -> float | None:
        return round(e / n, 4) if n else None

    no_filler_errors = errors - categories["filler"]["errors"] if "filler" in categories else errors
    diagnostics = (segment_diagnostics(ops, ref_units, hyp_units, segments, cues, offset, confusion)
                   if segments is not None and cues is not None else None)
    return {
        "reference_chars": len(ref),
        "mojiroku_chars": len(hyp),
        "mojiroku_chars_outside_reference_span": dict(outside),
        "segment_diagnostics": diagnostics,
        "errors": errors,
        "cer": rate(errors, len(ref)),
        "substitutions": counts["S"],
        "deletions": counts["D"],
        "insertions": counts["I"],
        "cer_without_filler_runs": rate(no_filler_errors, len(ref)),
        "unaligned_units": unaligned,
        "clock_offset_ms": {"median": round(offset), "q1": round(quartiles[0]), "q3": round(quartiles[2])},
        "edit_run_sizes": dict(run_lengths),
        "categories": {k: {**v, "share_of_errors": rate(v["errors"], errors)}
                       for k, v in sorted(categories.items(), key=lambda kv: -kv[1]["errors"])},
        "long_events": long_events,
        "reference_speakers": {k: {**v, "cer": rate(v["errors"], v["ref_chars"])}
                               for k, v in sorted(per_ref_speaker.items())},
        "mojiroku_speakers": {k: dict(v) for k, v in sorted(per_hyp_speaker.items())},
        "speaker_overlap_chars": {k: dict(v) for k, v in sorted(confusion.items())},
        "speaker_purity": rate(best_match, paired),
        "windows": [{"from": mmss(w * window_min * 60_000), "ref_chars": v["ref_chars"],
                     "errors": v["errors"], "cer": rate(v["errors"], v["ref_chars"])}
                    for w, v in sorted(windows.items())],
    }


def segment_diagnostics(ops: list[tuple], ref_units: list[Unit], hyp_units: list[Unit],
                        segments: list[dict], cues: list[dict], offset: float,
                        confusion: dict[str, Counter]) -> dict:
    """Whole mojiroku segments the reference does not support, and whole reference cues
    mojiroku missed, with what was happening on the other side at that time. Counts only."""
    seg_total, seg_matched = Counter(), Counter()
    for unit in hyp_units:
        seg_total[unit.group] += 1
    cue_total, cue_matched = Counter(), Counter()
    for unit in ref_units:
        cue_total[unit.group] += 1
    for op, i, j in ops:
        if op == "=":
            seg_matched[hyp_units[j].group] += 1
            cue_matched[ref_units[i].group] += 1

    def track(segment: dict) -> str:
        return "mic" if segment.get("speaker_id") == "self" else "system"

    remote = [(seg["start_ms"], seg["end_ms"], _trigrams(seg["text"]))
              for n, seg in enumerate(segments) if track(seg) == "system" and seg_total[n]]
    unsupported: dict[str, Counter] = defaultdict(Counter)
    phrases: dict[str, Counter] = defaultdict(Counter)
    duplicates = Counter()
    for n, segment in enumerate(segments):
        total = seg_total[n]
        if not total:
            continue  # empty or outside the scored span
        supported = seg_matched[n] >= 0.5 * total
        for phrase in HALLUCINATIONS:
            if phrase in segment["text"]:
                phrases[phrase][f"{track(segment)}_{'in' if supported else 'not_in'}_reference"] += 1
        previous = segments[n - 1] if n else None
        if previous and previous["text"].strip() == segment["text"].strip() and \
                previous.get("speaker_id") == segment.get("speaker_id"):
            duplicates["in_reference" if supported else "not_in_reference"] += 1
        if total < 4 or seg_matched[n] >= 0.3 * total:
            continue
        side = track(segment)
        unsupported[side]["segments"] += 1
        unsupported[side]["chars"] += total
        if side == "mic":
            grams = _trigrams(segment["text"])
            near = [(a, b, g) for a, b, g in remote
                    if a < segment["end_ms"] + 5000 and b > segment["start_ms"] - 5000]
            if any(a < segment["end_ms"] and b > segment["start_ms"] for a, b, _ in near):
                unsupported[side]["while_system_track_has_speech"] += 1
            if grams and any(len(grams & g) >= 0.3 * len(grams) for _, _, g in near):
                unsupported[side]["resembles_nearby_system_text"] += 1

    # A reference speaker heard mostly as 'self' talks into the mic track.
    on_mic = {speaker for speaker, row in confusion.items() if row and row.most_common(1)[0][0] == "self"}
    missed: dict[str, Counter] = defaultdict(Counter)
    for n, cue in enumerate(cues):
        total = cue_total[n]
        if total < 4 or cue_matched[n] >= 0.3 * total:
            continue
        speaker = next(u.speaker for u in ref_units if u.group == n)
        side = "mic" if speaker in on_mic else "system"
        start, end = cue["start"] - offset, cue["end"] - offset
        covered = any(track(seg) == side and seg["start_ms"] < end and seg["end_ms"] > start
                      for seg in segments)
        missed[side]["cues"] += 1
        missed[side]["chars"] += total
        missed[side]["mojiroku_had_a_segment_then" if covered else "no_mojiroku_segment_then"] += 1
    return {
        "mojiroku_segments_not_in_reference": {k: dict(v) for k, v in unsupported.items()},
        "reference_cues_missed": {k: dict(v) for k, v in missed.items()},
        "known_hallucination_phrases": {k: dict(v) for k, v in phrases.items()},
        "repeated_previous_segment": dict(duplicates),
        # Per segment index: (matched chars, scored chars). Used for audio levels, not printed.
        "_support": {n: (seg_matched[n], seg_total[n]) for n in seg_total},
    }


def _trigrams(text: str) -> set[str]:
    chars = "".join(tokens(text, "ja"))
    return {chars[i:i + 3] for i in range(len(chars) - 2)}


def cross_track_echoes(segments: list[dict], window_ms: int = 3000) -> int:
    """Mic ('self') segments that repeat a nearby remote segment: speaker audio leaking
    into the microphone gets transcribed twice."""
    remote = [(s["start_ms"], _trigrams(s["text"])) for s in segments if s.get("speaker_id") != "self"]
    starts = [start for start, _ in remote]
    echoes = 0
    for segment in segments:
        if segment.get("speaker_id") != "self":
            continue
        grams = _trigrams(segment["text"])
        if len(grams) < 3:
            continue
        lo = bisect.bisect_left(starts, segment["start_ms"] - window_ms)
        hi = bisect.bisect_right(starts, segment["start_ms"] + window_ms)
        if any(len(grams & other) >= 0.6 * len(grams) for _, other in remote[lo:hi]):
            echoes += 1
    return echoes


def hygiene(segments: list[dict]) -> dict:
    texts = [s["text"].strip() for s in segments]
    repeated = sum(1 for a, b in zip(texts, texts[1:]) if a and a == b)
    looping = sum(1 for t in texts if re.search(r"(.{2,12})\1{3,}", t))
    lengths = [len(tokens(t, "ja")) for t in texts]
    gaps = [b["start_ms"] - a["end_ms"] for a, b in zip(segments, segments[1:])]
    return {
        "segments": len(segments),
        "empty_segments": sum(1 for n in lengths if n == 0),
        "median_segment_chars": statistics.median(lengths) if lengths else 0,
        "max_segment_chars": max(lengths) if lengths else 0,
        "same_text_as_previous_segment": repeated,
        "segments_with_looping_text": looping,
        "known_hallucination_phrases": sum(t.count(p) for t in texts for p in HALLUCINATIONS),
        "overlapping_segment_pairs": sum(1 for g in gaps if g < 0),
        "gaps_over_30s": sum(1 for g in gaps if g > 30_000),
        "self_segments": sum(1 for seg in segments if seg.get("speaker_id") == "self"),
        "self_segments_echoing_remote": cross_track_echoes(segments),
    }


def print_report(result: dict, out=sys.stdout) -> None:
    def line(text: str = "") -> None:
        print(text, file=out)

    def pct(value) -> str:
        return "n/a" if value is None else f"{value * 100:.1f}%"

    line(f"Reference chars {result['reference_chars']:,}  mojiroku chars {result['mojiroku_chars']:,}")
    if result["mojiroku_chars_outside_reference_span"]:
        line(f"Not scored (mojiroku text outside the reference's time span): "
             f"{result['mojiroku_chars_outside_reference_span']}")
    line(f"CER {pct(result['cer'])}  (S {result['substitutions']:,} / D {result['deletions']:,} / "
         f"I {result['insertions']:,})  without filler runs {pct(result['cer_without_filler_runs'])}")
    if result["unaligned_units"]:
        line(f"Unaligned units (no anchors found): {result['unaligned_units']:,}")
    off = result["clock_offset_ms"]
    line(f"Clock offset ref-mojiroku: median {off['median'] / 1000:.1f}s, IQR "
         f"{off['q1'] / 1000:.1f}..{off['q3'] / 1000:.1f}s")
    line(f"Edit run sizes: {result['edit_run_sizes']}")
    line()
    line("Error categories (share of all errors):")
    for key, value in result["categories"].items():
        line(f"  {pct(value['share_of_errors']):>6}  {value['errors']:>6,} errors in {value['runs']:>5,} runs "
             f"(ref {value['ref_chars']:,} / mojiroku {value['hyp_chars']:,} chars)  {CATEGORY_LABELS[key]}")
    line()
    line("Reference speakers (anonymous, R1 spoke most):")
    for key, value in result["reference_speakers"].items():
        overlap = result["speaker_overlap_chars"].get(key, {})
        line(f"  {key}: {value.get('ref_chars', 0):,} chars, CER {pct(value['cer'])}, "
             f"heard by mojiroku as {dict(sorted(overlap.items(), key=lambda kv: -kv[1]))}")
    line(f"Speaker purity (each mojiroku speaker → its main reference speaker): {pct(result['speaker_purity'])}")
    line()
    line("Time windows (reference timeline):")
    for window in result["windows"]:
        line(f"  {window['from']}  {window['ref_chars']:>6,} chars  CER {pct(window['cer'])}")
    line()
    if result.get("segment_diagnostics"):
        line("Segment diagnostics:")
        for key, value in result["segment_diagnostics"].items():
            line(f"  {key}: {value}")
        line()
    events = result["long_events"]
    line(f"Long one-sided stretches: {len(events)} (time = mojiroku position)")
    for event in events[:40]:
        line(f"  {event['mojiroku_time']}  {event['kind']:<14} ref {event['ref_chars']:>4} / "
             f"mojiroku {event['hyp_chars']:>4} chars  ref speaker {event['reference_speaker']}, "
             f"mojiroku speaker {event['mojiroku_speaker']}")


# ── audio levels (numbers only) ──────────────────────────────────────────────

def _dbfs(handle, start_ms: float, end_ms: float) -> float | None:
    import array
    import math

    rate = handle.getframerate()
    first = max(int(start_ms * rate / 1000), 0)
    count = int((end_ms - start_ms) * rate / 1000)
    if count <= 0 or first >= handle.getnframes():
        return None
    handle.setpos(first)
    samples = array.array("h", handle.readframes(count))
    if sys.byteorder == "big":
        samples.byteswap()
    if not samples:
        return None
    power = sum(x * x for x in samples) / len(samples)
    return 20 * math.log10(max(math.sqrt(power), 1) / 32768)


def mic_levels(recordings: Path, recording: dict, segments: list[dict], support: dict) -> dict:
    """Mic and system loudness during mic ('self') segments, grouped by whether the
    reference supports the segment. Leaked remote audio or noise is much quieter on the
    mic than the owner's own voice and coincides with a louder system track."""
    import wave

    tracks = {label: (path, shift) for path, label, shift in recording_tracks(recordings, recording)}
    if "self" not in tracks or "system" not in tracks:
        return {"skipped": "needs both meeting tracks"}
    groups: dict[str, list[tuple[float, float]]] = defaultdict(list)
    with wave.open(str(tracks["self"][0])) as mic, wave.open(str(tracks["system"][0])) as system:
        if mic.getsampwidth() != 2 or system.getsampwidth() != 2:
            return {"skipped": "expects 16-bit PCM"}
        for n, segment in enumerate(segments):
            if segment.get("speaker_id") != "self" or n not in support:
                continue
            matched, total = support[n]
            if matched >= 0.5 * total:
                group = "supported"
            elif total >= 4 and matched < 0.3 * total:
                group = "not_in_reference"
            else:
                continue
            if any(p in segment["text"] for p in HALLUCINATIONS):
                group = "known_hallucination_phrase"
            a, b = segment["start_ms"], segment["end_ms"]
            m = _dbfs(mic, a - tracks["self"][1], b - tracks["self"][1])
            y = _dbfs(system, a - tracks["system"][1], b - tracks["system"][1])
            if m is not None and y is not None:
                groups[group].append((m, y))

    def summary(values: list[float]) -> list[float]:
        if len(values) < 2:
            return [round(v, 1) for v in values]
        q = statistics.quantiles(values, n=4)
        return [round(q[0], 1), round(statistics.median(values), 1), round(q[2], 1)]

    return {group: {"segments": len(pairs),
                    "mic_dbfs_q1_median_q3": summary([m for m, _ in pairs]),
                    "system_dbfs_q1_median_q3": summary([y for _, y in pairs]),
                    "mic_minus_system_db_q1_median_q3": summary([m - y for m, y in pairs])}
            for group, pairs in sorted(groups.items())}


# ── re-running the pipeline on the saved audio ──────────────────────────────

VARIANTS = {
    # name: (decoder, model file)
    "turbo-greedy": ("greedy", "ggml-large-v3-turbo-q5_0.bin"),
    "turbo-beam5": ("beam5", "ggml-large-v3-turbo-q5_0.bin"),
    "full-greedy": ("greedy", "ggml-large-v3-q5_0.bin"),
    "full-beam5": ("beam5", "ggml-large-v3-q5_0.bin"),
}


def recording_tracks(recordings: Path, recording: dict) -> list[tuple[Path, str, int]]:
    """Audio the app would transcribe, as (path, speaker label, shift ms) like merge_tracks."""
    rid = recording["id"]
    mic, system = recordings / f"{rid}-mic.wav", recordings / f"{rid}-system.wav"
    offset = recording.get("mic_offset_ms") or 0
    if recording["source_type"] == "live" and (mic.exists() or system.exists()):
        tracks = []
        if mic.exists():
            tracks.append((mic, "self", max(offset, 0)))
        if system.exists():
            tracks.append((system, "system", max(-offset, 0)))
        return tracks
    primary = [p for p in recordings.iterdir() if p.stem == rid]
    if not primary:
        raise SystemExit("no audio for this recording in the recordings folder")
    return [(primary[0], "-", 0)]


def transcribe(binary: Path, audio: Path, models: Path, language: str, decoder: str,
               model: str, extra: list[str], timeout: float) -> tuple[list[dict], float]:
    import subprocess

    # Output stays in this process; stderr (progress lines) is discarded, never echoed.
    completed = subprocess.run(
        [str(binary), str(audio), str(models), language, decoder, "--json", "--model", model, *extra],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout, check=False)
    if completed.returncode != 0:
        raise SystemExit(f"transcribe_cli exited {completed.returncode} on {audio.name}")
    payload = json.loads(completed.stdout)
    return payload["transcript"]["segments"], float(payload["pipeline_seconds"])


def merged_segments(per_track: list[tuple[list[dict], str, int]]) -> list[dict]:
    segments = []
    for track_segments, speaker, shift in per_track:
        for segment in track_segments:
            segments.append({"start_ms": segment["start_ms"] + shift, "end_ms": segment["end_ms"] + shift,
                             "text": segment["text"], "speaker_id": speaker})
    segments.sort(key=lambda s: s["start_ms"])  # stable: mic before system on ties
    return segments


def summary_row(name: str, result: dict, seconds: float | None) -> str:
    cats = result["categories"]
    pick = lambda key: cats.get(key, {}).get("errors", 0)
    timing = "" if seconds is None else f"{seconds:>8.1f}s"
    return (f"  {name:<22} CER {result['cer'] * 100:5.1f}%  no-filler {result['cer_without_filler_runs'] * 100:5.1f}%"
            f"  S/D/I {result['substitutions']:>5}/{result['deletions']:>5}/{result['insertions']:>5}"
            f"  missing {pick('missing_speech'):>5}  extra {pick('extra_text'):>5}{timing}")


def load_inputs(args) -> tuple[list[dict], str, list[Unit], dict, dict, list[dict]]:
    text = args.reference.read_text(encoding="utf-8", errors="replace")
    cues, form = parse_reference(text)
    if not cues:
        shapes = [shape(line) for line in text.splitlines()[:8]]
        raise SystemExit(f"reference format not recognised; first line shapes: {shapes}")
    ref_units, speakers = reference_units(cues)
    conn = open_readonly(args.db)
    recording = find_recording(conn, args.title, args.recording_id)
    segments = saved_segments(conn, recording["id"])
    conn.close()
    return cues, form, ref_units, speakers, recording, segments


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("saved", "rerun"):
        command = sub.add_parser(name)
        command.add_argument("--db", type=Path, required=True, help="mojiroku.db (opened read-only)")
        command.add_argument("--reference", type=Path, required=True, help="Zoom .vtt/.txt transcript")
        command.add_argument("--title", help="recording title (exact, then substring)")
        command.add_argument("--recording-id")
        command.add_argument("--json-out", type=Path, help="also write the aggregate report as JSON")
        command.add_argument("--window-min", type=int, default=5)
        command.add_argument("--no-clip", action="store_true",
                             help="also score mojiroku text outside the reference's time span")
    sub.choices["saved"].add_argument("--recordings", type=Path,
                                      help="the app's recordings folder: adds mic/system loudness")
    rerun = sub.choices["rerun"]
    rerun.add_argument("--recordings", type=Path, required=True, help="the app's recordings folder")
    rerun.add_argument("--models", type=Path, required=True, help="the app's models folder")
    rerun.add_argument("--binary", type=Path, required=True, help="transcribe_cli built from this checkout")
    rerun.add_argument("--variants", default="turbo-greedy,turbo-beam5,full-greedy")
    rerun.add_argument("--language", help="ja, en, auto or mixed (default: the recording's job setting)")
    rerun.add_argument("--extra-arg", action="append", default=[],
                       help="extra transcribe_cli flag for every call (repeatable)")
    rerun.add_argument("--timeout", type=float, default=3600)
    args = parser.parse_args(argv)
    if not (args.title or args.recording_id):
        parser.error("pass --title or --recording-id")

    cues, form, ref_units, speakers, recording, segments = load_inputs(args)
    report = {
        "report": REPORT_VERSION,
        "normalization": NORMALIZATION,
        "reference": {"format": form, "cues": len(cues), "speakers": len([s for s in speakers if s != "R?"]),
                      "span": mmss(cues[-1]["end"] - cues[0]["start"])},
        "recording": {k: recording[k] for k in ("id", "source_type", "duration_ms", "language",
                                                 "created_at", "mic_offset_ms", "jobs")},
        "mojiroku_hygiene": hygiene(segments),
    }
    print(f"Reference: {form}, {len(cues):,} cues, {report['reference']['speakers']} speakers, "
          f"span {report['reference']['span']}")
    rec = report["recording"]
    print(f"Recording: {rec['source_type']}, {mmss(rec['duration_ms'])}, language {rec['language']}, "
          f"mic offset {rec['mic_offset_ms']} ms, jobs {rec['jobs']}")
    print(f"mojiroku hygiene: {report['mojiroku_hygiene']}")

    saved = analyse(ref_units, hypothesis_units(segments), args.window_min, segments, cues, not args.no_clip)
    support = saved["segment_diagnostics"].pop("_support")
    report["comparison"] = saved
    if args.command == "saved":
        print_report(saved)
        if args.recordings:
            report["mic_levels"] = mic_levels(args.recordings, recording, segments, support)
            print("\nMic-track loudness during mic segments (dBFS; q1, median, q3):")
            for group, value in report["mic_levels"].items():
                print(f"  {group}: {value}")
    else:
        job_language = next((job["stt_lang"] for job in reversed(rec["jobs"])
                             if job["kind"] == "transcribe" and job.get("stt_lang")), None)
        language = args.language or job_language or "auto"
        tracks = recording_tracks(args.recordings, recording)
        print(f"Re-running {len(tracks)} track(s), language {language}, extra args {args.extra_arg}")
        rows = [summary_row("saved in app", saved, None)]
        report["variants"] = {}
        for name in args.variants.split(","):
            decoder, model = VARIANTS[name]
            if not (args.models / model).exists():
                rows.append(f"  {name:<22} skipped: {model} is not downloaded")
                continue
            per_track, seconds = [], 0.0
            for audio, speaker, shift in tracks:
                track_segments, took = transcribe(args.binary, audio, args.models, language, decoder,
                                                  model, args.extra_arg, args.timeout)
                per_track.append((track_segments, speaker, shift))
                seconds += took
            variant_segments = merged_segments(per_track)
            result = analyse(ref_units, hypothesis_units(variant_segments), args.window_min,
                             variant_segments, cues, not args.no_clip)
            result["segment_diagnostics"].pop("_support")
            report["variants"][name] = {"pipeline_seconds": seconds, "hygiene": hygiene(variant_segments),
                                        "comparison": result}
            rows.append(summary_row(name, result, seconds))
            print(rows[-1], flush=True)
        print("\nSummary (reference = the exported transcript):")
        print("\n".join(rows))
        for name, value in report["variants"].items():
            print(f"\n=== {name} ===")
            print(f"hygiene: {value['hygiene']}")
            print_report(value["comparison"])
    if args.json_out:
        args.json_out.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")


if __name__ == "__main__":
    try:
        main()
    except SystemExit:
        raise
    except Exception as error:  # noqa: BLE001 - never echo data-bearing messages
        tb = error.__traceback__
        while tb and tb.tb_next:
            tb = tb.tb_next
        raise SystemExit(f"failed: {type(error).__name__} at line {tb.tb_lineno if tb else '?'}")
