# Mastering and live effects

## Workflow

Effects open as live track inserts. Track / Master selects the destination, with Expand on each insert row. Play track temporarily auditions only that track through Master; Play mix auditions the saved mix. Audition does not change project mute or solo flags, and live edits retain the chosen audition source.

Apply to audio reviews its exact scope before rendering on a worker. Ordinary inserts bake the complete track chain and its tails, removing those inserts to prevent double processing. Track volume/pan automation and Master stay live. First-stage scoped pitch retains its existing edited-region commit. Undo and aligned clip history retain the source, and the result stays selected. Normalize, reverse and fades remain explicit audio edits in a separate dialog.

The compact transport combines project identity, transport, input/output routing, Save and Export on one row. The track A button opens automation below its controls; Lane, Parameter and Read/Touch/Write live there rather than in the Effects drawer.

## Native processors

- **Master limiter:** 512-frame lookahead, sample or reconstructed 4× true-peak detection, fixed/adaptive recovery, input drive, ceiling, reduction history and loudness readouts. True-peak mode reserves 0.3 dB detection margin. Place it last; later inserts can raise peaks.
- **Resonance control:** stereo-linked 2048-frame spectral processor, adjustable frequency region, sensitivity, sharpness, attack/release, transient protection, blend and removed-signal audition. Twelve reduction bands show its activity. Harmonics can also be reduced; this is not a semantic detector of unwanted sound.
- **Warmth:** original Classic plus Tape, Tube, Console and Diode curves, bias, emphasis, oversampling and matched-level blending. New characters are native mathematical models, not emulations of named commercial hardware.

Resonance/Warmth group tabs keep compact controls readable. Presets, numeric entry and parameter automation use the same shared workspace as existing inserts.

## Master output

The dedicated Master row processes the summed mix and full-mix export. Individual exported stems and source-audio commits exclude Master. Master level precedes inserts, keeping a final limiter's ceiling meaningful. The rack accepts at most 4096 frames of summed insert latency. Empty and zero-latency racks reserve no lookahead; adding/removing a delayed rack uses a prepared whole-output transition.

Analyze mix measures the pre-Master mix using K weighting, gated integrated loudness, reconstructed true peaks, broad energy balance and local spectral peaks. Three-second sections inform conservative editable input/resonance curves. Review shows metrics, section loudness, target and amount, plus individually selectable tools. Optional color is bypassed initially. The aim is a reference, not a promise of exact loudness or aesthetic quality. Changed track content/settings require fresh analysis before using the proposal. Master edits alone do not invalidate it.

## MIDI import

Standard format 0/1 files import into the current native instrument. Multi-part, longer or dense arrangements show an explicit part and beat-range chooser instead of silent truncation. A Designer excerpt is limited to 64 beats and 256 notes. Notes, velocities and supported channel controls are retained, including controls active at an excerpt's start. Variable tempo and SMPTE division produce actionable unsupported-file errors. MIDI program numbers do not load an external sound library.

## Qualification

Software DSP checks do not replace musical listening, physical audio/MIDI interface checks, or signed-release qualification.

Detector and loudness reference: [ITU-R BS.1770-5](https://www.itu.int/dms_pubrec/itu-r/rec/bs/R-REC-BS.1770-5-202311-I!!PDF-E.pdf). Spectral processing background: [DAFx spectral processing](https://www.dafx.de/DAFX_Book_Page_2nd_edition/chapter10.html).
