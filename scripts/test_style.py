#!/usr/bin/env python3
"""Regression checks for generated-file exclusions and partially staged commits."""

import contextlib
import io
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import style


class StagedStyleTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="soundroom-style-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        subprocess.run(["git", "init", "--quiet", str(self.root)], check=True)
        for name in [".swift-format", "portable/rustfmt.toml", "ruff.toml"]:
            target = self.root / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes((style.ROOT / name).read_bytes())
        # Keep using the real checkout's installed tool while ROOT points at a test index.
        self.ruff_patch = patch.object(style, "ruff", return_value=style.ruff())
        self.ruff_patch.start()
        self.addCleanup(self.ruff_patch.stop)
        self.root_patch = patch.object(style, "ROOT", self.root)
        self.root_patch.start()
        self.addCleanup(self.root_patch.stop)

    def add(self, name, contents):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(contents)
        subprocess.run(["git", "add", "--", name], cwd=self.root, check=True)
        return path

    def test_unformatted_rust_index_is_rejected_even_when_worktree_is_formatted(self):
        name = "portable/crates/example/src/lib.rs"
        staged = b"pub fn example( ) { }\n"
        path = self.add(name, staged)
        worktree = b"pub fn example() {}\n"
        path.write_bytes(worktree)
        with contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(RuntimeError):
                style.staged()
        self.assertEqual(path.read_bytes(), worktree)
        self.assertEqual(style.git("show", ":" + name), staged)

    def test_unformatted_python_index_is_rejected_even_when_worktree_is_formatted(self):
        name = "scripts/example.py"
        staged = b"values=[1,2];other=3\n"
        path = self.add(name, staged)
        worktree = b"values = [1, 2]\nother = 3\n"
        path.write_bytes(worktree)
        with contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(RuntimeError):
                style.staged()
        self.assertEqual(path.read_bytes(), worktree)
        self.assertEqual(style.git("show", ":" + name), staged)

    def test_python_lint_uses_index_and_staged_configuration(self):
        name = "scripts/example.py"
        staged = b"print(undefined_name)\n"
        path = self.add(name, staged)
        path.write_text('print("fixed only in the worktree")\n')
        self.add("ruff.toml", (self.root / "ruff.toml").read_bytes())
        (self.root / "ruff.toml").write_text('[lint]\nignore = ["F821"]\n')
        with contextlib.redirect_stderr(io.StringIO()) as errors:
            with self.assertRaises(RuntimeError):
                style.staged()
        self.assertIn("F821", errors.getvalue())
        self.assertEqual(style.git("show", ":" + name), staged)
        self.add(name, b'print("formatted in the index")\n')
        path.write_text("invalid worktree (\n")
        with contextlib.redirect_stdout(io.StringIO()):
            style.staged()
        self.assertEqual(path.read_text(), "invalid worktree (\n")

    def test_shell_syntax_uses_index_without_executing_script(self):
        marker = self.root / "must-not-exist"
        name = "scripts/example.sh"
        staged = f'#!/bin/bash\ntouch "{marker}"\nif true; then\n'.encode()
        path = self.add(name, staged)
        path.write_text("#!/bin/bash\nexit 0\n")
        with contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(RuntimeError):
                style.staged()
        self.assertFalse(marker.exists())
        self.assertEqual(style.git("show", ":" + name), staged)
        for shell in ("sh", "bash", "zsh"):
            with self.subTest(shell=shell):
                self.add(name, f'#!/bin/{shell}\ntouch "{marker}"\n'.encode())
                with contextlib.redirect_stdout(io.StringIO()):
                    style.staged()
                self.assertFalse(marker.exists())

    def test_zsh_specific_syntax_and_unsupported_shebang(self):
        style.shell_check("script.sh", b'#!/bin/zsh\nprint "${0:A:h}"\n')
        with self.assertRaises(RuntimeError):
            style.shell_check("script.sh", b"#!/bin/fish\necho nope\n")

    def test_formatted_swift_index_is_accepted_without_rewriting_unstaged_edits(self):
        name = "Sources/Example.swift"
        staged = b"struct Example {\n    let value: Int\n}\n"
        path = self.add(name, staged)
        worktree = b"struct Example{let value:Int;let other:Int}\n"
        path.write_bytes(worktree)
        with contextlib.redirect_stdout(io.StringIO()):
            style.staged()
        self.assertEqual(path.read_bytes(), worktree)
        self.assertEqual(style.git("show", ":" + name), staged)

    def test_generated_bindings_and_ignored_build_outputs_are_excluded(self):
        for name in style.GENERATED_SWIFT:
            self.add(name, b"generated binding\n")
        self.add("protocol/generated/models.rs", b"generated binding\n")
        self.add("Sources/Example.swift", b"struct Example {}\n")
        self.add(".gitignore", b"build/\n")
        output = self.root / "build/Example.swift"
        output.parent.mkdir()
        output.write_text("this is an ignored build output")
        self.assertEqual(style.files(), ["Sources/Example.swift"])
        self.assertEqual(style.files(staged=True), ["Sources/Example.swift"])

    def test_staged_configuration_wins_over_unstaged_configuration(self):
        self.add(".swift-format", (self.root / ".swift-format").read_bytes())
        (self.root / ".swift-format").write_text("invalid unstaged configuration")
        self.assertTrue(style.index_configuration(".swift-format").startswith(b"{"))

    def test_installer_preserves_an_existing_default_hook(self):
        hook = self.root / ".git/hooks/pre-commit"
        hook.write_text("#!/bin/sh\nexit 0\n")
        with self.assertRaises(RuntimeError):
            style.install_hook()
        self.assertEqual(hook.read_text(), "#!/bin/sh\nexit 0\n")
        result = subprocess.run(
            ["git", "config", "--get", "core.hooksPath"],
            cwd=self.root,
            capture_output=True,
        )
        self.assertEqual(result.returncode, 1)


if __name__ == "__main__":
    unittest.main()
