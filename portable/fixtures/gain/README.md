# Scalar gain and automation references

Eight full Swift-encoded projects and stereo Float32 reference renders preserve the native server's supported scalar gain range (0…4). Track, Master and combined scalar cases cover gains above the visible fader range. Delayed linear/decibel automation cases separately exercise the normalized 0…1 baseline before the first point and subsequent curves. The first scalar project uses format 1; the remaining projects use format 9.

References come from the actual Swift `RealtimeMixer`, in 257-frame blocks, with no audio device initialized. `portable/tools/GenerateGainFixtures.swift` generates a new output directory; it refuses to overwrite existing evidence. Compile it with `Sources/SoundroomCore/*.swift`, then supply a new directory and review differences before replacing committed fixtures.

Rust `tests/gain.rs` compares every stereo sample at block sizes 1, 128, 257 and 8,192 with the existing absolute tolerance 5e-6. All 32 comparisons are sample-exact on the recorded Mac build. These fixtures prove the covered gain paths; they do not qualify hardware, listening or control deadlines.
