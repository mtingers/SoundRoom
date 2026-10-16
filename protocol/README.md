# Soundroom protocol 3 contract

This directory is the migration contract for browser and portable clients. Native
protocol 2 stays unchanged. `RemoteV3Codec` translates v3 messages into the existing
`RemoteWireMessage` and `RemoteCommand` values; it never executes, authenticates,
sets a role, changes history or skips server validation.

Editor operation names are maintained in `editor_operations.json`, independently of the audio host implementation.

Run `python3 protocol/generate.py --check` to detect contract drift. The extractor
reads all 37 current command cases, 18 event cases, 48 authoritative editor operation
tags and 62 generated record grammars (including the nonsecret web client record) and 15 raw enums.
Generated artifacts are `manifest.json`, `request.schema.json`, `messages.schema.json`, `session.schema.json`, `contract.ts`, `models.ts`,
`contract.rs`, and the compiled Swift `RemoteV3Descriptor.swift`. The schemas validate outer messages and the complete supported stored-field grammar,
including nested records, dictionaries, arrays, enums, range tuples and seed strings.
Swift and TypeScript validators enforce the same strict grammar. Models honor the
existing decoder defaults for old projects through format9; unknown fields/tags and
future project versions reject explicitly. Stored project files retain their native
JSON seed numbers; `session.schema.json` describes their normalized v3 presentation.
References, audio asset bounds, permissions and DSP/domain invariants remain
authoritative server validation. Passing the grammar never proves project/audio parity.

## Wire messages

WebSocket text payloads contain one UTF-8 JSON document, up to 8 MiB. No TCP
length header is included. Binary PCM never enters control messages. Transfer
chunks are base64 until a negotiated binary transfer capability is introduced.
Unknown tags and unrecognized outer fields are rejected. Optional fields may be
omitted or null; absent and null both mean no change/no optional value, never an
implicit fallback to a different audio device.

```json
{"version":3,"kind":"request","id":"12345678-1234-4234-8234-123456789abc","clientID":"12345678-1234-4234-8234-123456789abc","expectedRevision":7,"expectedTrackRevisions":{"12345678-1234-4234-8234-123456789abc":4},"command":{"type":"seek","frame":48000}}
```

Commands use `type` plus their named associated fields. The two unnamed Swift
command values become `editor.intent` and `editTracks.edit`. Full field names and
types are in the generated manifest. `setDevices` takes `inputUID` and `outputUID`
strings. The Swift adapter resolves these against the authoritative host's device
list; unavailable IDs reject instead of silently choosing defaults. Native v2's
UInt32 device ID remains local to its legacy adapter.

Events are `{version:3,kind:"event",event:{type,...}}`. Single unnamed values are
named `snapshot`, `delta`, `transport`, `metersByEffect`, `devices`, `peers`,
`sources`, `message`, `asset`, `analysis`, and `projects` as applicable. UUID-keyed
`trackRevisions` and `metersByEffect` are JSON objects, not Swift's alternating
key/value array encoding. Other model records keep project-compatible field
names. All unsigned 64-bit `seed` and `organicSeed` fields are decimal strings in v3, with exact
round trips through Swift and Rust; clients must never convert them to Number.
JSON encoded inside editor base64 payloads uses the same seed policy; the
Swift adapter restores exact UInt64 values before the existing typed decoder.

Replies are `{version:3,kind:"reply",requestID,status,revision,message,payload?}`.
Status is accepted/rejected/conflict/busy. `payload` currently preserves the native
base64 data response. It must be interpreted according to the originating command:
waveform → RemoteAsset; pitch analysis/render → RemotePitchAnalysis; project/drum
export → RemoteArtifact or artifact list; artifact read → RemoteArtifactChunk;
MIDI source query → RemoteMIDISource list; editor completion → RemoteEditorContext.
`readArtifact` returns raw immutable artifact bytes in base64, without JSON interpretation.
Replies preserve payload bytes exactly so transfer integrity cannot be changed by
model normalization.
A rejected receipt is not a successful edit. Typed payload tags remain a subsequent
compatibility milestone; do not invent different payload meanings per client.

## Negotiated scalar mixer deltas

`RemoteSessionDelta.controlPatches` is an optional array of typed patches with
`trackID` and optional `gain`, `pan`, `muted`, `solo`, `armed`. Each present value
replaces only that scalar; zero and false are values. Clips, effects, automation,
routing and other track state remain untouched. Gain is linear in 0…4 and pan in −1…1. Every patch has at least one non-null scalar; a delta contains at most 1,024 patches with unique case-insensitive UUIDs. Targets must exist and must not be removed by that delta. Clients preflight the entire patch set before changing metadata. Track/global revisions retain the
normal conflict and ordering rules. Patches are not permission to bypass the
server's validation or authored history.

Existing v3 decoders reject unknown fields, so servers must omit `controlPatches`
entirely for an unnegotiated peer, including empty arrays and null. A supporting
server advertises feature `compact-control-patches` and editor operation
`client.features`. After authenticated Welcome and that capability advertisement,
a supporting client sends a read-only `editor` declaration with operation
`client.features` and a base64 JSON array payload `["compact-control-patches"]`.
The server records support for that connection before emitting patches. Reconnect
requires a new declaration. Old clients continue receiving their original full
controls or bounded paged-document fallback; new clients continue accepting those
formats. Hello stays unchanged so new clients can authenticate to older servers.

The Swift decoder and GUI understand scalar patches and declare support only when
advertised. The native host validates declarations but currently emits full
controls and does not advertise compact emission. Rust SDK and browser decoding
and opt-in negotiation are implemented; portable server publication now supports
per-connection negotiated patches with exact document admission. Live mixed-version qualification remains queued.

## Authentication and authority

The initial WSS adapter retains challenge-response pairing:

- Challenge: `{version:3,kind:"challenge",serverID,nonce}`; nonce is base64.
- Hello: `{version:3,kind:"hello",serverID,peer:{id,name,role},credentialID,proof}`.
- Welcome: `{version:3,kind:"welcome",serverID,name,role}`.
- Heartbeats: `{version:3,kind:"heartbeat"|"heartbeatReply",id}`.
- Failure: `{version:3,kind:"failure",message}`.

Only the securely stored server credential registry assigns viewer/editor/operator.
The hello role is a compatibility field, never authorization. An authenticated
connection's peer ID must equal each request clientID. HMAC proof covers the nonce,
peer identity/name and credential scope using the existing pairing algorithm.
Protocol v3 is translated back to internal v2 only after the wire version is
checked. HTTPS trust/pairing approval, origin validation and revocation are transport
responsibilities and must not be treated as implemented by this codec.

Request IDs retain existing deduplication semantics: retry the same ID only when
recovering the same uncertain command. A new ID means a new command. Revisions and
track revisions are authoritative stale-edit checks. Track gestures preserve their
`gestureID`, history name and authenticated author. Undo targets the caller's history
and cannot overwrite newer foreign edits. Client selection, scrolling, zoom,
workspace drafts and cursor stay private; transport changes are explicit commands.

Operator-approved browser pairing uses a separate, pre-authentication envelope
handled by the web channel: `{version:3,kind:"pair",code,name}`. On success the
server returns `{version:3,kind:"paired",credential:{version:3,serverID,credentialID,clientSecret,role}}`.
The clientSecret is base64 and never includes the native root TLS PSK. Approval
codes expire after 120 seconds, are single-use and bind an operator-approved
client name and role. `server.webPairing` and `server.revokeWebCredential` require
operator authorization in the authoritative host. The general codec does not
accept pairing envelopes as authenticated commands.

## Units and number policy

| Value | Meaning |
| --- | --- |
| frame/start/end/offset/length | Nonnegative integer samples on the host session timeline; half-open ranges |
| sampleRate | Frames per second; negotiated by host (current engine: 48000) |
| hostTime | Host monotonic seconds, never UTC or client wall time |
| gain | Linear amplitude multiplier; track/master scalar controls 0…4; normalized volume automation uses 0…1.5 |
| pan | -1 left, 0 center, +1 right |
| effect parameters | Physical units from RemoteParameterMetadata (Hz/ms/s/dB/Q/etc.) |
| automation values | Normalized 0…1 for effects; mixer scale is persisted explicitly |
| `%` parameter metadata | Stored as fraction 0…1, displayed as 0…100 percent |
| tempo | Quarter notes per minute, 20…300 for project timing |
| beatsPerBar / beatUnit | 1…16 / one of 2, 4, 8, 16 |
| musical tick | 1/960 quarter note; bar and beat display are one-based |
| channel | Zero-based host device channel, never client device channel |
| deviceUID | Opaque host backend identity; clients display the host-provided name |
| velocity / probability | Normalized fraction unless explicitly MIDI wire velocity 0…127 |
| job progress | Fraction 0…1 |
| Date fields | Seconds since 2001-01-01 00:00:00 UTC (Foundation reference date); Unix display adds 978307200 seconds |
| hostTime | Monotonic host uptime seconds; compare only within one server runtime |
| latency | Integer frames; milliseconds are display conversions at session rate |
| createdAt in old projects | Foundation reference-date seconds (2001-01-01 UTC); preserve legacy format |
| sha256 | 64 hexadecimal characters; integrity of the exact file/document bytes |

Every numeric value must be finite. JSON integers crossing the public protocol are
bounded by ±9,007,199,254,740,991, exactly representable by JavaScript. Revisions,
frames, offsets and sizes are nonnegative. A host reaching maximum revision must
reject further mutations and require a new epoch/server session rather than wrap.
Int64/UInt64 synthesis seeds are strings. Frame arithmetic must be checked for
addition overflow and the host's stricter supported timeline bound before editing.
Passing wire validation never grants access or replaces domain/range validation.

Capabilities are sent after authenticated welcome as
`{version:3,kind:"capabilities",capabilities:RemoteCapabilities}` using
`RemoteV3Codec.encodeCapabilities`/`decodeCapabilities`. They include complete factory
effect values, Designer instrument/organic presets, drum patterns, kit voices and
articulations. Feature strings are supplied explicitly by the authoritative host;
an empty feature list never implies all features.

Capabilities are host facts. `RemoteCapabilities` centralizes protocol/project
versions, commands, editor operations, sample rate, actual host codec/backend lists,
limits and effect parameter metadata read directly from DSP definitions. Pagination
is false by default until the host and client assembly path both support it. Empty
backend/codec lists do not mean all backends/codecs are available. The receiving client must negotiate supported versions/features explicitly; a compiled
backend is not qualified audio hardware.

## Revision-consistent large snapshots

`RemoteSnapshotPages` freezes an immutable snapshot document at revision R and
serializes it off the audio callback. `RemoteSnapshotAssembler` installs nothing
until every sequential page and final SHA-256 agree. Messages:

```text
{version:3,kind:"snapshotBegin",transferID,revision:R,totalBytes,sha256}
{version:3,kind:"snapshotPage",transferID,revision:R,offset,data:<base64>}
{version:3,kind:"snapshotEnd",transferID,revision:R,totalBytes,sha256}
```

Decoded pages are at most 512 KiB (encoded frames below 1 MiB); assembled documents
are at most 64 MiB. This replaces a single-message ceiling without an unlimited
memory promise. A transfer retains one immutable document; transport must stream
pages under its 32 MiB queue budget, not enqueue the entire transfer. Mutations
accepted after R are held/coalesced by the client and applied after snapshot install.
A missing page, conflicting transfer/revision or bad digest rejects assembly and
keeps the prior session. On disconnect, resume is possible only if the host still
retains the exact transferID/document and nextOffset; otherwise begin a new snapshot.
Resume is specified here but is not yet an implemented host retention service.

For still larger sessions, the next model contract must expose entity pages and
incremental note/clip/automation operations instead of raising the document bound.
Current track replacement remains an explicit compatibility command. Existing
native-v2 clients retain their documented metadata ceiling until migrated.

Run `bash protocol/check.sh` for generated
drift checks, TypeScript runtime fixture checks and Rust metadata compilation.

## Validation evidence

Swift fixtures exercise every command and event, handshake/reply round trips,
authorship/revisions, exact UInt64 seeds, malformed UUIDs, unknown fields/operations,
boolean and fractional integers, unsafe numbers, unresolved devices and oversized
upload chunks. Snapshot tests cover multiple pages, delayed install, digest failure,
revision mixing, offsets, bounds and cancellation. Capability tests compare DSP
parameter ranges/defaults/scales. TypeScript validator checks can run without npm:

```sh
python3 protocol/generate.py --check
tsc --target ES2022 --module commonjs --strict --outDir /tmp/soundroom-contract-ts protocol/validate.ts
node protocol/test.cjs /tmp/soundroom-contract-ts/validate.js
```

Hardware/latency, complete Rust DSP parity, full audio/domain parity, capability
negotiation, host pagination scheduling/resume and authentication lifecycle remain
separate acceptance work. This foundation does not mark those gates complete.

Typed Rust records, enums, commands, events and grammar descriptors are generated
in `generated/models.rs`. The portable client crate compiles these against Serde
and validates nested wire values before they enter client state. Secret-bearing
credential/grant records deliberately omit `Debug`. This artifact supplements the
standalone operation metadata in `generated/contract.rs`.
