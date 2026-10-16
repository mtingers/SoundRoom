# Server-stamped automation Write proposal

Status: **not implemented**. Continuous Write remains a parity gate. Current clients can edit explicit automation points, but neither network arrival time nor locally interpolated playhead time qualifies as an authoritative recording clock.

## Contract and ownership

Advertise a new feature and canonical operations only after the native and portable host implementations are qualified. Do not add unrecognized fields to an existing strict envelope.

| Operation | Proposed input | Result |
| --- | --- | --- |
| `automation.writeBegin` | Track context; expected track revision; UUID gesture; target volume/pan/effect amount/effect parameter; effect/parameter identity where required; mode `touch` | An owner-bound target lease, host transport epoch, target value mapping and capture limits |
| `automation.writeValue` | Gesture UUID; monotonic sequence; normalized value | Host acknowledgement with sequence, transport epoch and actual applied session frame |
| `automation.writeEnd` | Gesture UUID; optional cancel flag | One authored undoable commit, or cancellation restoring the original target |

No value request contains a client timestamp. Operator permission is required for live control; viewers may inspect the resulting curve. A later explicit distinction between edit and hardware control roles can broaden this deliberately.

Begin snapshots the target curve and validates its revision. Only one writer owns a target. Competing edits to that target conflict; unrelated tracks continue. Whole-track replacement touching the leased target must also conflict. End merges only that curve into the current authoritative track, so unrelated controls are preserved.

Host capability metadata must specify target normalization and curve scale. In particular, the track fader's scalar gain range of 0–4 must not be assumed to equal the existing volume automation lane's amplitude range. Effect parameter values must use their advertised logarithmic/linear mapping; gain-effect parameters remain dB.

## Real-time path

The control/authority actor validates values and publishes fixed-size parameter packets to a bounded, preallocated queue. The hardware callback consumes a packet at a known sample boundary, applies the same short ramp used by normal control changes, and stamps its session frame plus transport epoch. The callback does not allocate, lock, parse JSON, serialize snapshots or simplify curves.

The non-real-time actor drains stamped packets into bounded capture storage. Sequence numbers distinguish application from delivery acknowledgement. Coalescing may replace an unapplied value with the newest value, but must not invent timestamps for discarded values. Acknowledgements report which sequence actually reached audio.

Stop, note-off and cancellation retain a reserved priority lane. Heavy analysis, export or project admission must not prevent ending a write gesture. Offline hosts must decline live Write instead of fabricating callback clock progress.

## Curve quality and limits

Begin returns explicit limits, with proposed initial bounds of 128 pending value packets, one lease per target, 16 global leases, a ten-minute capture duration and 65,536 stamped samples per lease. The final project lane remains bounded by the supported 4096-point model.

A finalizer may reduce dense samples only within a declared error bound measured in the target's audible domain (volume dB, physical logarithmic effect parameters, pan position). Preserve first/last samples and sharp direction changes. If the supported point budget cannot meet that bound, return a reviewable failure; do not silently truncate or install a lower-quality curve. Simplification tolerances and longer captures need an explicit DSP quality decision before shipping.

For Touch, replace only the stamped touched interval. Preserve the original curve outside it, including normalized subdomains for nonlinear curves, and restore the original automation with a ramp on release. Automation value mappings and outside-interval preservation require per-frame regression tests.

## Lifecycle and uncertainty

- Explicit End commits one Undo gesture. Cancel restores the original target and does not author a curve.
- Owner Stop ends the gesture deterministically before transport teardown. A different operator's Stop must have a documented policy; it cannot silently impersonate a new author.
- Disconnect cancels the live lease and restores its original target. A previously received End remains governed by its exact request identity and receipt ledger.
- Seek, project replacement, device restart and loop discontinuities close or cancel the capture before changing epoch. The first version must state this clearly rather than stitching unrelated frames together. Loop overdubbing is a separate refinement.
- Duplicate End requests are idempotent through the existing request ledger. An uncertain End is reconciled by its original identity; clients never automatically replay a fresh authored request.

## Required acceptance

Pure checks: ownership, monotonic sequences, unknown targets, queue/capture bounds, target revision conflicts, unrelated-track concurrency, atomic curve installation, cancel/disconnect cleanup, exact original-domain preservation and duplicate/uncertain receipts.

Engine checks: callback sample stamping, ramps, correct normalization, priority Stop under heavy edit/analysis, no callback allocation/locking, and transport epoch transitions. Compare rendered automated samples against the original curve outside the touched interval.

Actual qualification: two controlling clients, constrained-network acknowledgement delay, long gestures, headphone listening, native/portable backend behavior and Windows/Linux/macOS device runs. UI status must distinguish pending packet, host-applied packet and authored curve receipt.
