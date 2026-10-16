# Effect workspaces

Effects share precise parameter controls, explicit processing scope, visual feedback, and a native dark interface.

The expanded pitch editor closes after Apply to audio succeeds, returning to the processed clip and audio range selected in the timeline. Processing failure, cancellation and discarded stale work keep the editor open. Undo/Redo restore the corresponding audio selection without issuing another dismissal. Local smoothing Apply continues editing the same selected points.

## Controls and feedback

Every live-capable effect uses the same parameter controls in Selection and Real-time. Groups describe the processor rather than an arbitrary list of sliders. Numeric entries use physical units; sliders keep native keyboard and accessibility behavior. Presets identify their current settings and retain project automation. Custom presets save parameters; effect chains with automation carry timeline curves and pitch targets.

| Effect | Editing focus | Appropriate feedback |
| --- | --- | --- |
| Gain | Level | Gain in dB and amplitude factor |
| Low-pass / high-pass | Cutoff and resonance | Frequency response calculated from the processor coefficients |
| Three-band EQ | Low, mid, high and output | Frequency response of the actual peaking bands |
| Compressor | Threshold/ratio/knee, timing, output | Steady-state transfer settings; real input/output/reduction meters in live mode |
| Echo | Repeat timing, feedback, tone and blend | Timing/repeat settings, distinguished from measured audio |
| Room reverb | Decay, pre-delay, tone and blend | Clear timing and tone values; no invented room measurement |
| Warmth | Drive, tone, level match, output and blend | Settings summary; dynamic level match cannot be represented by a single transfer curve |
| Chorus | Modulation, delay, width, feedback and blend | Delay-head movement implied by the settings |
| Dither | Target depth, noise and processing | Quantization-step settings, distinguished from measured noise |
| Pitch correction | Measured notes and editable targets | Independently measured input/output and live target curves |
| Normalize, fades, reverse | Selection-only operations | Explicit operation and scope |

Parameter-derived diagrams describe nominal settings. Automation can override these values during playback; they are not measured output traces. Only real telemetry is presented as a meter. The compact drawer and larger precision editor share current project bindings rather than detached copies.

## Compatibility and scope

Older one-knob processors keep their original signal path until explicitly upgraded. Mapped physical values are starting points; expanded algorithms may change the sound. Saving or recalling a legacy preset retains its legacy amount. A first physical selection edit initializes the entire mapped parameter set instead of unexpectedly resetting untouched controls.

Selection preview processes the selected audio. Apply to audio creates an undoable rendered edit with retained source history. Live inserts process the track during playback/export; their precision transport plays the mix. Modern Echo, Room and Chorus support optional selection tails. Older one-knob Echo/Room retain their original selection length until explicitly upgraded. Automation stays attached to the project. Pitch retains its more specific scoped-target and full-chain baking rules documented in [PITCH_CURVE_EDITOR.md](PITCH_CURVE_EDITOR.md).
