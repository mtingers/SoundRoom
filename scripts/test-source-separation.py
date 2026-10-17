#!/usr/bin/env python3
"""Bridge integrity/streaming tests; real model quality requires listening tests."""

import contextlib
import hashlib
import importlib.util
import io
import json
import math
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

spec = importlib.util.spec_from_file_location(
    "source_separation", Path(__file__).with_name("source-separation-worker.py")
)
worker = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = worker
spec.loader.exec_module(worker)


class DownloadResponse(io.BytesIO):
    def __init__(self, data, secure=True, length=None):
        super().__init__(data)
        self.headers = {"Content-Length": str(len(data) if length is None else length)}
        self.secure = secure

    def geturl(self):
        return (
            "https://dl.fbaipublicfiles.com/provider"
            if self.secure
            else "http://untrusted.invalid/"
        )


class SourceSeparationTests(unittest.TestCase):
    def test_target_specific_quality_model_matches_provider_one_hot_bag(self):
        self.assertEqual(worker.signature_for("quality", "vocals"), "04573f0d")
        self.assertEqual(worker.signature_for("quality", "drums"), "f7e0c4bc")
        self.assertEqual(worker.signature_for("quality", "bass"), "d12395a8")
        self.assertEqual(worker.signature_for("quality", "other"), "92cfc3b6")
        with self.assertRaises(worker.WorkerError):
            worker.signature_for("quality", "piano")
        self.assertEqual(worker.signature_for("extended", "piano"), "5c90dfd2")

    def test_status_never_imports_heavy_models_or_attempts_network(self):
        before = set(sys.modules)
        with (
            tempfile.TemporaryDirectory() as root,
            mock.patch.object(
                worker.urllib.request, "urlopen", side_effect=AssertionError("network")
            ),
        ):
            status = worker.runtime_status(Path(root))
        self.assertTrue(status["offline"])
        self.assertTrue(status["models"]["extended"]["experimental"])
        self.assertEqual(status["codeLicense"], "MIT")
        self.assertEqual(status["weightsLicense"], "unresolved")
        self.assertNotEqual(status["license"], "MIT")
        self.assertEqual(
            status["weightsLicenseSource"], "https://github.com/facebookresearch/demucs/issues/327"
        )
        self.assertNotIn("torch", set(sys.modules) - before)
        self.assertNotIn("demucs", set(sys.modules) - before)

    def test_download_hash_recording_cached_check_and_tamper_detection(self):
        data = b"small stand-in provider payload: provenance test only"
        digest = hashlib.sha256(data).hexdigest()
        name = f"955717e8-{digest[:8]}.th"
        with (
            tempfile.TemporaryDirectory() as root,
            mock.patch.dict(worker.MODEL_FILES, {"955717e8": name}),
        ):
            folder = Path(root)
            manifest = worker.load_manifest(folder)
            requests = []

            def opener(request, timeout):
                requests.append(request.full_url)
                self.assertEqual(timeout, 45)
                return DownloadResponse(data)

            with contextlib.redirect_stdout(io.StringIO()):
                worker.download_model(folder, "955717e8", manifest, opener)
                self.assertEqual(manifest["files"][name]["license"], "unresolved")
                # Existing verified weights may carry our previous incorrect
                # MIT label. Explicit setup must repair it without a download.
                manifest["files"][name]["license"] = "MIT"
                worker.atomic_text(folder / "manifest.json", json.dumps(manifest))
                worker.download_model(folder, "955717e8", manifest, opener)
            self.assertEqual(requests, [worker.MODEL_ROOT + name])
            self.assertEqual(manifest["files"][name]["sha256"], digest)
            self.assertEqual(manifest["files"][name]["license"], "unresolved")
            self.assertEqual(
                manifest["files"][name]["licenseSource"], worker.WEIGHTS_LICENSE_SOURCE
            )
            self.assertEqual((folder / name).read_bytes(), data)
            self.assertTrue(worker.verified_model(folder, "955717e8", manifest, full=True))
            self.assertEqual(json.loads((folder / "manifest.json").read_text()), manifest)
            stamp = (folder / name).stat()
            (folder / name).write_bytes(bytes([data[0] ^ 1]) + data[1:])
            import os

            os.utime(folder / name, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
            self.assertFalse(worker.verified_model(folder, "955717e8", manifest, full=True))
            self.assertEqual(list(folder.glob(".*.download")), [])

    def test_setup_separates_code_license_from_weight_terms(self):
        with (
            tempfile.TemporaryDirectory() as root,
            mock.patch.object(worker, "download_model"),
            mock.patch.object(worker, "runtime_status", return_value={}),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            folder = Path(root)
            worker.prepare_models(folder, "standard")
            self.assertTrue((folder / "DEMUCS-LICENSE.txt").read_text().startswith("MIT License"))
            notice = (folder / "DEMUCS-WEIGHTS-NOTICE.txt").read_text()
            self.assertIn("scientific purposes", notice)
            self.assertIn("https://github.com/facebookresearch/demucs/issues/327", notice)
            self.assertIn("not the weights", notice)

    def test_invalid_checksum_partial_download_and_http_redirect_publish_nothing(self):
        data = b"invalid model bytes"
        for response in (
            DownloadResponse(data),
            DownloadResponse(data, length=len(data) + 1),
            DownloadResponse(data, secure=False),
        ):
            with tempfile.TemporaryDirectory() as root:
                folder = Path(root)
                with self.assertRaises(worker.WorkerError):
                    worker.download_model(
                        folder,
                        "955717e8",
                        worker.load_manifest(folder),
                        lambda *_args, **_kwargs: response,
                    )
                self.assertEqual(list(folder.iterdir()), [])

    def test_overlap_math_preserves_exact_length_relative_gain_and_remove_identity(self):
        # Independent scalar reference for the worker's bounded streaming OLA.
        # A known linear predictor is not evidence about learned-model quality.
        for frames in (0, 1, 2, 31, 32, 33, 71, 96, 105):
            original = [math.sin(i * 0.13) * 1.7 for i in range(frames)]
            for segment in (8, 31, 32):
                pending = [0.0] * segment
                sum_weight = [0.0] * segment
                weights = worker.triangle_weights(segment)
                extracted, removed = [], []
                for start, n, writable in worker.overlap_layout(frames, segment):
                    for i in range(n):
                        pending[i] += original[start + i] * 0.4 * weights[i]
                        sum_weight[i] += weights[i]
                    for i in range(writable):
                        stem = pending[i] / sum_weight[i]
                        extracted.append(stem)
                        removed.append(original[start + i] - stem)
                    pending = pending[writable:] + [0.0] * writable
                    sum_weight = sum_weight[writable:] + [0.0] * writable
                self.assertEqual(len(extracted), frames)
                for i in range(frames):
                    self.assertAlmostEqual(extracted[i], original[i] * 0.4)
                    self.assertAlmostEqual(extracted[i] + removed[i], original[i])

    def test_malformed_and_foreign_manifests_are_not_trusted(self):
        with tempfile.TemporaryDirectory() as root:
            folder = Path(root)
            for content in (
                "{broken",
                "null",
                "[]",
                '{"schema":1,"provider":"foreign","files":{}}',
            ):
                (folder / "manifest.json").write_text(content)
                self.assertEqual(worker.load_manifest(folder)["files"], {})

    def test_ready_report_publishes_only_after_successful_preparation(self):
        status = {
            "runtimeReady": True,
            "sampleRate": 48000,
            "models": {
                "quality": {"available": True, "targets": ["vocals", "drums", "bass", "other"]},
            },
        }
        with tempfile.TemporaryDirectory() as root:
            ready = Path(root) / "ready.json"
            arguments = ["--prepare-models", "--models-dir", root, "--ready-file", str(ready)]
            with (
                mock.patch.object(
                    worker, "prepare_models", side_effect=worker.WorkerError("checksum failure")
                ),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                self.assertEqual(worker.main(arguments), 1)
            self.assertFalse(ready.exists())
            with (
                mock.patch.object(worker, "prepare_models"),
                mock.patch.object(worker, "runtime_status", return_value=status),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                self.assertEqual(worker.main(arguments), 0)
            actual = json.loads(ready.read_text())
            self.assertEqual(actual["schema"], 1)
            self.assertEqual(actual["sampleRate"], 48000)
            self.assertEqual(actual["models"], status["models"])
            self.assertEqual(list(Path(root).glob(".*.tmp")), [])

    def test_scratch_filename_is_known_to_the_parent_cancellation_guard(self):
        output = Path("/private/tmp/Artifacts/1234.wav")
        self.assertEqual(worker.scratch_path(output), output.parent / ".1234.wav.separation.tmp")

    @unittest.skipUnless(
        all(
            importlib.util.find_spec(name) for name in ("torch", "torchaudio", "numpy", "soundfile")
        ),
        "Pinned separation runtime is not installed",
    )
    def test_actual_wav_stream_with_a_known_linear_model(self):
        import argparse

        import numpy as np
        import soundfile as sf

        class LinearModel:
            audio_channels, samplerate, sources = 2, 44100, ["vocals"]

            def cpu(self):
                return self

            def eval(self):
                return self

        with tempfile.TemporaryDirectory() as root:
            folder = Path(root)
            input_path, extract_path, remove_path = (
                folder / x for x in ("input.wav", "extract.wav", "remove.wav")
            )
            frames = round(48000 * 8.1) + 7
            time = np.arange(frames) / 48000
            source = np.stack(
                [0.3 * np.sin(2 * np.pi * 750 * time), 0.2 * np.sin(2 * np.pi * 2250 * time)],
                axis=1,
            ).astype("float32")
            sf.write(input_path, source, 48000, subtype="FLOAT")
            args = argparse.Namespace(
                input=input_path,
                output=extract_path,
                models_dir=folder,
                model="quality",
                target="vocals",
                operation="extract",
            )
            with (
                mock.patch.object(worker, "runtime_status", return_value={"runtimeReady": True}),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                worker.render(args, LinearModel, lambda _model, audio: audio * 0.4)
                args.output, args.operation = remove_path, "remove"
                worker.render(args, LinearModel, lambda _model, audio: audio * 0.4)
            extracted, rate = sf.read(extract_path, dtype="float32")
            removed, _ = sf.read(remove_path, dtype="float32")
            self.assertEqual(rate, 48000)
            self.assertEqual(extracted.shape, source.shape)
            self.assertLess(
                float(np.max(np.abs(extracted[1000:-1000] - source[1000:-1000] * 0.4))), 0.001
            )
            self.assertLess(float(np.max(np.abs(extracted + removed - source))), 1e-6)


if __name__ == "__main__":
    unittest.main()
