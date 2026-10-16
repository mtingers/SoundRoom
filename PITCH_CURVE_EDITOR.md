# Pitch curve editor

## Editing

1. Select a clip or audio range and choose Pitch correction. Analyze while stopped; analysis does not change audio or add an Undo entry.
2. Blue shows the measured input. The initial orange curve is a draft. Drag an anchor or choose Targets to activate it as a saved live insert. New editor inserts process only edited voiced regions, before the rest of the track chain. Existing inserts keep their original chain position and automatic processing outside edited regions.
3. Shift-click to select several anchors, or drag empty chart space to select points in a time range. Drag the group from its original snapshot; anchors cannot cross unselected neighbors or leave their voiced region. Numeric time, note and cents controls provide precise edits. Add point uses the chart cursor; Delete removes selected anchors. Targets do not bridge unvoiced gaps.
4. Choose an outgoing Hold, Linear, Smooth, Ease In or Ease Out curve. Smoothing changes selected interior targets while preserving region endpoints and timing. Retune, Humanize and Vibrato remain synthesis controls, separate from target-curve smoothing.
5. Time/pitch zoom, arrows, Fit and Selected explicitly control the viewport; edits do not change its vertical scale. Time zoom reaches 10 ms; the fine pitch grid reaches 25 cents. Expand opens the same bindings in a scrolling precision sheet. Space auditions/stops locally; Command-Z and Shift-Command-Z undo/redo committed project edits. Text fields retain native text Undo.
6. Recheck independently measures the rendered pitch-stage output without playing. Green is measured output, not a renamed requested curve. Original auditions this track with the edited pitch insert removed. Audition includes all its inserts and volume/pan automation. The main loop does not turn these one-shot auditions into a loop.

Natural moves measured targets halfway toward permitted notes; Tight fully snaps with linear curves; Robotic uses held snapped notes; Keep performance restores measured targets. Snap presets honor Key/Scale/custom pitch classes. Seeded presets stabilize each voiced run: a bounded 200 ms median sets its first note, and later note choices require an 18-cent preference plus 110 ms of continuous evidence. Confirmed changes keep their original onset. Short ornaments and ambiguous boundary notes favor stability; draw their targets manually when needed. This policy does not alter hand-authored points or Keep performance. An empty custom mask preserves measured targets. Parameter presets, including saved custom presets, leave manual curves intact. A portable effect chain saved with automation includes its pitch targets; a plain track/chain copy clears them while retaining the scoped insert's inactive mode.

## Applying to audio

The action states its scope before rendering:

- **Apply to audio:** for a first, scoped pitch stage, bake all of its edited regions together (including gaps between them), then remove that pitch insert. Other inserts and track mix automation remain live.
- **Apply full track:** for global pitch or a pitch stage later in the chain, bake the entire insert chain over the track and its capped tails, then remove those inserts. Gain, pan and mix automation remain live. This deliberate broader scope avoids processing the same effect twice or cutting a predecessor's small residual tail. A prefix-only experiment lost 1,440 tail samples despite matching all earlier audio; the implementation retains strict sample equality rather than hiding that discrepancy behind a tolerance.

The worker warms the same Mixer used by playback/export with absolute timeline automation clocks. It freezes resolved original clip envelopes before replacing audio and renders them exactly once. Source PCM remains available; one Undo/Redo includes the full chain and clip changes. Clip history recovers source audio, while project Undo restores effects too. Cancellation and changed audio, selection or session discard pending results without a history entry. Work runs off the main actor.

Full-track baking makes rendered tails visible as audio on the timeline. In multi-track sessions, conservative session-wide silent export padding can change when effect tails become clips; this does not establish universal identical export duration. Choose an explicit export range when a delivery must have an exact duration. Curve moves/copies use Move with automation, including split boundaries and cross-track moves. Ordinary audio-only moves leave timeline automation at its existing time.

## Persistence and limits

New projects use version 5. Version 1–4 projects remain readable; editing scoped targets upgrades their required version. Older Soundroom builds reject version 5 instead of silently dropping curves or changing scoped tuning into global tuning. Portable chains with scoped metadata use version 2; plain older chains remain version 1. Target/region IDs persist and are renewed when copying.

The detector measures a single pitch at 50–1000 Hz; synthesis correction remains limited to ±1200 cents. A requested target outside that shift range can differ from measured output. Dense plots cull invisible samples and retain extrema and unvoiced separators. Playback prepares indexed curve cursors off the callback and uses 5 ms raised-cosine scope gates inside edited regions, with exact aligned dry audio outside them.
