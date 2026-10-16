import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import numpy as np
import soundfile as sf
import torch


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    result = importlib.util.module_from_spec(spec); spec.loader.exec_module(result)
    return result


experiments = module("experiments", "vocal-separator-experiments.py")
stream, base = experiments.stream, experiments.base
download = module("vocadito_download", "setup-vocadito-evaluation.py")
torch.set_num_threads(2)


class ExperimentTests(unittest.TestCase):
    def test_binary_checkpoint_retains_width_context_and_exact_complement(self):
        model = base.Separator(width=16, classes=base.BINARY_CLASSES, complex_masks=True).eval()
        model.inference_frames = 48000; model.pad_context = True
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "own.pt"
            experiments.checkpoint(model, 5, path)
            loaded = base.load_model(path)
            self.assertEqual(loaded.width, 16); self.assertTrue(loaded.pad_context)
            self.assertEqual(loaded.inference_frames, 48000)
            for length in [1, 510, 50001]:
                x = torch.randn(2, length) * .02
                result = base.predict(loaded, x)
                self.assertEqual(result.shape, (2, 2, length))
                self.assertTrue(torch.isfinite(result).all())
                torch.testing.assert_close(result.sum(0), x, atol=1e-7, rtol=1e-5)
            self.assertEqual(float(base.predict(loaded, torch.zeros(2, 31)).abs().max()), 0)

    def test_gain_loss_corrects_attenuation_and_absent_target_stays_finite(self):
        target = torch.randn(2, 2, 2, 9, 13, dtype=torch.complex64)
        target[0, 1] = 0
        quiet = (target * .2).requires_grad_(True)
        supervised = experiments.fidelity_loss(quiet, target)
        self.assertGreater(float(supervised), float(experiments.fidelity_loss(quiet, target, 0)))
        self.assertAlmostEqual(float(experiments.fidelity_loss(target, target)), 0, places=10)
        supervised.backward(); self.assertTrue(torch.isfinite(quiet.grad).all())
        silence = torch.zeros_like(target, requires_grad=True)
        loss = experiments.fidelity_loss(silence, torch.zeros_like(target))
        loss.backward(); self.assertTrue(torch.isfinite(silence.grad).all())

    def test_paired_recipes_keep_absences_and_evaluation_voices_out_of_training(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            dataset = stream.Mixtures.__new__(stream.Mixtures)
            dataset.seed = 17; dataset.bank = root; dataset.voices_root = root; dataset.data = root
            dataset.sources = dict(files=[]); dataset.songs = {}
            dataset.manifest = dict(scenes=[dict(path=s, split=s, scoreID=s) for s in ["train", "validation", "test"]])
            dataset.voice_manifest = dict(voices=[dict(path=s, split=s, singer=s) for s in ["train", "validation", "test"]])
            for split in ["train", "validation", "test"]:
                folder = root / split; folder.mkdir()
                for filename in [f"part{i}.flac" for i in range(5)] + ["dry.flac", "wet.flac"]:
                    audio = np.sin(np.arange(240000) * .01)[:, None] * np.array([[.1, .07]])
                    sf.write(folder / filename, audio, 48000, subtype="PCM_24")
            for category in ["no_vocals", "solo_vocals", "hard"]:
                # Use native index1 for negative fixture (no Slakh files here).
                a, ra = dataset.sample("train", 1, 1., category=category)
                b, rb = dataset.sample("train", 1, 4., category=category)
                torch.testing.assert_close(a, b[..., :48000])
                self.assertEqual(ra["sourceGroups"], rb["sourceGroups"])
                self.assertTrue(all(g.endswith(":train") for g in ra["sourceGroups"]))
                if category == "no_vocals": self.assertEqual(float(b[1].abs().max()), 0)
                if category == "solo_vocals": self.assertEqual(float(b[0].abs().max()), 0)
                again, _ = dataset.sample("train", 1, 4., category=category)
                torch.testing.assert_close(b, again)
            with self.assertRaisesRegex(ValueError, "evaluation only"):
                dataset.sample("train", 0, external=dict(path=root / "voice.wav", singer="fresh"))

    def test_selection_rejects_attenuation_and_hallucinated_vocals(self):
        active = dict(vocals=dict(absent=False, sdr=8., siSDR=15., siSDRImprovement=10., referenceGain=.95), accompaniment=dict(absent=False))
        absent = dict(vocals=dict(absent=True), vocalEnergyFraction=.005)
        self.assertTrue(experiments.summarize([active, absent], [1e-7])["numericalGatePassed"])
        quiet = dict(vocals=dict(active["vocals"], referenceGain=.2), accompaniment=dict(absent=False))
        self.assertFalse(experiments.summarize([quiet, absent], [0])["numericalGatePassed"])
        false = dict(vocals=dict(absent=True), vocalEnergyFraction=.05)
        self.assertFalse(experiments.summarize([active, false], [0])["numericalGatePassed"])

    def test_vocadito_checksum_paths_and_cached_bytes_are_verified(self):
        def archive(extra=None):
            buffer = io.BytesIO()
            with zipfile.ZipFile(buffer, "w") as z:
                for i in range(40): z.writestr(f"Audio/vocadito_{i + 1}.wav", b"fixture only")
                if extra: z.writestr(extra, b"unsafe")
            return buffer.getvalue()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); payload = archive()
            with self.assertRaisesRegex(ValueError, "checksum"): download.install(payload, root)
            with patch.object(download, "MD5", hashlib.md5(payload).hexdigest()):
                result = download.install(payload, root)
                self.assertEqual(len(result["files"]), 40)
                self.assertIn("never training", result["use"])
                (root / "Audio/vocadito_1.wav").write_bytes(b"tampered")
                with self.assertRaisesRegex(ValueError, "differs"): download.install(payload, root)
            payload = archive("../escape")
            with patch.object(download, "MD5", hashlib.md5(payload).hexdigest()):
                with self.assertRaisesRegex(ValueError, "Unsafe"): download.install(payload, root / "bad")
            self.assertFalse((root.parent / "escape").exists())
            linked = root / "linked"; linked.mkdir(); (linked / "Audio").symlink_to(root / "Audio", target_is_directory=True)
            payload = archive()
            with patch.object(download, "MD5", hashlib.md5(payload).hexdigest()):
                with self.assertRaisesRegex(ValueError, "escapes"): download.install(payload, linked)


if __name__ == "__main__": unittest.main()
