import contextlib
import io
import json
from pathlib import Path
import random
import sqlite3
import sys
import tempfile
import unittest
from unittest import mock

import reference
from metrics import edit_distance


def rebuild(ops, ref, hyp):
    return ("".join(ref[i] for op, i, _ in ops if i is not None),
            "".join(hyp[j] for op, _, j in ops if j is not None))


class ParseTests(unittest.TestCase):
    def test_vtt_with_speaker_prefixes(self):
        raw = ("WEBVTT\n\n1\n00:00:01.000 --> 00:00:03.500\nアリス: こんにちは。\n\n"
               "2\n00:00:04.000 --> 00:00:06.000\nボブ: はい、どうも\n\n"
               "3\n00:00:06.000 --> 00:00:07.000\nアリス: 時間: 十時\n")
        cues, form = reference.parse_reference(raw)
        self.assertEqual(form, "vtt/srt")
        self.assertEqual([c["speaker"] for c in cues], ["アリス", "ボブ", "アリス"])
        self.assertEqual(cues[2]["text"], "時間: 十時")
        self.assertEqual(cues[0]["start"], 1000)

    def test_single_colon_prefix_is_text(self):
        raw = "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\n議題: 予算\n"
        cues, _ = reference.parse_reference(raw)
        self.assertIsNone(cues[0]["speaker"])
        self.assertEqual(cues[0]["text"], "議題:予算")

    def test_bracket_name_time_txt(self):
        raw = "[Alice Smith] 13:02:11\nこんにちは\n続きです\n[Bob] 13:02:20\nはい\n"
        cues, form = reference.parse_reference(raw)
        self.assertEqual(form, "bracket-name-time")
        self.assertEqual([c["speaker"] for c in cues], ["Alice Smith", "Bob"])
        self.assertEqual(cues[0]["text"], "こんにちは続きです")
        self.assertEqual(cues[0]["end"], cues[1]["start"])

    def test_wall_clock_crossing_midnight_stays_monotonic(self):
        raw = "[A] 23:59:50\nあ\n[B] 00:00:05\nい\n"
        cues, _ = reference.parse_reference(raw)
        self.assertEqual(cues[1]["start"] - cues[0]["start"], 15_000)

    def test_speaker_labels_are_anonymous_and_ranked(self):
        cues = [{"start": 0, "end": 1000, "speaker": "Quiet", "text": "あ"},
                {"start": 1000, "end": 2000, "speaker": "Loud", "text": "いいいい"}]
        units, totals = reference.reference_units(cues)
        self.assertEqual(units[0].speaker, "R2")
        self.assertEqual(totals, {"R1": 4, "R2": 1})

    def test_shape_hides_content(self):
        self.assertEqual(reference.shape("[田中] 10:02:03"), "[JJ] 99:99:99")


class AlignTests(unittest.TestCase):
    def check(self, ref, hyp):
        ops, unaligned = reference.align(ref, hyp)
        self.assertEqual(rebuild(ops, ref, hyp), (ref, hyp))
        errors = sum(op != "=" for op, _, _ in ops)
        for op, i, j in ops:
            if op == "=":
                self.assertEqual(ref[i], hyp[j])
            if op == "S":
                self.assertNotEqual(ref[i], hyp[j])
        return errors, unaligned

    def test_small_inputs_match_exact_edit_distance(self):
        rng = random.Random(7)
        for _ in range(200):
            ref = "".join(rng.choice("あいうアイ漢字") for _ in range(rng.randint(0, 25)))
            hyp = "".join(rng.choice("あいうアイ漢字") for _ in range(rng.randint(0, 25)))
            errors, _ = self.check(ref, hyp)
            self.assertEqual(errors, edit_distance(list(ref), list(hyp)))

    def test_anchored_alignment_of_long_text_is_near_exact(self):
        rng = random.Random(11)
        alphabet = [chr(c) for c in range(0x3041, 0x3094)] + [chr(c) for c in range(0x4e00, 0x4e40)]
        ref = "".join(rng.choice(alphabet) for _ in range(3000))
        hyp = list(ref)
        edits = 0
        for _ in range(120):
            at = rng.randrange(len(hyp))
            kind = rng.choice("SDI")
            if kind == "S":
                hyp[at] = "ズ"
            elif kind == "D":
                del hyp[at]
            else:
                hyp.insert(at, "ズ")
            edits += 1
        del hyp[1000:1200]  # one dropped stretch
        hyp = "".join(hyp)
        with mock.patch.object(reference, "DP_LIMIT", 10_000):
            errors, unaligned = self.check(ref, hyp)
        self.assertEqual(unaligned, 0)
        self.assertLessEqual(errors, edits + 200)
        self.assertGreaterEqual(errors, edit_distance(list(ref[:400]), list(hyp[:400])))

    def test_unrelated_text_without_anchors_is_counted_unaligned(self):
        with mock.patch.object(reference, "DP_LIMIT", 10):
            errors, unaligned = self.check("あ" * 20, "い" * 20)
        self.assertEqual(errors, 40)
        self.assertEqual(unaligned, 40)


class CategoryTests(unittest.TestCase):
    def test_categories(self):
        cases = {
            ("あいうえおかきくけこさし", ""): "missing_speech",
            ("", "ご視聴ありがとうございました"): "extra_text",
            ("えーと", ""): "filler",
            ("あの", "はい"): "filler",
            ("三", "3"): "number",
            ("ok", "オーケー"): "latin",
            ("サーバ", "サーバー"): "katakana",
            ("は", ""): "particle",
            ("が", "を"): "particle",
            ("できる", "出来る"): "kana_vs_kanji",
            ("会議", "開議"): "kanji",
            ("ました", "ます"): "kana",
        }
        for (ref, hyp), expected in cases.items():
            with self.subTest(ref=ref, hyp=hyp):
                self.assertEqual(reference.categorise(ref, hyp), expected)

    def test_chance_matches_do_not_split_an_invented_stretch(self):
        ref = "あいうえお" + "た" + "かきくけこ"
        hyp = "あいうえお" + "さしすせそ" + "た" + "ちつてとな" + "かきくけこ"
        ops, _ = reference.align(ref, hyp)
        runs = reference.edit_runs(ops)
        self.assertEqual(len(runs), 1)
        part = lambda idx, text: "".join(text[n] for n in idx)
        self.assertEqual(reference.categorise(part(runs[0]["ref"], ref), part(runs[0]["hyp"], hyp)),
                         "extra_text")
        self.assertEqual(runs[0]["cost"], 10)

    def test_separate_small_errors_stay_separate(self):
        ops, _ = reference.align("あいうえおかきくけこ", "あXうえおかきYけこ")
        self.assertEqual([r["cost"] for r in reference.edit_runs(ops)], [1, 1])

    def test_long_vowel_alone_is_not_katakana(self):
        self.assertEqual(reference.categorise("ー", ""), "other")


class AnalyseTests(unittest.TestCase):
    def test_text_after_the_reference_ends_is_not_scored(self):
        ref = reference.spread("あいうえおかきくけこさしすせそ", 60_000, 70_000, "R1", 0)
        hyp = (reference.spread("あいうえおかきくけこさしすせそ", 0, 10_000, "S1", 0)
               + reference.spread("たちつてと" * 6, 100_000, 120_000, "S1", 1))
        result = reference.analyse(ref, hyp)
        self.assertEqual(result["errors"], 0)
        self.assertEqual(result["mojiroku_chars_outside_reference_span"], {"S1": 30})
        unclipped = reference.analyse(ref, hyp, clip=False)
        self.assertEqual(unclipped["errors"], 30)

    def test_segment_diagnostics(self):
        cues = [{"start": 0, "end": 4000, "speaker": "A", "text": "来週の打ち合わせは水曜日にしましょう"},
                {"start": 10_000, "end": 13_000, "speaker": "B", "text": "資料をすぐに送ります"}]
        segments = [
            {"start_ms": 0, "end_ms": 4000, "text": "来週の打ち合わせは水曜日にしましょう", "speaker_id": "S1"},
            {"start_ms": 1000, "end_ms": 4000, "text": "打ち合わせ水曜日しましょう", "speaker_id": "self"},
            {"start_ms": 20_000, "end_ms": 21_000, "text": "ご視聴ありがとうございました", "speaker_id": "S1"},
        ]
        ref_units, _ = reference.reference_units(cues)
        result = reference.analyse(ref_units, reference.hypothesis_units(segments),
                                   segments=segments, cues=cues, clip=False)
        diagnostics = result["segment_diagnostics"]
        self.assertEqual(diagnostics["mojiroku_segments_not_in_reference"]["mic"],
                         {"segments": 1, "chars": 13, "while_system_track_has_speech": 1,
                          "resembles_nearby_system_text": 1})
        self.assertEqual(diagnostics["reference_cues_missed"],
                         {"system": {"cues": 1, "chars": 10, "no_mojiroku_segment_then": 1}})
        self.assertEqual(diagnostics["known_hallucination_phrases"],
                         {"ご視聴ありがとうございました": {"not_in_reference": 1}})


class HygieneTests(unittest.TestCase):
    def test_mic_echo_of_remote_speech_is_counted(self):
        segments = [
            {"start_ms": 0, "end_ms": 4000, "text": "来週の打ち合わせは水曜日にしましょう", "speaker_id": "S1"},
            {"start_ms": 900, "end_ms": 4200, "text": "来週の打ち合わせは水曜日にしましょう。", "speaker_id": "self"},
            {"start_ms": 9000, "end_ms": 11000, "text": "わかりました、資料を送ります", "speaker_id": "self"},
        ]
        report = reference.hygiene(segments)
        self.assertEqual(report["self_segments"], 2)
        self.assertEqual(report["self_segments_echoing_remote"], 1)


class EndToEndTests(unittest.TestCase):
    SECRET_NAME = "秘密花子"
    SECRET_TEXT = "極秘プロジェクト鈴蘭の予算は三千万円です"

    def make_db(self, path: Path):
        conn = sqlite3.connect(path)
        conn.executescript("""
            CREATE TABLE recordings (id TEXT PRIMARY KEY, source_type TEXT, title TEXT,
              duration_ms INTEGER, sample_rate INTEGER, language TEXT, created_at TEXT,
              mic_offset_ms INTEGER);
            CREATE TABLE segments (id INTEGER PRIMARY KEY, recording_id TEXT, idx INTEGER,
              start_ms INTEGER, end_ms INTEGER, text TEXT, speaker_id TEXT);
            CREATE TABLE jobs (id TEXT, recording_id TEXT, kind TEXT, status TEXT, params TEXT,
              created_at TEXT);
        """)
        conn.execute("INSERT INTO recordings VALUES ('r1','live','秘密MTG 2026-10-29',60000,48000,"
                     "'ja','2026-10-29T00:00:00Z',120)")
        conn.execute("INSERT INTO recordings VALUES ('r2','mic','別の秘密会議',1000,48000,'ja','x',NULL)")
        segments = [(0, 5000, "はい、よろしくお願いします", "self"),
                    (6000, 12000, "極秘プロジェクト鈴蘭の予算は三千万です", "S1"),
                    (13000, 15000, "ご視聴ありがとうございました", "S1")]
        for n, (a, b, text, speaker) in enumerate(segments):
            conn.execute("INSERT INTO segments (recording_id, idx, start_ms, end_ms, text, speaker_id) "
                         "VALUES ('r1', ?, ?, ?, ?, ?)", (n, a, b, text, speaker))
        conn.execute("INSERT INTO jobs VALUES ('j','r1','transcribe','done',?, 'x')",
                     (json.dumps({"stt_lang": "ja", "lang": "ja", "transcription_model": "turbo",
                                  "secret": self.SECRET_NAME}),))
        conn.commit()
        conn.close()

    def test_report_never_contains_text_or_names(self):
        with tempfile.TemporaryDirectory() as folder:
            db, ref, out = Path(folder, "m.db"), Path(folder, "z.vtt"), Path(folder, "r.json")
            self.make_db(db)
            ref.write_text(
                "WEBVTT\n\n00:00:02.000 --> 00:00:07.000\n大地: はい、よろしくお願いします\n\n"
                f"00:00:08.000 --> 00:00:14.000\n{self.SECRET_NAME}: {self.SECRET_TEXT}\n\n"
                f"00:00:15.000 --> 00:00:16.000\n{self.SECRET_NAME}: えーと\n\n"
                "00:00:16.000 --> 00:00:17.000\n大地: はい\n", encoding="utf-8")
            argv = ["saved", "--db", str(db), "--reference", str(ref), "--title", "秘密MTG",
                    "--json-out", str(out)]
            buffer = io.StringIO()
            with contextlib.redirect_stdout(buffer):
                reference.main(argv)
            printed = buffer.getvalue() + out.read_text(encoding="utf-8")
            # Phrases from the fixed public HALLUCINATIONS list may be named; nothing else.
            for secret in (self.SECRET_NAME, "大地", "鈴蘭", "予算", "よろしく", "秘密", "別の"):
                self.assertNotIn(secret, printed)
            report = json.loads(out.read_text(encoding="utf-8"))
            comparison = report["comparison"]
            self.assertEqual(report["reference"]["speakers"], 2)
            self.assertLess(abs(comparison["clock_offset_ms"]["median"] - 2000), 500)
            self.assertEqual(report["mojiroku_hygiene"]["known_hallucination_phrases"], 1)
            self.assertEqual(comparison["errors"], sum(v["errors"] for v in comparison["categories"].values()))
            self.assertEqual(report["recording"]["jobs"][0]["stt_lang"], "ja")
            self.assertEqual(set(comparison["speaker_overlap_chars"]), {"R1", "R2"})

    def test_rerun_merges_tracks_and_keeps_text_private(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            db, ref, out = root / "m.db", root / "z.vtt", root / "r.json"
            self.make_db(db)
            ref.write_text(
                "WEBVTT\n\n00:00:00.000 --> 00:00:05.000\n大地: はい、よろしくお願いします\n\n"
                f"00:00:06.000 --> 00:00:12.000\n{self.SECRET_NAME}: {self.SECRET_TEXT}\n\n"
                "00:00:12.000 --> 00:00:13.000\n大地: はい\n", encoding="utf-8")
            recordings, models = root / "recordings", root / "models"
            recordings.mkdir()
            models.mkdir()
            for name in ("r1-mic.wav", "r1-system.wav"):
                (recordings / name).write_bytes(b"")
            (models / "ggml-large-v3-turbo-q5_0.bin").write_bytes(b"")
            fake = root / "transcribe_cli"
            fake.write_text(
                f"#!{sys.executable}\n"
                "import json, sys\n"
                "mic = sys.argv[1].endswith('-mic.wav')\n"
                "seg = {'start_ms': 0, 'end_ms': 5000, 'text': 'はい、よろしくお願いします'} if mic else "
                f"{{'start_ms': 5880, 'end_ms': 11880, 'text': '{self.SECRET_TEXT}'}}\n"
                "print('progress', file=sys.stderr)\n"
                "print(json.dumps({'transcript': {'segments': [seg]}, 'pipeline_seconds': 1.5}))\n",
                encoding="utf-8")
            fake.chmod(0o755)
            argv = ["rerun", "--db", str(db), "--reference", str(ref), "--recording-id", "r1",
                    "--recordings", str(recordings), "--models", str(models), "--binary", str(fake),
                    "--variants", "turbo-greedy,full-greedy", "--json-out", str(out)]
            buffer = io.StringIO()
            with contextlib.redirect_stdout(buffer):
                reference.main(argv)
            printed = buffer.getvalue() + out.read_text(encoding="utf-8")
            for secret in (self.SECRET_NAME, "大地", "鈴蘭", "予算", "よろしく"):
                self.assertNotIn(secret, printed)
            self.assertIn("full-greedy            skipped", printed)
            variant = json.loads(out.read_text(encoding="utf-8"))["variants"]["turbo-greedy"]
            self.assertEqual(variant["comparison"]["errors"], 2)  # the final はい is not in the fake output
            self.assertEqual(variant["pipeline_seconds"], 3.0)
            # The 120 ms mic offset shifts the mic track like merge_tracks does.
            self.assertEqual(set(variant["comparison"]["speaker_overlap_chars"]["R1"]), {"system"})

    def test_unmatched_title_does_not_list_other_titles(self):
        with tempfile.TemporaryDirectory() as folder:
            db = Path(folder, "m.db")
            self.make_db(db)
            conn = reference.open_readonly(db)
            with self.assertRaises(SystemExit) as raised:
                reference.find_recording(conn, "秘密", None)
            conn.close()
            self.assertIn("2 recordings matched", str(raised.exception))
            self.assertNotIn("別の", str(raised.exception))


if __name__ == "__main__":
    unittest.main()
