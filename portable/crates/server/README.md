# Portable control service

This crate adds the real v3 WSS listener and serialized authority around the portable engine. It is an integration checkpoint, not a qualified cross-platform release.

## Start

```sh
soundroom-server init /path/to/private-root "Studio"
soundroom-server grant /path/to/private-root "Engineer" operator /path/to/new-credential.json
SOUNDROOM_TLS_IDENTITY_PASSWORD='identity password' \
SOUNDROOM_WEB_ORIGINS='https://studio.example' \
  soundroom-server serve /path/to/private-root /path/to/identity.p12 0.0.0.0:8443 [project.soundroom]
```

Provision a PKCS#12 identity with the host name clients actually use. Clients verify the certificate chain and host name through their platform TLS stack; a private CA must be explicitly configured. There is no plaintext fallback or trust-all setting. `/control` accepts `soundroom.v3` WebSocket control traffic. `--web-assets /path/to/web/dist` serves an operator-selected bundled client over the same HTTPS identity. Assets are preloaded within 32 MiB (512 files,8 MiB each, eight directory levels), reject symlinks/reparse points, and are served from a canonical URL lookup. HTTP requests cannot access project/media/credential paths; security headers restrict scripts/frames and keep credentials out of referrers. Native clients can omit Origin; browser Origins must match the operator's exact approved HTTPS origin.

The service owns the root exclusively. Registry, credential and identity files must be owner-only regular files; Unix paths reject symlinks. Pairing codes expire after 120 seconds, are bound to approved name/role and are consumed on submission. `server.webClients`, `server.webPairing`, and `server.revokeWebCredential` require the authenticated operator role. Claimed roles never establish authority.

## Implemented workflows

- Shared tracks/master, resource versions, structural revision fences, add/remove/order, controls, split/delete/move and authored undo/redo. History refuses to overwrite another author's changed resource.
- Exact-ID/content receipt reconciliation, authenticated identity before cache lookup, bounded 256-receipt runtime windows. Large reply payload expiration retains the original status/revision; a new read-only request retrieves the data. Clients never automatically replay uncertain mutations.
- Up to sixteen connections, strict masked-client RFC 6455 framing, authentication deadline, explicit heartbeat, request-rate bounds, reserved control queues and a 128 MiB global retained-message/snapshot budget. Snapshot documents use SHA-256, stable revision and paced 512 KiB pages.
- Local file import through sequential verified chunks: 2 GiB per file, 4 GiB in-flight aggregate, four uploads globally/two per credential, ten-minute expiry and 8 GiB media budget. Import currently requires 48 kHz mono/stereo WAV. Client file paths never enter the wire contract.
- Server project list, atomic new save and operator open with a revision fence. Saves contain all source/history media; open waits for complete validation and refuses to replace a session changed during loading.
- Off-thread WAV mix/selection/stem export, cooperative rendering cancellation, progress throttled to four updates/second, owned artifacts, SHA-256 chunk download and expiry. Export has a 2 GiB artifact/4 GiB retained-artifact budget; stem requests accept up to sixteen tracks.
- Off-thread Designer/Drummer commit and Drummer export with exact seed handling, native instrument renders, target version fences, five-state clip history and cancellation cleanup.
- Native effect parameter units, presets, Designer instruments and drum catalog copied from the native capabilities exporter; command/features advertise only implemented portable operations. UI selection, scroll and zoom stay private to clients.

Project saves are the durable authored-document boundary. Unsaved working state/history is held by the running service; crash-recovery autosave/journaling remains a release gate. Import/export resampling and additional formats remain gates. Audio device choices, capture and loops remain unavailable; without the explicit default-output opt-in below, transport requests return `busy` and no fictitious playback clock is published.

## Validation and open gates

`cargo test -p soundroom-server -p soundroom-client` runs control/storage/history/worker/framing/pagination tests. Two real TLS integration tests additionally authenticate multiple clients and upload/save/open/export/download through actual WSS, and reject untrusted CA/wrong hostname/unapproved Origin. On macOS those are explicitly ignored by default because this managed environment rejects `native-tls` PKCS#12 identity import with `errSecParam` before socket bind. They must be run outside that restriction:

```sh
cargo test -p soundroom-server --test wss -- --ignored --nocapture
```

They have **not passed here**; they are not counted as completed WSS-service qualification. No audio device is opened by these tests. Hardware behavior requires separate device qualification.

The portable client has separately passed against the native Swift WSS host with normal certificate verification. Windows/Linux TLS, Windows owner ACLs/exclusive locks, socket behavior and hardware still require their own live runs. Windows now has a protected owner-only ACL provider, handle-based no-reparse/single-link checks, exclusive sharing locks and write-through sibling registry replacement. Existing broad or inherited private-file ACLs are refused. `portable/CI/windows.ps1` exercises actual Windows credentials/locks, native Schannel TLS and server workflows without audio hardware; that Windows runner execution is still pending.

The cached `native-tls`/crypto/dependency versions and custom bounded WebSocket codec need maintenance/security qualification before release. The codec is behind a transport boundary that can be replaced by a maintained implementation when dependency access is available. SHA-1 is used only for the public RFC 6455 Upgrade checksum; authentication is HMAC-SHA-256.

### Callback audio opt-in

The local macOS host (`serve-local`) supports recording from one microphone or
interface at a time. Every armed track's saved input UID and mono/stereo channel
assignment is validated before capture; explicit missing inputs and mixed-device
setups are rejected. Tracks using Session input follow the session device.
Stop/Finish can cancel recording preparation and count-in, and authored edits
remain blocked until capture and take finalization finish.

Loaded project audio is linked into managed `Media/` custody. Removing unused
working media preserves saved project packages. Recovery publication is serialized
and ordered by authored revision, including the final synchronous shutdown save.

`serve ... [PROJECT.soundroom] --audio-default` explicitly opens the macOS default output adapter. Without that flag the service opens no audio device and rejects audio transport. Attached capabilities advertise only callback-backed play/pause/seek; capture, device selection, per-track solo audition and loops remain unavailable in this service checkpoint.

Transport runs on one dedicated publisher. Preparation freezes the acknowledged callback position, forks the canonical graph, seeks through bounded DSP checkpoints, warms the target pages, and accepts a receipt only after the callback adopts both the handoff token and command sequence. Stop has an independent reserved queue and cancels preparation. The published clock comes exclusively from callback telemetry. Audio edits/history/project replacement are rejected while playback or preparation is active until future-frame authored graph handoffs are integrated.

Source PCM is paged within 32 MiB and a dedicated I/O worker pumps bounded callback requests. Worker decoder storage and DSP checkpoints each have separate 32 MiB bounds. Repeated seeks reuse checkpoints and immutable media; an uncached distant seek still needs canonical DSP preroll, and continuous checkpoint warming remains a performance qualification item. Actual default-output service listening checks are queued pending desktop availability; unit tests pump the real AudioHost without opening hardware.

Build the browser client with `npm run build` in `web/`, then add `--web-assets /absolute/path/to/web/dist` to serve. Open `https://SERVER:PORT/` through the normally trusted certificate; the approved browser Origin must match that exact HTTPS origin. `/control` continues to require the individual approved credential/role. There is no separate HTTP listener or certificate-warning bypass. Static request/preload unit tests pass; the actual HTTPS fetch is included in the pending native TLS listener integration fixture.

Large authored changes that exceed the 8 MiB frame are published as replacement
paged snapshots. One immutable serialized document is shared across peers and
charged once to the 128 MiB retained budget; queued page bytes remain charged per
peer. Changes while a transfer is running coalesce into one latest replacement
revision instead of leaking intermediate deltas across its revision boundary.
Full document admission occurs before edits or project replacement, and accepted
request receipts are retained even when a slow peer must reconnect for refresh.
The two-client >8 MiB fixture verifies monotonic 1→3 revision convergence and
budget release; a fully prepared intermediate revision can be delivered.

Initial snapshots, authored delta generation and oversized replacement encoding
run on one shared publication worker. It retains one in-flight immutable input
and one latest pending input, merging target sets; skipped authored revisions
become a replacement snapshot. A preparation barrier prevents edits overtaking
an initial snapshot. One latest transport/status/presence/device overlay is
replayed after snapshotBegin; metadata stays FIFO even after the final page is
queued, so priority metadata cannot replace newer state with an older replay.
The worker serializes one body for all target clients, and
project opening encodes before its final revision/credential fence outside the
control mutex. Deterministic blocked-worker tests verify control access, priority
heartbeat delivery, latest-revision convergence and budget release. The audio
publisher observes an atomic authored revision and uses nonblocking telemetry
updates, so an already enqueued Stop can complete its actual callback gain ramp
even while authority is occupied.

Candidate admission, authored-history traversal, typed state capture/cloning and
fine-grained controls still require latency and large-session scaling work; the
128 MiB budget covers retained serialized bodies/page queues, not all transient
typed model storage. These checks establish bounded correctness and isolation,
not qualified 5 ms control response for dense metadata or real-hardware latency.
