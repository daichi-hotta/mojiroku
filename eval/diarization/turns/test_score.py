import json
import tempfile
import unittest
from pathlib import Path

from score import score


def write(path: Path, data) -> str:
    path.write_text(json.dumps(data))
    return str(path)


class ScoreTest(unittest.TestCase):
    def setUp(self):
        self.dir = Path(tempfile.mkdtemp())
        self.ref = write(self.dir / "ref.json", {"turns": [
            {"speaker": "A", "start": 0.0, "end": 4.0, "text": "one two three"},
            {"speaker": "B", "start": 4.2, "end": 4.8, "text": "yes"},
            {"speaker": "A", "start": 5.0, "end": 9.0, "text": "four five six"},
        ]})

    def seg(self, a, b, spk, text):
        return {"start_ms": int(a * 1000), "end_ms": int(b * 1000), "speaker_id": spk, "text": text}

    def test_perfect_prediction(self):
        pred = write(self.dir / "p.json", [
            self.seg(0, 4, "S1", "one two three"), self.seg(4.2, 4.8, "S2", "yes"), self.seg(5, 9, "S1", "four five six"),
        ])
        r = score(self.ref, pred)
        self.assertEqual((r["straddling"], r["mislabelled_time"], r["turn_order_error"], r["short_replies_ok"]), (0, 0.0, 0.0, "1/1"))
        self.assertEqual(r["text_recall"], 1.0)

    def test_swallowed_reply(self):
        # the reply is labelled with the surrounding speaker
        pred = write(self.dir / "p.json", [
            self.seg(0, 4, "S1", "one two three"), self.seg(4.2, 4.8, "S1", "yes"), self.seg(5, 9, "S1", "four five six"),
        ])
        r = score(self.ref, pred)
        self.assertEqual(r["short_replies_ok"], "0/1")
        self.assertGreater(r["turn_order_error"], 0)
        self.assertGreater(r["mislabelled_time"], 0)

    def test_straddling_line(self):
        pred = write(self.dir / "p.json", [
            self.seg(0, 4.8, "S1", "one two three yes"), self.seg(5, 9, "S1", "four five six"),
        ])
        self.assertEqual(score(self.ref, pred)["straddling"], 1)


if __name__ == "__main__":
    unittest.main()
