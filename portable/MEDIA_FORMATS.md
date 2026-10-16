# Portable media workers

`AudioReader`, `AudioWriter` and `import_audio` in `crates/engine/src/media.rs` provide real streamed file operations. They never open an audio device or advance transport. Container detection uses file headers. Canonical project assets and the existing realtime graph remain stereo 48 kHz Float32 WAV; import creates that format from a selected mono/stereo source route. Service authority must register the completed artifact and retain source custody.

`AudioReader::from_file`, `AudioWriter::from_file` and `import_audio_from_files` consume admitted handles from [private storage](PRIVATE_STORAGE.md), preserving custody through path replacement. Constructors rewind source handles, reject nonempty output handles and never reopen checked paths. Handle-based import leaves failed unpublished artifact cleanup to the caller's stage; it does not remove or reopen a source.

| Operation | Formats and limits |
| --- | --- |
| WAV decode | RIFF PCM8/16/24/32 and IEEE Float32/64, including the existing extensible format support; 1–64 channels |
| AIFF decode | Signed big-endian integer PCM, 1–32 significant bits packed into 1–4 bytes |
| AIFF-C decode | `NONE` / `twos` big-endian PCM, `sowt` little-endian PCM, `fl32` / `FL32` and `fl64` / `FL64` big-endian IEEE float |
| Import | Explicit mono/stereo source-channel route; native rates 8–384 kHz, including fractional legacy AIFF rates; adaptive sinc conversion to 48 kHz |
| Export | Stereo Float32 WAV at 48 kHz; stereo AIFF PCM16/24 or AIFF-C Float32 at an explicitly supplied integer 8–384 kHz rate |

AIFF decoding follows Apple's original [AIFF/AIFF-C file specification](https://www.mmsp.ece.mcgill.ca/Documents/AudioFormats/AIFF/Docs/MacOS_Sound-extract.pdf): FORM/chunk bounds and odd-byte padding, arbitrary chunk order, one COMM, required AIFF-C FVER, authoritative frame count, SSND offset and explicit-integer-bit 80-bit sample rate. Additional PCM/float identifiers are documented in Apple's [sound sample descriptions](https://developer.apple.com/documentation/quicktime-file-format/sound_sample_descriptions). Unsupported compression rejects explicitly; there is no silent decoder fallback.

AIFF integer samples remain signed and left aligned when their significant bit depth is not a multiple of eight. Finite float headroom is retained; nonfinite or Float64 values unrepresentable in Float32 become zero, matching the WAV decoder policy. Integer exports round and clamp to the destination format. Dither belongs to the caller's explicit DSP chain and is never added twice by the container writer. Export requires an exact frame count, uses exclusive creation, and syncs only a complete result. AIFF's 32-bit FORM size imposes a 4 GiB bound; RF64 and compressed-codec export are not supplied.

The decoder holds at most 8,192 interleaved frames (4 MiB at the admitted 64-channel Float64 maximum), independent of duration. Import additionally holds fixed native/selected planar blocks and one bounded resampler for the selected channel union. Its filter uses about 3–24 MiB depending on ratio. Large sparse files are seeked and read directly; full files are never loaded into memory. Import checks cancellation between blocks, never replaces an existing destination, and removes only its own incomplete destination after closing it. The caller still owns global CPU/memory/disk admission, private storage, source leases and asset/revision authority. Decoder and resampling work stays off the callback.

## Qualification

`fixtures/aiff/native-reference.json` identifies provenance for every input. Apple AudioFile APIs created five PCM/float files, including four-channel 96 kHz and Float64 192 kHz. Two additional specification-created files cover little-endian PCM and a 22,254.54 Hz legacy fractional rate; they are explicitly labelled rather than represented as Apple-generated. Apple's AudioFile reader independently opened all seven, verified rates/format descriptors, and returned their original PCM bytes. `tools/GenerateAIFFFixtures.swift` reproduces this file-only check without calling any audio-device API.

`tools/ValidateAIFFExports.swift` independently reads Rust-created PCM16, PCM24 and Float32 exports with Apple AudioFile APIs and checks channel/rate/bit-depth metadata and samples, including format clipping and float headroom. Rust tests additionally cover 1–32-bit integer layouts, every truncation boundary in a small fixture, malformed chunk/header/version/rate/channel/depth/offset/frame combinations, first stereo pair versus all-channel routing, bounded 1.6 GB sparse-file seeking, cancellation/exclusive output, Float64 sanitation and fractional-rate conversion. The 997 Hz fractional-rate analytic reference measures a settled maximum error of 1.444e-6. Existing integer-clock recording/resampling/recovery tests remain unchanged.

These are file interoperability and deterministic DSP checks. Service/UI import/export integration, codec capability negotiation, Windows/Linux qualification, recording and physical audio quality remain separate gates.

Example file-only operations (with the workspace's Cargo configuration):

```sh
cargo run --manifest-path portable/Cargo.toml -p soundroom-engine --example media_files -- SOURCE.aiff
cargo run --manifest-path portable/Cargo.toml -p soundroom-engine --example media_files -- import SOURCE.aiff NEW_ASSET.wav
```

The example chooses mono channel 0 or the first stereo pair. The API supports an explicit route from any admitted source channels.
