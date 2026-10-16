import importlib.util
import io
from pathlib import Path
import re
import random
import tempfile
import unittest
from unittest import mock
import zipfile

spec = importlib.util.spec_from_file_location("prepare_data", Path(__file__).with_name("setup-separator-training-data.py"))
data = importlib.util.module_from_spec(spec); spec.loader.exec_module(data)


class Response(io.BytesIO):
    def __init__(self, payload, start, total, status=206):
        super().__init__(payload); self.status = status
        self.headers = {"Content-Range": f"bytes {start}-{start + len(payload) - 1}/{total}"}
    def geturl(self): return "https://zenodo.org/data.zip"


class TrainingDataTests(unittest.TestCase):
    def archive(self):
        output = io.BytesIO()
        with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as archive:
            archive.writestr("voice/a.wav", b"sample" * 20000)
            archive.writestr("padding.bin", random.Random(19).randbytes(150000))
            archive.writestr("../unsafe.wav", b"never extract")
        return output.getvalue()

    def test_range_reader_extracts_and_verifies_only_requested_members(self):
        source = self.archive()
        def opener(request, timeout):
            match = re.fullmatch(r"bytes=(\d*)-(\d*)", request.headers["Range"])
            start, end = match.groups()
            first = int(start) if start else max(0, len(source) - int(end))
            last = int(end) if start else len(source) - 1
            return Response(source[first:last + 1], first, len(source))
        with mock.patch.object(data.urllib.request, "urlopen", side_effect=opener), tempfile.TemporaryDirectory() as root:
            with data.RemoteZip("https://zenodo.org/data.zip") as remote, zipfile.ZipFile(remote) as archive:
                records = []; destination = Path(root)
                data.retain(archive, "voice/a.wav", destination, "vocalset", "voice-a", records)
                self.assertEqual((destination / records[0]["path"]).read_bytes(), b"sample" * 20000)
                self.assertEqual(len(records[0]["sha256"]), 64)
                with self.assertRaises(RuntimeError): data.retain(archive, "../unsafe.wav", destination, "vocalset", "voice-a", records)
                (destination / records[0]["path"]).write_bytes(b"changed")
                with self.assertRaises(RuntimeError): data.retain(archive, "voice/a.wav", destination, "vocalset", "voice-a", records)

    def test_ignored_range_and_download_budget_fail_closed(self):
        with mock.patch.object(data.urllib.request, "urlopen", return_value=Response(b"data", 0, 4, status=200)):
            with self.assertRaises(RuntimeError): data.RemoteZip("https://zenodo.org/data.zip")
        with mock.patch.object(data.urllib.request, "urlopen", return_value=Response(b"data", 0, 4)):
            with self.assertRaises(RuntimeError): data.RemoteZip("https://zenodo.org/data.zip", byte_budget=3)

    def test_valid_large_member_uses_bounded_reads(self):
        output = io.BytesIO()
        payload = b"wave-pcm" * (17 * 1024 * 1024 // 8)
        with zipfile.ZipFile(output, "w", zipfile.ZIP_STORED) as archive:
            archive.writestr("long.wav", payload)
        source = output.getvalue()
        def opener(request, timeout):
            start, end = re.fullmatch(r"bytes=(\d*)-(\d*)", request.headers["Range"]).groups()
            first = int(start) if start else max(0, len(source) - int(end))
            last = int(end) if start else len(source) - 1
            return Response(source[first:last + 1], first, len(source))
        with mock.patch.object(data.urllib.request, "urlopen", side_effect=opener), tempfile.TemporaryDirectory() as root:
            with data.RemoteZip("https://zenodo.org/data.zip") as remote, zipfile.ZipFile(remote) as archive:
                records = []
                data.retain(archive, "long.wav", Path(root), "vocalset", "voice-a", records)
                self.assertEqual(records[0]["bytes"], len(payload))
                self.assertEqual((Path(root) / records[0]["path"]).read_bytes(), payload)

    def test_vocal_selection_preserves_singer_groups_deterministically(self):
        names = [f"FULL/{voice}/scales/{i}.wav" for voice in ("female1", "female2", "male1", "male2") for i in range(8)]
        selected = data.select_vocals(names, 3)
        self.assertEqual(selected, data.select_vocals(list(reversed(names)), 3))
        self.assertEqual(len(selected), 12)
        self.assertEqual(len(set(group for _, group in selected)), 4)


if __name__ == "__main__": unittest.main()
