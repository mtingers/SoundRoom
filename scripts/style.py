#!/usr/bin/env python3
"""Format/check authored Swift and Rust sources without touching generated files."""

import argparse
import difflib
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
GENERATED_SWIFT = frozenset(
    {
        "Sources/SoundroomRemote/RemoteV3Descriptor.swift",
        "Sources/SoundroomRemote/RemoteV3Models.swift",
        "Sources/SoundroomRemote/RemoteAutomationWriteRanges.swift",
        "Tests/SoundroomRemoteTests/RemoteV3EnumParityTests.swift",
    }
)


def git(*arguments):
    return subprocess.check_output(["git", *arguments], cwd=ROOT)


def source_language(name):
    if name in GENERATED_SWIFT:
        return None
    if name.endswith(".swift"):
        return "swift"
    if name.startswith("portable/crates/") and name.endswith(".rs"):
        return "rust"
    return None


def files(staged=False):
    arguments = (
        ["diff", "--cached", "--name-only", "--diff-filter=ACMR", "-z"]
        if staged
        else ["ls-files", "--cached", "--others", "--exclude-standard", "-z"]
    )
    names = {os.fsdecode(name) for name in git(*arguments).split(b"\0") if name}
    return sorted(
        name
        for name in names
        if source_language(name) and (staged or (ROOT / name).is_file())
    )


def swift_formatter():
    # Use the selected Swift toolchain rather than a second formatter dependency.
    swift = shutil.which("swift")
    if swift:
        result = subprocess.run(
            [swift, "format", "--help"], capture_output=True, cwd=ROOT
        )
        if result.returncode == 0:
            return [swift, "format"]
    formatter = shutil.which("swift-format")
    if formatter:
        return [formatter]
    raise RuntimeError("Swift formatting requires Swift 6.4 / Xcode 27 or newer.")


def run(arguments, **options):
    subprocess.run(arguments, cwd=ROOT, check=True, **options)


def working_tree(command, language):
    if language in ("all", "swift"):
        sources = [name for name in files() if source_language(name) == "swift"]
        if sources:
            operation = ["format", "--in-place"] if command == "format" else ["lint", "--strict"]
            run(
                swift_formatter()
                + operation
                + ["--parallel", "--configuration", str(ROOT / ".swift-format")]
                + sources
            )
            action = "formatted" if command == "format" else "checked"
            print(f"Swift: {len(sources)} authored files {action}.", flush=True)
    if language in ("all", "rust"):
        arguments = ["cargo", "fmt", "--manifest-path", "portable/Cargo.toml", "--all"]
        if command == "check":
            arguments += ["--", "--check"]
        run(arguments)
        action = "formatted" if command == "format" else "checked"
        print(f"Rust: workspace {action}.", flush=True)


def index_configuration(name):
    result = subprocess.run(
        ["git", "show", ":" + name], cwd=ROOT, capture_output=True
    )
    return result.stdout if result.returncode == 0 else (ROOT / name).read_bytes()


def staged():
    """Check index blobs, preserving unstaged changes and partially staged files."""
    names = files(staged=True)
    if not names:
        return
    has_swift = any(source_language(name) == "swift" for name in names)
    swift = swift_formatter() if has_swift else None
    failures = []
    with tempfile.TemporaryDirectory(prefix="soundroom-staged-style-") as folder:
        configuration = Path(folder)
        (configuration / ".swift-format").write_bytes(index_configuration(".swift-format"))
        (configuration / "rustfmt.toml").write_bytes(index_configuration("portable/rustfmt.toml"))
        for name in names:
            original = git("show", ":" + name)
            if source_language(name) == "swift":
                arguments = swift + [
                    "format", "--configuration", str(configuration / ".swift-format"),
                    "--assume-filename", name,
                ]
            else:
                # stdin avoids traversing modules outside the selected index blob.
                # rustfmt --check on stdin can exit zero despite a diff, so compare bytes.
                arguments = [
                    "rustfmt", "--emit", "stdout",
                    "--config-path", str(configuration / "rustfmt.toml"),
                ]
            result = subprocess.run(arguments, input=original, cwd=ROOT, capture_output=True)
            if result.returncode:
                sys.stderr.buffer.write(result.stderr)
                failures.append(name)
            elif original != result.stdout:
                failures.append(name)
                diff = difflib.unified_diff(
                    original.decode().splitlines(keepends=True),
                    result.stdout.decode().splitlines(keepends=True),
                    fromfile="staged/" + name,
                    tofile="formatted/" + name,
                )
                sys.stderr.writelines(diff)
    if failures:
        raise RuntimeError(
            "Staged formatting failed. Run make format, review the diff, "
            "then stage the intended changes again."
        )
    print(f"Staged formatting: {len(names)} files passed.")


def install_hook():
    existing = subprocess.run(
        ["git", "config", "--get", "core.hooksPath"], cwd=ROOT, capture_output=True, text=True
    ).stdout.strip()
    if existing and existing != ".githooks":
        raise RuntimeError(
            f"core.hooksPath already uses {existing!r}; "
            "add the style check to that hook instead."
        )
    if not existing:
        active = Path(os.fsdecode(git("rev-parse", "--git-path", "hooks/pre-commit")).strip())
        active = active if active.is_absolute() else ROOT / active
        if active.exists():
            raise RuntimeError(
                "A pre-commit hook already exists; add the staged style check "
                "to that hook instead."
            )
    hook = ROOT / ".githooks/pre-commit"
    hook.chmod(hook.stat().st_mode | 0o111)
    run(["git", "config", "--local", "core.hooksPath", ".githooks"])
    print("Enabled the read-only staged formatting hook for this checkout.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["format", "check", "staged", "install-hook"])
    parser.add_argument("--language", choices=["all", "swift", "rust"], default="all")
    arguments = parser.parse_args()
    try:
        if arguments.command == "staged":
            staged()
        elif arguments.command == "install-hook":
            install_hook()
        else:
            working_tree(arguments.command, arguments.language)
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"Style check failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
