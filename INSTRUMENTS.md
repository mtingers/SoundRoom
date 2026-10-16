# Native instruments and editable playback

The Sound designer contains the existing analog synth and ten project-native models: studio piano, tine electric piano, nylon plucked strings, marimba, glass bells, tonewheel organ, string ensemble, electric guitar, picked bass and vowel-shaped vocal ensemble. These use original modal, harmonic and FM synthesis. They contain no recorded instrument library or external plug-ins. Their names describe the intended character, not a claim of indistinguishability from an acoustic recording.

Choose **Instrument** to retain your notes, controller lanes and layers while loading a model's default envelope and tone. Hammer/mallet hardness, tine bite, pick position, bell/drawbar color, ensemble spread, pickup position, pick hardness or vowel color changes that model's excitation or harmonic balance. Resonance changes decay or sustained harmonic balance. Tone, drive and ADSR remain available; layers can combine these instruments with the existing synth presets, with independent gain, pan and transpose.

**Space / Play loop** starts an independent repeating pattern. Notes, lengths, velocities, instrument controls, layers, tempo and controller curves remain editable. Undo/Redo also updates the playing sound. Changes render on a cancellable worker; playback retains the old audio until the replacement is ready, preserves its musical beat and crossfades over 10 ms. A 60 ms debounce coalesces rapid edits. The cursor follows the audio renderer. Stop ramps down over 5 ms; MIDI recording remains a separate mode. Removing every note leaves a stoppable silent loop.

Preview folds release tails into the loop. A single stereo headroom gain prevents folded tails from hard clipping while preserving waveform and pan balance. Insertion/export retains the ordinary release tail. Loop preview and one-shot insertion therefore differ at the boundary by design. Pattern complexity, CPU load and duration affect the time before an edit is heard; the UI thread does not render the replacement.

New instrument models, new drum kits and expanded drum performance controls require project format 6. Formats 1–5 remain readable. A legacy pattern without the new fields retains its analog sound. Projects upgrade only when the new data is present; earlier builds reject format 6. Extended drum pattern banks use format 2, with format 1 still readable.

## Qualification

Automated checks cover shared live/offline voice PCM, instrument control sensitivity, finite output and headroom, extreme MIDI keys, legacy decoding, project and drum-bank roundtrips, loop replacement position, smooth transitions, stereo headroom and render partition invariance.

Further sound-quality work should use matched-level listening comparisons and musical performances. Acoustic piano sympathetic/pedal resonance, actual organ drawbars/rotary tone, bowed-string expression and portable recorded drum libraries are separate next steps. The current models substantially expand the native library, but automated DSP checks cannot establish acoustic realism or professional production suitability.

Electric guitar uses a pickup-position comb on decaying string partials; picked bass uses velocity/pick-dependent harmonic excitation; the vocal ensemble model morphs vowel formants on detuned voiced harmonics. Tone, envelope, resonance, controllers and layers remain editable. The vocal model is original wordless synthesis, not recorded singing or a speech model.
