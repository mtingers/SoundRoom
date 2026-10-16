# Third-party notices

## Optional Demucs source separation

Soundroom's explicit source-separation setup installs Demucs 4.0.1 and official
Hybrid Transformer Demucs weights from Meta Platforms, Inc. and affiliates.
The default uses the per-source fine-tuned `htdemucs_ft` models; optional
`htdemucs` and experimental `htdemucs_6s` models provide alternate processing.

- Source and model documentation: https://github.com/facebookresearch/demucs
- Code license: MIT, https://github.com/facebookresearch/demucs/blob/main/LICENSE
- Pretrained weights have separate, unresolved licensing. The upstream author
  states that the code's MIT license does not cover the weights and that they
  are provided only for scientific purposes:
  https://github.com/facebookresearch/demucs/issues/327
  (reviewed 2026-10-15). That statement predates HTDemucs v4; we have not found
  a later upstream grant clearing the installed v4 weights for general or
  commercial use. Do not treat the code license, converted file format or a
  mirror's license label as clearance for these weights.
- The installer retains the full code copyright/license text in
  `build/SeparationRuntime/models/DEMUCS-LICENSE.txt`, records model provenance
  and checksums, writes a separate `DEMUCS-WEIGHTS-NOTICE.txt`, and records
  resolved runtime package versions. Old installed manifests may incorrectly
  label weights as MIT; this does not establish permission. Demucs and its
  dependencies remain a separate optional installation, outside the app bundle;
  a separate download does not resolve the weights' licensing.
- Soundroom performs bounded CPU inference, resampling and overlap-add;
  it does not modify or retrain the pretrained weights.

The proposed `nikhilunni/demucs-rs` backend has Apache-2.0 source code, but
uses conversions of the same upstream checkpoints. It has not been installed
or integrated. The production optional backend remains Python/Demucs; see
[installation and runtime details](BUILDING.md#python-source-separation-optional).

## DRSKit drum samples (DrumGizmo.org)

Soundroom's recorded drum kits ("Recorded kit · natural / tight close mics /
big room / vintage overheads") play audio derived from **DRSKit 2.1** by the
DrumGizmo team (Deva & Lars Muldjord), recorded with a kit lent by Jes Eiler
of DRSDrums. "Modeled from recordings" generates hits from parameters measured
on the same recordings (`portable/crates/engine/assets/measured-drums.json`).

- Source: https://drumgizmo.org/wiki/doku.php?id=kits:drskit
- License: Creative Commons Attribution 4.0 International
  (https://creativecommons.org/licenses/by/4.0/)
- Changes: the 13 microphone channels of each hit were mixed down to stereo
  close-mic, overhead and room stems (close mics panned to kit position,
  inter-mic bleed at -6 dB, snare-bottom polarity inverted), resampled from
  44.1 kHz to 48 kHz, trimmed and stored as 24-bit WAV
  (`portable/crates/engine/examples/build_drum_kit.rs`). The measured model
  is produced by `portable/crates/engine/examples/analyze_drum_kit.rs`.

DrumGizmo asks that productions using the samples credit:
"Drum samples provided by DrumGizmo.org".

## Recorded sound-designer instruments (CC0)

The sound designer's recorded instruments are converted (48 kHz, trimmed,
looped, 16-bit) from public-domain (CC0 1.0) libraries; no attribution is
required, but we gratefully note the sources:

- Versilian Community Sample Library (VCSL), Versilian Studios — Kawai grand
  piano, Yamaha upright piano, concert harp, vibraphone, marimba, kalimba:
  https://github.com/sgossner/VCSL
- VSCO 2 Community Edition, Versilian Studios — flute; violin, viola and
  cello sections: https://github.com/sgossner/VSCO-2-CE
- Karoryfer Samples / D. Smolken — Swagbass (bass guitar), Emilyguitar
  (electric guitar), Shinyguitar (archtop guitar, microphone recordings, used
  for the acoustic guitar), Otto Rubner double bass (pizzicato); Karoryfer x
  bigcat cello (Kamila Borowiak): https://github.com/sfzinstruments

Tooling: `portable/tools/instruments/` (download only mapped notes, convert
with `examples/build_instrument.rs`, write `manifest.json`).

## Optional separator research datasets

These audio datasets are downloaded explicitly into `build/Training`; they are
not included in the application. Local manifests and audition directories
retain the source links, credits, licenses and file hashes. Research models
start from our own random weights or our own earlier instrument baseline;
they do not use Demucs pretrained weights.

- VocalSet — Julia Wilkins, Prem Seetharaman, Alison Wahl and Bryan Pardo.
  CC-BY-4.0: https://zenodo.org/records/1442513
- Slakh/BabySlakh — Ethan Manilow, Gordon Wichern, Prem Seetharaman and
  Jonathan Le Roux. CC-BY-4.0; the pilot uses the BabySlakh v1 archive:
  https://zenodo.org/records/4603844.
- vocadito — Rachel M. Bittner, Katherine Pasalo, Juan José Bosch, Gabriel
  Meseguer-Brocal and David Rubinstein (2021). CC-BY-4.0:
  https://zenodo.org/records/5578807. All 40 clips are reserved for evaluation;
  they are excluded from training and checkpoint selection.
