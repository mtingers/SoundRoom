# Host-stamped continuous automation Write

Contract foundation in progress. No host advertises these operations or the `automation-write` feature until its real implementation is qualified. The portable Rust service implements them (`portable/crates/server/src/service_write.rs`) but advertises them only when `serve` is started with `--audio-default --experimental-automation-write`, an unqualified development opt-in. The Swift Mac host does not implement them. Clients must require the feature and the particular operations. The existing explicit point editor remains available. This document defines the shared Mac GUI/browser/portable host behavior; a passing codec test is not proof of audible control.

## Typed operations

All operations use the existing v3 `editor` envelope with one required base64 JSON payload. Context is the default empty context: no track, clip, range, effect, automation target or cursor supplied separately. `integers`, `numbers`, `strings`, and `flags` are empty. No request accepts a client time.

| Operation | Payload | Authoritative result |
| --- | --- | --- |
| `automation.writeBegin` | `RemoteAutomationWriteBeginRequest`: gesture UUID, track UUID, typed target, mode `touch` | `RemoteAutomationWriteBeginReply`: owner-bound lease, current target revision, initial normalized value, exact mapping/interpolation/error metadata, limits and real clock stamp. Exactly one expected track version is required. |
| `automation.writeValue` | `RemoteAutomationWriteValueRequest`: gesture UUID, monotonic positive safe-integer sequence, normalized finite value 0–1 | `RemoteAutomationWriteValueReply`: requested sequence, actually applied sequence, applied/superseded disposition, ramp start/target and real clock stamp. No authored revision/history mutation. |
| `automation.writeEnd` | `RemoteAutomationWriteEndRequest`: gesture UUID, final sequence (zero if no values), explicit cancel | Normal receipt payload `RemoteAutomationWriteLeaseUpdate`. A non-cancel End waits for the actual applied/superseded final sequence and final authored commit. |
| `automation.writeDraft` | `RemoteAutomationWriteDraftRequest`: exactly one draft UUID or gesture UUID | Operator/owner-scoped `RemoteAutomationWriteDraftReply` summary and existing verified-chunk `RemoteArtifact` for the full bounded prefix. No automatic reapply. |

Value/End/Draft use no expected resource revisions. Begin establishes the lease's target/version proof. End rechecks the current resource and original target curve before authoring. Lease identity always includes authenticated credential, runtime UUID, gesture UUID and server-assigned engine slot cookie. Guessing any ID confers no access.

The host may send an owner-only `automationWrite` event containing a small `RemoteAutomationWriteLeaseUpdate`. Only a peer that invoked Begin can receive this new event. The phase distinguishes writing, finalizing, committed, cancelled, conflict, expired, disconnected and failed. A committed update carries the actual authored revision; reviewable outcomes carry an opaque draft ID and explanation. It never installs an automation curve in the client by itself; ordinary authoritative deltas/snapshots do that.

## Audio and clocks

Up to 16 preallocated engine slots receive fixed-size packets from the control owner. Resolving UUIDs/parameter descriptors, parsing models, networking, file work, curve construction and memory allocation happen off the callback. Only the latest unapplied value for a slot needs to remain queued. Superseded sequences are acknowledged as superseded by an actual applied sequence, never assigned invented application points.

A callback applies a packet at a real processing boundary, using a short ramp in the saved lane's interpolation/mapping domain. The stamp contains transport epoch, submitted timeline/output frame **O**, actual automation lookup/control frame **F**, unwrapped playback clock and structural offset **F−O**. These are distinct coordinates. For track gain/pan, F includes master lookahead; insert parameters include downstream lookahead minus preceding processor latency. The offset is not a promise of precise audible onset for every effect, whose feedback/control histories differ. Saved points use F so offline replay can match live DSP. Fixture comparisons must establish this behavior for native and portable implementations.

Begin itself does not change audible values. Volume automation is normalized 0–1 mapping to amplitude 0–1.5; static saved track gain still supports 0–4. A previously unautomated gain above 1.5 cannot be preserved outside a new legacy lane, so the initial implementation explicitly declines that target without changing audio. Extending that scale with an interoperable migration remains required. Pan maps to −1…1. Effects use their canonical descriptors; logarithmic mappings and discrete choices must not be treated as interchangeable normalized linear ramps. Continuous supported descriptors qualify first; stepped/discrete Write remains an explicit gate.

Mapping error domains have explicit units: `physical` is absolute difference in the descriptor's physical units; `decibels` is absolute difference in 20·log10(amplitude), with the existing 1e-4 amplitude floor and hard-zero endpoints retained separately; `logarithmic` is absolute difference in ln(positive physical value), a dimensionless relative error. For the existing `%` descriptors, physical values are fractions (0–1 displayed as 0–100%): error 0.01 means one percentage point. Volume reports unit `gain`, pan an empty unit, and legacy amount `%`. A logarithmic mapping's normalized accuracy bound is ln-error/ln(maximum/minimum); physical bounds use a conservative maximum derivative. Host metadata supplies the accuracy, never UI coordinates.

## Curves and authorship

Touch captures actual applied ramps and the return to the currently evaluated original curve. Replacing the touched interval preserves every outside sample, including nonlinear curve subdomains and existing volume interpolation scale. It does not simply remove points by frame and reconnect neighboring knots. First/last samples, ramp interruptions, and true silence endpoints are retained. A simplifier may reduce captured data only within the declared audible-domain error tolerance. If the 4096 written-point budget cannot meet that tolerance, keep a reviewable prefix; do not silently truncate. Existing outside points remain intact, within the full model's 100000-point lane bound.

Only one owner writes a target. Competing target mutations conflict; whole-track replacements are checked against the leased target. Unrelated authored edits preserve their normal rights and cannot be discarded. Finalization merges only the lane into current authoritative content, preserving other fields, and creates one original-author Undo action. Whole-track version fencing alone cannot silently redefine unrelated-resource concurrency; target proof/migration handling needs explicit implementation tests.

Ambiguous original lanes with duplicate point frames are initially rejected at Begin. The original bytes/metadata are preserved; the finalizer also rejects them if encountered later and retains the actual prefix for review. Playback's existing last-point-at-frame behavior does not authorize silently normalizing unrelated authored metadata. An explicit duplicate-lane migration is a remaining parity gate.

Normal Stop freezes the applied prefix at the callback, releases the audible override, and returns its normal callback-confirmed transport receipt promptly. A worker then finalizes that prefix as one original-author Undo and reports the lease outcome. Stop never waits for large curve simplification or snapshot encoding. An explicit End waits for its authored outcome. Revoke, explicit cancel and conflicting target state restore Read with an explanation; a nonempty conflicting prefix remains reviewable.

## Bounds, uncertainty and lifecycle

Limits are returned by Begin: at most 16 leases, 65536 captured applied packets, ten minutes of actual frames, 4096 new written points, 100000 total lane points, 32 MiB full draft storage and a bounded value-idle lease timeout. The host must budget original lane/configuration ownership separately; these counters are not a total-heap claim. Values can renew a held constant target without client playhead sampling. Capture overflow terminates safely with a truthful draft outcome.

End/cancel, Stop and disconnect release keep reserved control capacity. A value request can wait only for a bounded actual callback acknowledgement; it does not occupy an unbounded worker queue. The SDK reserves terminal commands, retains exact uncertain IDs and never replays an authored End under a new ID. Duplicate requests and duplicate sequences are content checked before returning existing results.

Disconnect or idle expiry promptly restores Read and preserves a bounded owner-review draft rather than silently authoring a new request. A reconnect using the same authenticated runtime can look up the draft by its known gesture UUID if it missed the event. A different runtime, a revoked credential or an expired draft cannot retrieve even a known artifact ID. Draft materialization and SHA hashing run off the owner/callback; artifact chunks retain existing frame/base64 limits. No draft is automatically applied. A future deliberate reapply must use current resource versions and the same outside-curve preservation rules.

## Verified draft body

The artifact contains UTF-8 JSON `RemoteAutomationWriteDraftContent` version 1:

| Field | Meaning |
| --- | --- |
| `gestureID`, `trackID`, `target`, `trackRevision` | Original scoped gesture/target and source authored track version. |
| `baseline`, `mapping`, `originalPoints` | Exact original Read curve and its pre-first-point normalized baseline/interpolation mapping; at most 100000 points. |
| `reason` | Bounded explanation (512 UTF-8 bytes). |
| `capturedRamps` | Up to 65536 `RemoteAutomationWriteCapturedRamp` records: sequence only for an applied value, `restoring` only for the final return ramp, actual `stamp`, `startValue`, planned `targetValue`, and `rampFrames`. Frames and value sequences strictly increase, with one source epoch and structural offset. |
| `terminal` | Optional `RemoteAutomationWriteCapturedRelease`: actual release stamp and paired optional `lastControlFrame`/`lastValue`. When present, the stamp's source frame is the first untouched Read frame, exactly one after the actual last sampled prefix. |

A planned endpoint is never a claim that the callback sampled it. Later ramp stamps truncate earlier plans; the terminal's actual last sample truncates the final plan. A missing terminal, or absent actual last sample, is incomplete review data and cannot become an automatic authored edit. A client may inspect/export verified data; this version exposes no reapply operation. The artifact itself remains subject to the 32 MiB storage bound and credential/runtime/operator access checks.

Draft summary `appliedValueCount` counts sequence-bearing Applied records, excluding restoration. `firstStamp` is the first Applied stamp. `lastStamp` is the terminal stamp when present, or the final captured-ramp stamp for an incomplete draft; it reports the latest known receipt, not an invented completed ramp endpoint. The source `trackRevision` stays the Begin version even when the current mirror has advanced.

The initial epoch-fenced implementation declines loops and cancels safely on Seek, project replacement, device restart or incompatible graph rebind. This is a temporary integration boundary, not complete Write parity. Cycle-aware loop writing, seamless unrelated graph handoffs, extended volume scale, discrete parameters, process restart recovery and actual two-client/hardware/listening qualification remain open. Capability activation requires real host behavior; source compilation alone is insufficient.
