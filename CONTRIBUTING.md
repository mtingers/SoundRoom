# Contributing

The Rust server and macOS GUI share one set of checked formatting rules. Run these commands from the repository root:

```sh
make format        # Apply formatting to authored Swift and Rust code
make format-check  # Check formatting without changing files
make lint          # Swift style checks, Rust formatting, and Clippy
make check         # Lint plus generated protocol/validator checks
make install-hooks # Optional: check staged formatting before each commit
```

Use Swift 6.4 / Xcode 27 and Rust 1.94 with `rustfmt` and `clippy` installed. CI uses the Xcode 27 runner and Rust 1.94.0. `rustup component add rustfmt clippy` installs the Rust tools for your selected local toolchain. Protocol checks also need Node.js, the pinned TypeScript 5.3.3 compiler, Python 3, and `rustc`; see [protocol documentation](protocol/README.md).

## Formatting

Swift uses the toolchain's [`swift format`](https://github.com/swiftlang/swift-format/blob/main/Documentation/Configuration.md) with four-space indentation, a 120-column target, sorted imports, one declaration/case per line, and no semicolon-packed statements. Lint also flags ambiguous trailing-closure overloads, assignment expressions, playground literals, non-ASCII identifiers, and incorrectly capitalized type names. The configuration preserves identifiers, access levels, explicit initializers, numeric literals, and multiline string contents. Keep meaningful grouping and explanatory comments around DSP equations and concurrency boundaries.

Rust uses `rustfmt`, four-space indentation, the 2021 style edition, and a 100-column target. Clippy checks the complete workspace, including tests; warnings fail the check. Debug macros, unfinished `todo!` calls, and `unimplemented!` placeholders are also reported. Explain narrowly scoped exceptions next to the code; avoid crate-wide lint suppression.

`.editorconfig` supplies matching editor defaults. The scripts select source files through Git and exclude ignored build/cache/data directories. Generated protocol bindings retain their generator-owned layout; edit the models/generator and run `python3 protocol/generate.py` instead of formatting generated files by hand. `make protocol-check` verifies those outputs have not drifted.

## Commit hook and CI

The optional hook checks the actual staged source blobs and staged formatting configuration. It does not rewrite files, stage changes, stash work, or include unstaged edits. After a formatting failure, run `make format`, inspect the result, and stage the changes you intend to commit. The installer preserves existing default hooks and refuses to replace a different configured hooks directory. To disable this repository's hook later, run `git config --local --unset core.hooksPath`.

GitHub Actions runs Swift formatting, Rust formatting/Clippy, protocol drift checks, and regression tests for the hook's handling of partially staged files. These are read-only checks. CI does not run audio hardware, microphone, or desktop automation. Use the existing software and hardware test suites when a behavioral change requires them.

Formatting improves readability; it does not replace review of callback allocations/locks, cancellation, revision fences, or DSP behavior. Prefer small functions with a clear responsibility when changing an existing area. Avoid unrelated structural rewrites in a bug fix.
