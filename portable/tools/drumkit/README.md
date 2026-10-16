# Recorded drum kit tooling

Rebuilds the recorded kit (DRSKit 2.1, CC BY 4.0; see `THIRD_PARTY_NOTICES.md`)
without downloading the 2.8 GB archive: entries are fetched one instrument at a
time with HTTP range requests, converted, and the raw audio deleted.

```sh
W=build/Kits; mkdir -p $W/work/raw $W/tools && cp portable/tools/drumkit/*.py $W/tools/
python3 -I $W/tools/zipindex.py https://drumgizmo.org/kits/DRSKit/DRSKit2_1.zip $W/work/drskit-index.json
cargo build --release --manifest-path portable/Cargo.toml -p soundroom-engine --example build_drum_kit
OUT="$HOME/Library/Application Support/Soundroom/RustServer/Kits/drskit"
python3 -I $W/tools/run.py "$OUT" portable/target/release/examples/build_drum_kit \
  Snare:snare:center:1.6:--invert-snare-bottom Kdrum_without_contact:kick:center:1.4:--invert-snare-bottom \
  Snare_rim:snare:sideStick:0.8:--invert-snare-bottom Snare_whisker:snare:brush:1.2:--invert-snare-bottom \
  Hihat_closed:closedHat:center:0.8:--invert-snare-bottom Hihat_open:openHat:center:2.2:--invert-snare-bottom \
  Tom1:highTom:center:2.0:--invert-snare-bottom Tom3:lowTom:center:2.4:--invert-snare-bottom \
  Ride_tip:ride:center:3.5:--invert-snare-bottom Ride_tip_bell:ride:bell:3.5:--invert-snare-bottom \
  Crash_left_shank:crash:center:4.0:--invert-snare-bottom
```

Result: 229 hits, ~300 MB. The server looks for `Kits/drskit` in its storage
root, then in the app bundle's `Resources/Kits`.

Measured model (embedded in the engine) and its validation:

```sh
cargo run --release -p soundroom-engine --example analyze_drum_kit -- "$OUT" portable/crates/engine/assets/measured-drums.json
cargo run --release -p soundroom-engine --example evaluate_drum_models -- "$OUT"
cargo run --release -p soundroom-engine --example compare_drum_levels -- "$(dirname "$OUT")"
cargo run --release -p soundroom-engine --example render_drum_demo -- "$(dirname "$OUT")" build/Kits/demo
```
