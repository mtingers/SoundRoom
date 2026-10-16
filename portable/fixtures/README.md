# Swift/Rust audio comparison fixtures

Generated on macOS from `Sources/SoundroomCore` using `portable/tools/GenerateParityFixtures.swift`. Each `.soundroom` directory contains the source WAV and full Swift-encoded format-9 session. Each `*-swift.wav` is a stereo float reference render (14,000 frames, 24,000 frames for pitch, or longer for explicitly tail-aware cases) produced by the Swift `RealtimeMixer` in 257-frame blocks.

The Rust parity test opens the actual projects and compares all output samples, including sample positions spanning clip joins, legacy echo delay, automation boundaries, master gain ordering, latency-aligned baked-range bypass, sample/true-peak limiting, spectral reduction, monophonic pitch correction, saved manual regions, formant shifts, automatic scales, fractional octave targets, detector-measured corrected output, oversampled character transitions and finite tails, at four block sizes. No expectations are calculated by the Rust implementation under test.

These short fixtures intentionally cover the currently ported deterministic subset. Unported inserts fail explicitly. Hardware, long-session performance, loop/transport ramp behavior, synthesis and sound-quality qualification require separate fixtures and gates.

Pitch contour JSON references are generated independently by Swift `PitchContourAnalysis` from the original/tuned waveform. Tests compare detector timing, confidence and measured frequency, plus intentional upward/downward pitch behavior. The voiced input contains harmonics, a phrase gap, a pitch transition and opposed stereo polarity.
