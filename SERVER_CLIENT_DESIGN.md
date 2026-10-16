# Soundroom server and clients

Soundroom has one audio host: the Rust `soundroom-server`. The macOS GUI is a controller. Playback, recording, instruments, effects, project storage and source separation run on the server. The GUI keeps selections, editor drafts, scroll positions and display caches locally.

## Launching

`scripts/build-app.sh` packages the macOS GUI, terminal client and `Contents/Helpers/soundroom-server`. Launching the app starts or reconnects to that Rust helper over authenticated WSS on loopback port 49160. Its private data, certificate and GUI credential live in `~/Library/Application Support/Soundroom/RustServer` by default. Closing the GUI leaves the server and established recording running; stopping the server is an explicit operator action.

Use `--rust-server PATH`, `--rust-data-directory DIRECTORY` and `--rust-port PORT` to select a different local Rust installation. A missing Rust executable produces an error. There is no in-process audio engine or Swift server fallback in the GUI.

To connect to an existing server without starting a local one:

```sh
Soundroom --connect-wss wss://studio.local:8443/control \
  --client-origin https://studio.local:8443 \
  --credential-file /path/to/approved-client.json \
  --client-name "Studio GUI"
```

Use the exact approved client name and a normally trusted server certificate. Local launches pin their own generated CA. The legacy native host/port client adapter remains available for compatible external services; it does not host audio. The retired `--standalone`, `--swift-server` and `--server` GUI options are rejected before startup.

## Authority and collaboration

The server owns the document, audio clock, hardware, Undo histories, recording recovery and background jobs. Clients send authenticated commands with resource revisions. Stale edits conflict instead of replacing a newer track. Continuous gestures coalesce, while private selections and navigation remain independent. Editors can edit; operators additionally control transport, devices and access; viewers inspect.

The GUI receives authoritative metadata, meters and visible waveform tiles rather than complete PCM assets. Its optimistic metadata edits are reconciled against server receipts. Private pitch, drum, instrument and mastering drafts retain source/version fences so late replies cannot overwrite a newer draft or redirect an edit after reconnection.

Imports and local project opening upload validated media/packages to the server. Save writes a server project and downloads a validated portable package back to the opened local file. Save As selects another destination. Closing the GUI disconnects its controller without stopping the audio host.

## Performance and validation

The Rust audio callback does not wait on clients or perform file transfer and JSON work. Worker queues prepare graphs, media, waveforms, analysis, rendering and exports. Running audio follows the server clock; network latency affects command arrival and live remote notes.

The macOS integration tests exercise the GUI against a Rust host using its software callback. Hardware and cross-computer tests require explicit opt-in. `SoundroomCore` and the Swift control libraries retain shared models, file-validation helpers and DSP references; they do not provide a standalone GUI or a packaged Swift audio server. See [Rust server documentation](portable/crates/server/README.md), [browser access](WEB_ACCESS.md) and [terminal usage](TUI_USAGE.md).
