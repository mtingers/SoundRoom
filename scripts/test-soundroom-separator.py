import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import numpy as np
import soundfile as sf
import torch


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    loaded = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(loaded)
    return loaded


separator = module("separator", "soundroom-separator.py")
generator = module("generator", "generate-separator-corpus.py")
recordings = module("recordings", "generate-separator-recordings.py")
torch.set_num_threads(2)


class SeparatorTests(unittest.TestCase):
    def test_windowed_inference_preserves_frames_stereo_gain_and_additivity(self):
        torch.manual_seed(51)
        model = separator.Separator().eval()
        for length in [1, 511, 5001]:
            audio = torch.randn(2, length) * 0.07
            prediction = separator.predict(model, audio, chunk_frames=4096)
            self.assertEqual(tuple(prediction.shape), (4, 2, length))
            self.assertTrue(torch.isfinite(prediction).all())
            torch.testing.assert_close(prediction.sum(0), audio, atol=2e-7, rtol=1e-5)
            extract = prediction[2]
            remove = audio - extract
            torch.testing.assert_close(extract + remove, audio, atol=2e-8, rtol=1e-5)
        self.assertEqual(float(separator.predict(model, torch.zeros(2, 33)).abs().max()), 0)

    def test_training_gradient_and_checkpoint_roundtrip(self):
        torch.manual_seed(52)
        model = separator.Separator()
        audio = torch.randn(1, 4, 2, 4096) * 0.03
        target = separator.spectrum(audio)
        prediction = model(target.sum(1))
        loss = (prediction - target).abs().mean()
        loss.backward()
        self.assertTrue(
            any(p.grad is not None and p.grad.abs().max() > 0 for p in model.parameters())
        )
        self.assertTrue(
            all(torch.isfinite(p.grad).all() for p in model.parameters() if p.grad is not None)
        )
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "own.pt"
            torch.save(
                dict(
                    architecture="soundroom-mask-unet-v0",
                    classes=separator.CLASSES,
                    state=model.state_dict(),
                ),
                path,
            )
            loaded = separator.load_model(path)
            torch.testing.assert_close(loaded(target.sum(1)), prediction)
            torch.save(dict(architecture="other", classes=separator.CLASSES, state={}), path)
            with self.assertRaises(ValueError):
                separator.load_model(path)

    def test_complex_six_source_model_is_additive_and_has_finite_gradients(self):
        model = separator.Separator(classes=separator.ALL_CLASSES, complex_masks=True)
        target = separator.spectrum(torch.randn(1, 6, 2, 4096) * 0.02)
        mixture = target.sum(1)
        prediction = model(mixture)
        self.assertEqual(tuple(prediction.shape), tuple(target.shape))
        torch.testing.assert_close(prediction.sum(1), mixture, atol=1e-6, rtol=1e-5)
        (prediction - target).abs().mean().backward()
        self.assertTrue(
            all(torch.isfinite(p.grad).all() for p in model.parameters() if p.grad is not None)
        )
        actual = separator.predict(model.eval(), torch.randn(2, 5001) * 0.03, chunk_frames=4096)
        self.assertEqual(tuple(actual.shape), (6, 2, 5001))
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "complex.pt"
            torch.save(
                dict(
                    architecture="soundroom-complex-unet-v1",
                    classes=separator.ALL_CLASSES,
                    state=model.state_dict(),
                ),
                path,
            )
            loaded = separator.load_model(path)
            self.assertTrue(loaded.complex_masks)
            torch.testing.assert_close(loaded(mixture), prediction)

    def test_cli_rejects_vocals_for_instrument_only_checkpoint(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            source = root / "input.wav"
            output = root / "vocals.wav"
            checkpoint = root / "own.pt"
            sf.write(source, np.zeros((1024, 2)), 48000, subtype="FLOAT")
            torch.save(
                dict(
                    architecture="soundroom-mask-unet-v0",
                    classes=separator.CLASSES,
                    state=separator.Separator().state_dict(),
                ),
                checkpoint,
            )
            result = subprocess.run(
                [
                    sys.executable,
                    "-B",
                    str(Path(__file__).with_name("soundroom-separator.py")),
                    "separate",
                    str(source),
                    str(output),
                    "--checkpoint",
                    str(checkpoint),
                    "--source",
                    "vocals",
                ],
                capture_output=True,
                text=True,
                timeout=30,
            )
            self.assertEqual(result.returncode, 2)
            self.assertIn("not trained", result.stderr)
            self.assertFalse(output.exists())

    def test_complex_supervision_penalizes_attenuation_and_phase_errors(self):
        target = torch.complex(torch.ones(1, 6, 2, 3, 4), torch.ones(1, 6, 2, 3, 4)) * 0.2
        self.assertEqual(float(separator.complex_loss(target, target)), 0)
        quiet = (target * 0.1).detach().requires_grad_(True)
        loss = separator.complex_loss(quiet, target)
        self.assertGreater(float(loss), 0.1)
        self.assertGreater(float(separator.complex_loss(target * 1j, target)), 0.5)
        loss.backward()
        self.assertTrue(torch.isfinite(quiet.grad).all())

    def test_energy_supervision_rejects_one_stem_collapse(self):
        torch.manual_seed(53)
        target = torch.complex(torch.randn(2, 6, 2, 24, 16), torch.randn(2, 6, 2, 24, 16)) * 0.03
        mixture = target.sum(1)
        uniform = mixture[:, None].expand_as(target) / 6
        collapsed = torch.zeros_like(target)
        collapsed[:, 0] = mixture
        self.assertGreater(
            float(separator.complex_loss(collapsed, target)),
            float(separator.complex_loss(uniform, target)) * 1.5,
        )

    def test_split_leakage_and_source_tampering_are_rejected(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            scene = root / "train/0000"
            scene.mkdir(parents=True)
            metadata = dict(frames=1024, sha256={})
            for family in separator.CLASSES:
                path = scene / f"{family}.flac"
                sf.write(path, np.zeros((1024, 2)), 48000, subtype="PCM_24")
                metadata["sha256"][path.name] = separator.digest(path)
            (scene / "scene.json").write_text(json.dumps(metadata))
            record = dict(
                path="train/0000",
                split="train",
                scoreID="score1",
                metadataSHA256=separator.digest(scene / "scene.json"),
            )
            manifest = dict(classes=separator.CLASSES, sampleRate=48000, scenes=[record])
            (root / "corpus.json").write_text(json.dumps(manifest))
            separator.read_corpus(root)
            self.assertEqual(tuple(separator.load_scene(root, record).shape), (4, 2, 1024))
            (scene / "bass.flac").write_bytes(b"tamper")
            with self.assertRaises(ValueError):
                separator.load_scene(root, record)
            manifest["scenes"].append(dict(path="test/0000", split="test", scoreID="score1"))
            (root / "corpus.json").write_text(json.dumps(manifest))
            with self.assertRaises(ValueError):
                separator.read_corpus(root)

    def test_singer_leakage_is_rejected_even_for_independent_scores(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            manifest = dict(
                classes=separator.ALL_CLASSES,
                sampleRate=48000,
                scenes=[
                    dict(
                        path="train/a",
                        split="train",
                        scoreID="a",
                        sourceGroups=["vocalset:female1"],
                    ),
                    dict(
                        path="test/b", split="test", scoreID="b", sourceGroups=["vocalset:female1"]
                    ),
                ],
            )
            (root / "corpus.json").write_text(json.dumps(manifest))
            with self.assertRaisesRegex(ValueError, "Singer/composition"):
                separator.read_corpus(root)

    def test_slakh_choir_is_other_and_source_reading_is_aligned(self):
        self.assertEqual(recordings.family(dict(program_num=52, is_drum=False)), "other")
        self.assertEqual(recordings.family(dict(program_num=33, is_drum=False)), "bass")
        self.assertEqual(recordings.family(dict(program_num=0, is_drum=True)), "drums")
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "source.wav"
            sf.write(
                path,
                np.sin(np.arange(16000) * 0.1).astype(np.float32) * 0.1,
                16000,
                subtype="FLOAT",
            )
            audio, start = recordings.read_audio(
                path, 48000, __import__("random").Random(1), offset=0
            )
            self.assertEqual(audio.shape, (48000, 2))
            self.assertEqual(start, 0)
            np.testing.assert_array_equal(audio[:, 0], audio[:, 1])
            self.assertTrue(np.isfinite(audio).all())

    def test_splits_join_same_song_and_identical_midi_before_variants(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            records = [
                dict(dataset="vocalset", group=f"singer{i}", path=f"voice/{i}.wav")
                for i in range(10)
            ]
            for i, (song, midi) in enumerate(
                [("A", "m1"), ("A", "m2"), ("B", "m3"), ("C", "m3"), ("D", "m4")]
            ):
                path = root / f"Track{i}/metadata.yaml"
                path.parent.mkdir()
                path.write_text(f"lmd_midi_dir: matched/{song}/file.mid\nstems: {{}}\n")
                records += [
                    dict(dataset="babyslakh", group=f"Track{i}", path=str(path.relative_to(root))),
                    dict(
                        dataset="babyslakh",
                        group=f"Track{i}",
                        path=f"Track{i}/all_src.mid",
                        sha256=midi,
                    ),
                ]
            _, singers, songs = recordings.assign_groups(root, dict(files=records), 5)
            self.assertEqual(songs["Track0"]["split"], songs["Track1"]["split"])
            self.assertEqual(songs["Track2"]["split"], songs["Track3"]["split"])
            self.assertEqual(len({s["identity"] for s in songs.values()}), 3)
            self.assertFalse(set(singers["train"]) & set(singers["test"]))
            self.assertFalse(set(singers["validation"]) & set(singers["test"]))

    def test_recorded_cache_rejects_changed_generation_provenance(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            scene = root / "train/a"
            scene.mkdir(parents=True)
            (scene / "scene.json").write_text(json.dumps(dict(seed=5, generationFingerprint="old")))
            with self.assertRaisesRegex(ValueError, "different generation provenance"):
                recordings.render_scene(
                    root, "train/a", "train", None, [], [], None, root, None, None, None, 5, "new"
                )

    def test_native_cache_rejects_changed_generation_provenance(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            scene = root / "train/0000"
            scene.mkdir(parents=True)
            (scene / "scene.json").write_text(
                json.dumps(
                    dict(
                        seed=5, plan=generator.make_plan("train", 0, 5), generationFingerprint="old"
                    )
                )
            )
            with self.assertRaisesRegex(RuntimeError, "different generation provenance"):
                generator.render_scene(root, "train", 0, 5, None, None, "new")

    def test_metrics_measure_attenuation_and_absent_source_leakage(self):
        y = torch.zeros(4, 2, 1000)
        y[0] = torch.randn(2, 1000) * 0.1
        rows = separator.metrics(y * 0.1, y, y.sum(0))
        self.assertAlmostEqual(rows[0]["referenceGain"], 0.1, places=5)
        self.assertLess(rows[0]["sdr"], 1)
        self.assertTrue(rows[1]["absent"])
        self.assertEqual(rows[1]["leakageDBFS"], -120)

    def test_score_generation_is_reproducible_and_split_specific(self):
        plans = [generator.make_plan(split, 0, 5) for split in ["train", "validation", "test"]]
        self.assertEqual(plans[0], generator.make_plan("train", 0, 5))
        ids = [{asset["id"] for asset in plan["assets"]} for plan in plans]
        self.assertFalse(ids[0] & ids[1] or ids[0] & ids[2] or ids[1] & ids[2])
        for plan in plans:
            self.assertEqual(plan["session"]["masterTrack"]["realtimeEffects"], [])
            self.assertEqual(len(plan["assets"]), 4)


if __name__ == "__main__":
    unittest.main()
