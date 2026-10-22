"""Build a two-speaker reference dialogue with exact turn boundaries (Issue #65).

The real recordings used elsewhere in eval/diarization only have hand-marked
intervals accurate to a few seconds, which cannot show whether a transcript
segment straddles a speaker change. This script synthesises a dialogue with the
macOS `say` voices so every turn's start, end, speaker and text are known exactly.

What it exercises:
- quick hand-offs (0.15-0.3 s gaps) where Whisper tends to glue two turns together
- short replies (「はい」「なるほど」, "Right.") between longer turns
- normal pauses, for contrast

It is synthetic speech, not a meeting benchmark: it checks ordering and
attribution around turn changes, not recognition quality on real voices.
The script text below is original; the audio is generated locally and ignored.

Usage (macOS):
    python3 eval/diarization/turns/make_dialogue.py            # both languages
    python3 eval/diarization/turns/make_dialogue.py --lang ja
Writes eval/diarization/turns/out/<lang>.wav (16 kHz mono) and <lang>.json.
"""
from __future__ import annotations

import argparse
import json
import subprocess
import tempfile
import wave
from pathlib import Path

OUT = Path(__file__).resolve().parent / "out"
RATE = 16000

# (speaker, gap before this turn in seconds, text)
DIALOGUES = {
    "ja": {
        "voices": {"A": "Kyoko", "B": "Reed (Japanese (Japan))"},
        "turns": [
            ("A", 0.5, "では今週のリリースの範囲を確認します。まず、履歴画面の改善は予定どおり入りました。"),
            ("B", 0.2, "ありがとうございます。検索の速さはどうでしたか。"),
            ("A", 0.3, "千件の録音で試して、一秒かからずに結果が出ました。"),
            ("B", 0.2, "なるほど。"),
            ("A", 0.15, "ただ、古い録音はタイトルが空のものが多くて、一覧で見分けにくいです。"),
            ("B", 0.8, "それなら、種類と日時で名前を付ける案がありましたよね。あれを先に入れましょう。"),
            ("A", 0.2, "はい。"),
            ("B", 0.15, "テストはどこまで終わっていますか。"),
            ("A", 0.3, "結合テストが半分くらいです。金曜日までには終わらせます。"),
            ("B", 0.25, "それなら月曜日に出せそうですね。"),
            ("A", 0.2, "一点だけ懸念があって、古いマックでのメモリの使い方をまだ測れていません。"),
            ("B", 0.15, "それは私の方で測っておきます。"),
            ("A", 0.6, "助かります。結果が出たら共有してください。"),
            ("B", 0.2, "わかりました。"),
            ("A", 0.3, "では今日はここまでにしましょう。お疲れさまでした。"),
        ],
    },
    "en": {
        "voices": {"A": "Samantha", "B": "Ralph"},
        "turns": [
            ("A", 0.5, "Let's go over the release scope for this week. The history screen changes landed on schedule."),
            ("B", 0.2, "Thanks. How fast is search now?"),
            ("A", 0.3, "I tried it with a thousand recordings and results came back in under a second."),
            ("B", 0.2, "Right."),
            ("A", 0.15, "The catch is that older recordings often have no title, so they are hard to tell apart."),
            ("B", 0.8, "There was a plan to name them by kind and time. Let's ship that first."),
            ("A", 0.2, "Okay."),
            ("B", 0.15, "How far along are the tests?"),
            ("A", 0.3, "Integration tests are about half done. I will finish them by Friday."),
            ("B", 0.25, "Then we can release on Monday."),
            ("A", 0.2, "One concern. We still have not measured memory use on older Macs."),
            ("B", 0.15, "I will measure that."),
            ("A", 0.6, "Thanks. Please share the numbers when you have them."),
            ("B", 0.2, "Sure."),
            ("A", 0.3, "Let's wrap up here. Thanks, everyone."),
        ],
    },
}


def synth(voice: str, text: str, path: Path) -> list[int]:
    """Render one turn with `say` and return 16-bit mono samples at RATE."""
    subprocess.run(
        ["say", "-v", voice, "-o", str(path), f"--data-format=LEI16@{RATE}", "--file-format=WAVE", text],
        check=True,
    )
    with wave.open(str(path)) as w:
        assert w.getframerate() == RATE and w.getnchannels() == 1 and w.getsampwidth() == 2
        raw = w.readframes(w.getnframes())
    samples = [int.from_bytes(raw[i : i + 2], "little", signed=True) for i in range(0, len(raw), 2)]
    # `say` pads with silence; trim it so the recorded turn bounds are the speech bounds.
    thresh = 200
    first = next((i for i, s in enumerate(samples) if abs(s) > thresh), 0)
    last = len(samples) - next((i for i, s in enumerate(reversed(samples)) if abs(s) > thresh), 0)
    return samples[first:last]


def build(lang: str) -> None:
    spec = DIALOGUES[lang]
    OUT.mkdir(exist_ok=True)
    pcm: list[int] = []
    turns = []
    with tempfile.TemporaryDirectory() as tmp:
        for i, (spk, gap, text) in enumerate(spec["turns"]):
            pcm.extend([0] * int(gap * RATE))
            start = len(pcm) / RATE
            pcm.extend(synth(spec["voices"][spk], text, Path(tmp) / f"{i}.wav"))
            turns.append({"speaker": spk, "start": round(start, 3), "end": round(len(pcm) / RATE, 3), "text": text})
    pcm.extend([0] * int(0.5 * RATE))
    wav = OUT / f"{lang}.wav"
    with wave.open(str(wav), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(b"".join(s.to_bytes(2, "little", signed=True) for s in pcm))
    (OUT / f"{lang}.json").write_text(
        json.dumps({"lang": lang, "voices": spec["voices"], "turns": turns}, ensure_ascii=False, indent=1)
    )
    print(f"{wav}: {len(pcm) / RATE:.1f} s, {len(turns)} turns")


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--lang", choices=sorted(DIALOGUES), action="append")
    for lang in ap.parse_args().lang or sorted(DIALOGUES):
        build(lang)
