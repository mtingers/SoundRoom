# Soundroom

Soundroom is a multitrack audio workstation with a native macOS client and a Rust audio server. Record and edit mono or stereo audio, arrange drum and instrument patterns directly on the timeline, automate a mix, and export audio or stems.

The macOS client and Rust server are the primary applications. A terminal controller and an optional browser client are also included. Other platform backends are experimental.

## Build and run on macOS

Requires macOS 14 or later, recent Xcode Command Line Tools (or Xcode), and Rust/Cargo. The clean source build was checked with Apple Swift 6.4 and Rust 1.94 on Apple silicon. Other toolchain/platform combinations need their own validation.

From the repository root:

```sh
zsh scripts/build-app.sh
open build/Soundroom.app
```

The build script compiles the native GUI, terminal controller, and Rust server, creates the app icon, and signs the local app ad hoc. Cargo downloads the dependencies recorded in `portable/Cargo.lock` on the first build. Python, model weights, recorded sample libraries, and Node.js are not required for this build.

Run the app bundle when recording so macOS can present microphone permission correctly. The GUI starts the local Rust server automatically. The macOS GUI has no standalone audio engine or Swift server fallback; the bundled Rust server handles all audio. Quitting the GUI leaves the audio server running; use the app's server connection controls to stop it explicitly.

See [BUILDING.md](BUILDING.md) for optional features, tests, and troubleshooting.

## What is included

- Multitrack recording, stereo waveforms, clip editing, fades, looping, project save/recovery, and Undo.
- Inline drum and instrument pattern editors, session tempo, sound design, and MIDI workflows.
- Track and master effects, automation, EQ, dynamics, reverb/delay, vocal enhancement, Sound Putty, and a guitar pedal/amp builder.
- A Rust engine/server, native SwiftUI/AppKit GUI, terminal clients, browser client source, shared protocol, and regression tests with synthetic audio fixtures.

## Demo projects

The repository includes two self-contained, editable demos:

- [Twinkle Star · darkwave](demos/Twinkle-Star/README.md) — synthesizers, drums, and piano at 100 BPM.
- [Nutshell · instrumental](demos/Nutshell/README.md) — the steel-string acoustic version at 68 BPM.

Open either `.soundroom` project through File → Open Session. Its `Audio` directory is included, so playback does not require installing the recorded instrument libraries or Python models. Each demo also has an AAC listening preview. See [DEMO.md](DEMO.md) for opening, editing, and credits.

## Optional source separation

**Effects → Separate source…** uses a separate Python/Demucs runtime launched by the Rust server. It is an optional installation, not a prerequisite for ordinary editing, recording, effects, or building the app.

Install Python **3.11**, then run this once from the repository root on the audio server's Mac:

```sh
bash scripts/setup-source-separation.sh --models all
```

The script creates `build/SeparationRuntime` with its own Python environment and model files. The default app in `build/` discovers it automatically. Installation requires network access; subsequent separation runs offline. No Python environment, pretrained weights, or training datasets are included in this repository.

Demucs code and pretrained weights have different licensing considerations. Read [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) before installing or using the models. Live spectral cleanup and the vocal enhancer do not require that runtime.

## Documentation

- [Build, optional components, and checks](BUILDING.md)
- [Server and client design](SERVER_CLIENT_DESIGN.md)
- [Terminal controller](TUI_USAGE.md) and [browser access](WEB_ACCESS.md)
- [Instruments](INSTRUMENTS.md), [drums](DRUMS.md), and [the two demo projects](DEMO.md)
- [Effects](EFFECT_WORKSPACES.md), [mixing](MASTERING_AND_LIVE_EFFECTS.md), and [pitch editor](PITCH_CURVE_EDITOR.md)
- [Developer documentation](docs/README.md)

## License

Soundroom's own source code is available under the [MIT License](LICENSE). Demo music is separate from the code license; see the [demo credits](DEMO.md). Third-party audio assets, downloaded sample libraries, model weights, and research datasets retain their respective terms; see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). The optional research separator is experimental and is not the production source-separation backend.

## Code style and checks

Run `make setup-style` once for Python development tooling. Use `make format` for Swift, Rust, and Python, `make lint` for style, Clippy, and shell syntax checks, and `make check` to include protocol validation. `make lint-scripts` checks the Python and shell tools on their own. The optional staged commit hook and editor settings are documented in [CONTRIBUTING.md](CONTRIBUTING.md).
