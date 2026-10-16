# AIFF file references

Generated on this Mac using `portable/tools/GenerateAIFFFixtures.swift`. This tool uses Apple AudioFile file APIs only; it never enumerates or opens a hardware device.

| File | Creation | Independent verification |
| --- | --- | --- |
| pcm16.aiff | Apple AudioFile, signed PCM16 stereo 48 kHz | Apple AudioFile format/PCM-byte read |
| pcm24.aiff | Apple AudioFile, signed PCM24 stereo 44.1 kHz | Apple AudioFile format/PCM-byte read |
| pcm32.aiff | Apple AudioFile, signed PCM32 four-channel 96 kHz | Apple AudioFile format/PCM-byte read |
| float32.aiff | Apple AudioFile, IEEE Float32 stereo 48 kHz | Apple AudioFile format/PCM-byte read |
| float64.aiff | Apple AudioFile, IEEE Float64 stereo 192 kHz | Apple AudioFile format/PCM-byte read |
| little16.aiff | Explicit AIFF-C `sowt` specification fixture, PCM16 stereo 48 kHz | Apple AudioFile format/PCM-byte read |
| fractional.aiff | Explicit AIFF specification fixture, PCM16 mono 22,254.54 Hz | Apple AudioFile format/PCM-byte read |

`native-reference.json` stores actual sample values, sample rates and separate `nativeCreated` / `nativeReadVerified` provenance. Apple file creation rejected the little-endian fixture's requested stream description; the generator therefore constructs that file explicitly instead of claiming native creation. The fractional-rate fixture deliberately exercises a legacy rate that the creator cannot represent as its usual integer-rate path.

The explicit layouts follow Apple's [original AIFF/AIFF-C specification](https://www.mmsp.ece.mcgill.ca/Documents/AudioFormats/AIFF/Docs/MacOS_Sound-extract.pdf) and [PCM format identifiers](https://developer.apple.com/documentation/quicktime-file-format/sound_sample_descriptions).

Reproduce into a **new** directory:

```sh
swiftc -O -module-cache-path /private/tmp/soundroom-aiff-swift-cache portable/tools/GenerateAIFFFixtures.swift -o /private/tmp/soundroom-generate-aiff
/private/tmp/soundroom-generate-aiff NEW_DIRECTORY
```

Rust-created exports are separately checked by `portable/tools/ValidateAIFFExports.swift`. Their test expectations are not produced by the Rust decoder.
