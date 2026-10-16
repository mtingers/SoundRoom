# Building Soundroom

## Native macOS application

Install Apple's Command Line Tools (`xcode-select --install`) or Xcode, and a current Rust toolchain with Cargo. The app targets macOS 14 and later. The source build is validated on Apple silicon with Swift 6.4 / Rust 1.94; Windows/Linux hardware backends are experimental.

```sh
zsh scripts/build-app.sh
open build/Soundroom.app
```

Build outputs live in `.build/`, `portable/target/`, and `build/`; none belongs in Git. No paid plugins or Apple Developer ID certificate are needed for a local source build. A distributable signed/notarized app is a separate release process described in [DISTRIBUTION.md](DISTRIBUTION.md).

The app bundles only the Rust audio server. Standalone GUI mode and the Swift server helper have been removed.

The script supports a custom destination as its first argument. Prefer an absolute path. Stop the server and quit the existing app before replacing the app you are actively using. The GUI and server must use matching source versions.

Project files and server credentials are created in the user's Application Support directory at runtime. Do not commit pairing credentials, recordings, projects, or private keys. Imported projects can contain substantial audio data.

## Python source separation (optional)

The Rust server invokes `scripts/source-separation-worker.py` through a private Python environment. The setup script pins Python 3.11 and its ML dependencies; the system's newest `python3` is not necessarily suitable.

```sh
python3.11 --version
bash scripts/setup-source-separation.sh --models all
```

If Python 3.11 is not on PATH, pass its executable explicitly:

```sh
bash scripts/setup-source-separation.sh --python /path/to/python3.11 --models all
```

Keep `build/SeparationRuntime` beside the default `build/Soundroom.app`. If moving the app to another location, install into the local Rust server's storage directory instead:

```sh
bash scripts/setup-source-separation.sh --models all \
  --runtime-dir "$HOME/Library/Application Support/Soundroom/RustServer/SeparationRuntime"
```

For a custom server root, use `<server-root>/SeparationRuntime`; alternatively set `SOUNDROOM_SEPARATION_RUNTIME` for the **server process**. A shell variable set only for a remote GUI does not configure the server. Python virtual environments are machine/location dependent: run the installer on the destination Mac instead of copying an installed `.venv`.

The default quality model separates vocals, drums, bass, and other material. The extended model adds guitar and experimental piano separation. The install downloads packages and weights; separation itself is local/offline. Missing installation disables learned separation with an explanation while other audio tools remain available. Preserve the [third-party notices](THIRD_PARTY_NOTICES.md), including the unresolved upstream pretrained-weight terms.

## Recorded instruments and drums (optional)

Modeled and electronic voices ship in source, including the measured instrument/drum models. The larger recorded sample libraries are installed separately in the Rust server's storage directory. This repository contains their conversion/download tooling, not your local library installation.

- [Recorded instruments](portable/tools/instruments/README.md)
- [Recorded drum kit](portable/tools/drumkit/README.md)

Use the download commands from those guides when you need the recorded variants. They require network access and the stated conversion tools. Their asset licenses are described in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Browser client (optional)

Install Node.js/npm, then build the browser assets before packaging the app:

```sh
cd web
npm install
npm run build
cd ..
zsh scripts/build-app.sh
```

The app build includes `web/dist` when it exists. A native-only build does not need Node.js. Browser access requires explicit server pairing/setup; see [WEB_ACCESS.md](WEB_ACCESS.md).

## Checks

Run the checks for the components you change. These software checks are distinct from audio-interface, microphone, and listening qualification.

```sh
# Swift tests; some integration cases require local sockets or hardware privileges.
swift test -c release --disable-sandbox --cache-path .build/cache -debug-info-format none

# Rust engine and server tests.
cargo test --release --locked --manifest-path portable/Cargo.toml \
  -p soundroom-engine -p soundroom-server

# Python bridge tests use the standard library and do not download models.
python3 scripts/test-source-separation.py

# Browser tests, after installing its dependencies.
cd web
npm test
cd ..

# Shared protocol checks require Python 3, Node.js, TypeScript, and rustc.
PATH="$PWD/web/node_modules/.bin:$PATH" bash protocol/check.sh
```

Synthetic WAV/AIFF/MIDI/project fixtures in `portable/fixtures` are intentional regression inputs. Keep them: Rust parity and media tests depend on them. Test programs that explicitly open audio hardware should only be run when you intend to use that hardware.

## Troubleshooting

- If Cargo cannot download crates, check network access. `--offline` works only after all locked dependencies are cached.
- If `swift` uses an unexpected SDK/toolchain, check `xcode-select -p` and `swift --version`.
- If Separate source reports no models, verify Python 3.11 was used, let setup finish successfully, and install on the server machine at one of the paths above.
- If recorded instruments are unavailable, install their optional library rather than copying another user's Application Support directory.
- Hardware backends and permissions vary across Macs. A successful software build does not qualify every interface, clock arrangement, or real-time processing load.
