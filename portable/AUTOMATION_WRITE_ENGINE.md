# Callback-applied automation Write engine

This milestone adds a bounded control path to the portable audio graph. It does not activate a host or client Write capability, modify the production Swift backend, open a device, or complete the collaboration/recording gates. The wire contract and authored history finalizer live outside the engine.

## Worker and callback ownership

A worker calls `PreparedGraph::resolve_write_target(&WriteTarget)` and `write_mapping(resolved)` against its canonical prepared generation. IDs and parameter strings resolve once on the worker. The returned target has private numeric indices and generation binding; callers cannot forge them in safe Rust. The mapping comes from the processor's own physical bounds, defaults and logarithmic/integer policy, and includes the original pre-first-point normalized baseline and linear/decibel interpolation scale. Effect Amount retains its saved normalized 0–1 domain; it does not pretend to be a named physical parameter. Units follow `SoundroomCore/EffectParameters.swift`.

The one control owner uses these `GraphControl` methods:

```rust,ignore
begin_write(gesture: u128, target: ResolvedWriteTarget) -> Result<WriteLease, WriteError>
write_value(lease: WriteLease, sequence: u64, value: f32) -> Result<WriteSubmission, WriteError>
end_write(lease: WriteLease, final_sequence: u64, reason: ReleaseReason) -> Result<(), WriteError>
poll_write(lease: WriteLease) -> Result<Option<WriteReceipt>, WriteError>
```

Begin requires actual playing callback telemetry and timestamp fields, reserves one of 16 slots, and waits for a callback `Began`. Begin never changes the control value. Runtime validation binds the slot to the graph generation, handoff token and transport epoch. A process-monotonic nonce keeps old handles stale even when a gesture UUID is reused. Duplicate active gestures and resolved targets reject; a seventeenth slot reports Exhausted.

Each slot owns fixed command, applied, restoration and terminal receipt storage. A CAS transfers ownership between its control publisher and callback consumer. Only an unclaimed Value can be superseded. A claimed/applied receipt cannot be overwritten; the publisher gets Busy until it consumes that receipt. Values must be finite normalized 0–1 with a strictly increasing positive sequence. No callback allocates or destroys heap storage, locks, performs disk/network I/O, wakes an OS thread or looks up an ID. The graph stores its override directly beside the target lane, rather than scanning leases per sample. Active gestures bound control processing chunks to 256 graph frames; ordinary playback preserves the existing larger block path.

## Actual receipts and authored replay

Applied values ramp over 256 frames using the saved lane's arithmetic (including decibel volume interpolation). `Applied` reports the actual next DSP lookup frame, attained ramp start and desired endpoint. The endpoint is planned, not evidence that future samples were rendered. A newer Applied or Restoring stamp truncates that plan.

`WriteStamp` carries:

- `output_frame`: the graph timeline frame at the actual callback boundary, O.
- `playback_clock`: its continuous unwrapped clock.
- `control_frame`: the actual source/DSP automation lookup coordinate, F.
- `transport_epoch` and `latency_offset_frames`: the captured run and structural F−clock offset.

Track gain/pan and master gain use F = output clock + master lookahead. A track insert adds its track latency and subtracts preceding insert latencies; a master insert subtracts preceding master insert latencies. Saved automation must replay at **F**, not the request time, client timer, or O. This structural offset is not a measurement of acoustic onset; processor state, queued lookahead and parameter semantics can affect when a listener hears a change.

Finished waits for the declared final sequence and final target ramp, then restores the original Read curve. A priority End may overtake its final Value: values through `final_sequence` remain admissible while closing. The service must impose a bounded lease deadline and explicitly abort if that value never arrives. Stop rejects an unapplied pending value, starts restoring immediately, and freezes the actually rendered prefix when its transport fade settles. Explicit cancel, revoke, disconnect or conflict also restores Read; a stale final sequence never prevents an abort.

Restoring reports its actual start and planned return endpoint. The callback splits irregular buffers at restoration boundaries, so Released gives the **actual** final sample `(F, normalized value)` and the first subsequent untouched Read frame. Normal End/Stop/abort therefore reports `nextReadF == actualLastF + 1`, including an interrupted partial Stop fade. A short fade does not author the unrendered remainder of a return ramp. Released without any applied samples has no last-applied value. Poll ordinary Applied/Rejected receipts first, Restoring next, Released last. Superseded/unapplied requests are not automation points.

The service owns committing the performed prefix as one original-author Undo or retaining a bounded conflict/disconnect draft. It must preserve the original curve on both sides, including the constant baseline before the first original point and nonlinear/decibel partial domains. It must never insert a line from an old earlier point to the gesture start that changes untouched audio. The engine performs no authored mutation or permission decision.

Live overrides change DSP history relative to the immutable authored model. A touched graph remains noncanonical even after Read is restored. Its forks, cache storage, checkpoint restoration and publication cannot masquerade as clean authored history. Worker replay must use a canonical checkpoint or rebuild the newly authored graph. A valid canonical checkpoint can explicitly restore clean history.

## Qualified pure behavior

`tests/automation_write.rs` drives the real `AudioHost::render_timed` API with **simulated** native timestamp coordinates and an allocation/deallocation probe. No device is enumerated or opened.

- Twenty independent live-performance versus authored offline replay scenes cover legacy amount, linear/decibel track/master volume, pan, named gain and filters, fractional echo, compressor, Chorus, EQ, Room, oversampled Warmth, Resonance, Dither, Pitch and master limiter. Every output sample compares within the existing 5e-6 tolerance; measured maximum is 7.53e-7. Source offsets include 16, 2,048 and 4,096 frames.
- Partial Stop replay retains only actual samples; irregular buffers terminate at the exact restoration endpoint. All four explicit abort reasons release Read. End-before-Value, unapplied Stop rejection, nonce reuse, empty callbacks, 16-slot capacity and stale seek handoffs are covered.
- Beginning is bit-identical to original playback; after release, legacy original Hold/smooth/ease-in/ease-out/linear Read curves resume bit-identically. Metadata and unsupported-target gates are tested.
- A two-thread 100,000-value mailbox stress resolves every sequence exactly once as consumed or superseded. An independent concurrent control/actual `AudioHost` test resolves 1,000 values and finishes the real ramp/restore/terminal path with zero callback heap operations.
- Existing full engine parity, paging, loop, synthesis, capture/recovery and authored history tests remain green. Final environment/commands and terminal results are retained in `QA/automation-write-engine-macos.log`.

## Explicit remaining gates

Write capability remains hidden until authenticated service ownership, deadlines, callback receipts, finalization/drafts and the client controls are integrated and qualified end to end. No hardware timing, physical output, listening, network latency or device reset claim follows from these pure tests.

The initial version refuses loop Write and cancels an active gesture on seek/rebuild/handoff or missing valid callback timing. Loop/seek/live-edit migration of a held gesture remains unfinished. Hardware discontinuity detection and recovery require the host's lifecycle integration. Normal authored Stop preservation is a service gate, not an engine-created Undo.

Unautomated gain above 1.5 rejects until an extended/versioned volume automation scale can preserve the native 0–4 baseline. Discrete/sentinel parameters, master pan, bypassed insert controls and legacy named-parameter/amount recipe promotion reject explicitly until their saved-path semantics have qualified migration policies. Modern continuous parameters whose existing recipe does not change when a lane is authored are supported. DSP faults/discontinuous cancellation must retain a reviewable draft rather than pretending a normal audible performance was committed. The service must reject duplicate source frames in the authored target lane before Begin: prepared playback intentionally retains its longstanding last-duplicate rule, while authored duplicate metadata remains ambiguous to a history-preserving finalizer. The authoritative project/context guard owns that admission; it must not normalize away metadata as a side effect of a gesture. None of these gates is silently marked complete.
