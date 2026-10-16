# Recorded instrument tooling

Builds the sound designer's recorded instruments into
`~/Library/Application Support/Soundroom/RustServer/Kits/instruments/<id>/`
(manifest + 16-bit 48 kHz WAVs), downloading only the mapped note samples.

```sh
cd portable && cargo build --release -p soundroom-engine --example build_instrument
OUT="$HOME/Library/Application Support/Soundroom/RustServer/Kits/instruments"
C=target/release/examples/build_instrument; W=../build/Instruments/work/download
# SFZ-mapped libraries
uv run --no-project tools/instruments/build_library.py pickedBass sfzinstruments/karoryfer.swagbass swagbass_clean.sfz "$OUT" $C --release 0.15 --scratch $W
uv run --no-project tools/instruments/build_library.py electricGuitar sfzinstruments/karoryfer.emilyguitar emily_clean.sfz "$OUT" $C --release 0.25 --scratch $W
uv run --no-project tools/instruments/build_library.py acousticGuitar sfzinstruments/karoryfer.shinyguitar Programs/acoustic.sfz "$OUT" $C --release 0.3 --robins 3 --sample-dir Samples --scratch $W
uv run --no-project tools/instruments/build_library.py uprightBass sfzinstruments/dsmolken.double-bass d_smolken_rubner_bass_pizz.sfz "$OUT" $C --release 0.4 --scratch $W
uv run --no-project tools/instruments/build_library.py soloCello sfzinstruments/karoryfer-bigcat.cello Programs/vc_arco_sus_map.sfz "$OUT" $C --loop --release 0.35 --scratch $W
# Name-mapped libraries (VCSL, VSCO 2 CE): save the GitHub trees first
curl -s "https://api.github.com/repos/sgossner/VCSL/git/trees/master?recursive=1" -o $W/vcsl-tree.json
curl -s "https://api.github.com/repos/sgossner/VSCO-2-CE/git/trees/master?recursive=1" -o $W/vsco-tree.json
for i in studioPiano uprightPiano concertHarp vibraphone marimba kalimba woodenFlute stringEnsemble; do
  SOUNDROOM_TREES=$W uv run --no-project tools/instruments/build_from_names.py $i "$OUT" $C $W tools/instruments/specs/$i.json
done
```

About 1.2 GB is downloaded in total (deleted as it is converted); ~690 MB is
stored. Octaves are corrected from each note's detected pitch.
