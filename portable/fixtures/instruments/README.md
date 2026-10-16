# Native synthesis and MIDI references

Generated directly by Swift `SoundroomCore` using `portable/tools/GenerateInstrumentFixtures.swift`. Regenerate from the repository root after compiling that tool with every Core Swift source, as for the DSP fixture generator.

- 63 kit/voice float WAVs cover every kit and drum voice, with velocity, round robin, damping and supported articulations varied across combinations.
- Three drum pattern JSON/WAV pairs cover legacy/electronic/extended kits, variation cycles, swing, timing/velocity humanization, probability, per-lane tuning/pan/gain, warmed cyclic tails and hat choking.
- 24 Designer JSON/WAV pairs cover sixteen presets, all four full oscillator combinations and four organic layered patches. Numeric seeds include values above JavaScript's safe integer bound and remain exact.
- Eleven prepared voice JSON/WAV pairs cover every instrument with controller changes and a short note release; Rust compares these at block sizes 1, 128, 257 and 8192. A stereo bank reference covers stealing, sustain, controller inheritance and release. One folded-loop render verifies release-tail placement.
- 24 MIDI files and Swift inspection JSONs cover articulated note export, controllers and quantization. A separately authored format-1 MIDI file retains named parts, separate channels, program changes and running status. Imported note IDs are intentionally file-local; musical fields and controller data are compared, rather than Swift's randomly generated IDs.

No expectations are computed by the Rust code under test. These short deterministic fixtures and the allocation probe do not qualify live OS hardware, callback deadlines, long performances or listening quality.
