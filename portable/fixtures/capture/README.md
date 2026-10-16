# Simulated capture recovery fixtures

These raw archives use deterministic native PCM values and explicit simulated sample/host timestamps. They are file fixtures, not microphone recordings or physical clock qualification.

The three cases cover a continuous capture, a missing native packet, and four loop passes with a punch range. Each includes raw native packets, the sequenced output clock journal, capture completion metadata, recovery settings and expected take/clip/asset identities and SHA-256 fingerprints. Tests regenerate streamed WAV artifacts, validate their fingerprints and compare recovery receipts exactly. Separate analytic-signal tests qualify resampling quality; a drifting-clock worker/recovery test compares every recovered sample against the live worker.

Generate into a **new** directory for review from the repository root:

```sh
cargo run --manifest-path portable/Cargo.toml --locked --release \
  -p soundroom-engine --example generate_capture_recovery -- /tmp/new-capture-fixtures
```

The reference hashes were generated on macOS arm64. Windows recovery remains gated on private storage integration, and cross-platform floating-point/durability qualification remains open.
