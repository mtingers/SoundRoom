# Mac native capture custody prerequisite

This engine milestone adds host-only source integrity proofs. It opens no audio device, enables no Record operation/capability, changes no production backend and makes no hardware, microphone permission, routing, latency or listening claim.

## Native Stop to durable raw custody

The native owner first obtains `CoreAudioInput::stop_and_seal()`. It propagates the actual native Stop error; after a successful Stop, it waits without spinning for admitted callbacks to leave, closes entry to future callbacks and irreversibly seals the producer. A stopped receipt is opaque, scoped to that input queue and contains frozen native health, the selected device UID fingerprint and the actual native host-tick scale. A caller-created health struct or producer seal is insufficient.

The worker drains every accepted packet and obtains the queue's `CaptureSealReceipt`. It then consumes its archive worker through:

```rust,ignore
RecordingSourceWorker::finish_core_audio_capture(
    stopped: CoreAudioInputStopped,
    drained: CaptureSealReceipt,
) -> Result<CoreAudioCaptureCustody>
```

This validates source scope, final queue counters, native format, selected UID and host-tick scale. Native accepted frames must equal the archived accepted PCM; archive nonfinite counts must match the producer's frozen counts. A same-format receipt from another source, omitted packet, mismatched device label or clock scale cannot create custody.

Final queue overruns/faults and a `nativeInputStopped` event are journaled before raw completion. The native event retains started state, callback/accepted/timestamped counts, rejected callbacks/frames and failed Stop attempts/status. Reentry losses remain visible even though they cannot mutate the callback-owned queue writer. Native render/buffer/push failures now also retain rejected-frame evidence. Rejected callbacks have unknown placement in the received PCM coordinate: the event's source frame is the worker observation boundary, not a fabricated missing audio region.

A custody result can be complete or explicitly incomplete. Clean classification requires a started native source, no native rejection or failed Stop, valid timestamps for every accepted frame, no queue drop/device fault/nonfinite PCM and no suspended worker alignment. Incomplete custody retains the received raw data and health for reviewed recovery. A successful `completion.json` write proves a durable raw archive, not a clean native recording.

The archive hashes the four entries incrementally during worker writes, including raw Float32 bits and health events. Stop therefore performs no whole-take hashing pass. After syncing raw/events/completion and the directory, finish compares named entries with retained original file identities and closes **every** mutable archive handle. `CoreAudioCaptureCustody` owns the held private directory and exposes original `CaptureArtifactSeal` records (name, identity, length and SHA-256), native health, stats, capture scope and completeness. The service must compare these with its independent source admission fingerprints/hashes before consuming the directory. The proof itself cannot be caller-forged or cloned in safe Rust. Private storage's established same-owner/advisory-lock threat model still applies.

A future complete Record adopter must consume this opaque proof and enforce its classification. The existing directory-only host adopter remains a path for incomplete/operator-reviewed recovery; a directory and completion file cannot substitute for native proof. Service Record lifecycle activation and restart custody remain separate gates.

## Recovery keeps native loss visible

The recovery reader recognizes final native health. Clean snapshots add no loss issue. Rejected/reentrant frames, missing timestamp coverage, failed Stop attempts or a never-started source produce a `nativeInputHealth` issue requiring review, including the exact original event. Recovery without incomplete authorization rejects; explicitly reviewed recovery retains the received frame count and the issue. It never fabricates PCM for an unplaced native rejection or removes the issue merely because raw completion exists.

## Bounded uncertain native teardown

Native inputs reserve a permit **before device discovery or native opening**. At most four contexts can be active or retained as uncertain at once; each physical source can serve many track/channel routes. Clean teardown releases its permit. Failed Stop, a still-admitted callback, failed uninitialization or failed disposal permanently quarantines the permit together with the native unit/context, rather than freeing callback-owned storage. Further opens fail at the fixed bound with an actionable server-restart message. No cleanup retry races a retained callback; the OS reclaims that storage at process exit.

`input_admission_status()` offers a bounded coherent optional count snapshot for the host. The reservation fence, rather than an advisory count, decides admission. This conservative limit is an explicit multi-device/resource gate; it is not automatic device recovery or proof that four devices meet physical callback deadlines. Output-unit teardown and broader host/device-change orchestration remain separate qualification work.

## Source qualification

The tests inject native function tables and invoke the actual native input callback with prepared PCM/timestamp buffers; no device is enumerated or opened. Callback heap probes remain zero.

- Clean, reentrant, untimestamped, retried failed Stop and final queue-overrun fixtures exercise native callback → drained queue → worker archive → custody → actual recovery. Only the clean source is accepted without review.
- Foreign source, wrong UID/timebase, omitted PCM and renamed completion/manifest entries reject custody. Independent file reads verify original identities, lengths and SHA-256 seals.
- Four uncertain teardown paths (Stop, in-flight callback, uninitialize, dispose) exhaust a private fixed admission budget; clean teardown returns capacity. Tests exclusively own mock native resources and clean them up without hardware calls.
- Full debug/release engine tests, strict Clippy and terminal evidence are retained in `QA/capture-custody-macos.log`.

Native hardware/device-change/record start-stop acceptance is still pending. Record remains disabled until service custody, cancellation/deadlines, raw retention/quota/restart recovery and physical native host checks are integrated. Existing automation Write loop/handoff migration and extended gain/discrete/bypass/source-duplicate gates remain unfinished.
