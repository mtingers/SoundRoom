# Compact mixer controls

The portable authority stores gain, pan, mute, solo and arm edits as five scalar values plus an immutable content fingerprint. Authored Undo/Redo checks both current values and that fingerprint, so another client's change to the track cannot be overwritten. Adjacent edits with the same author, resource, action and gesture retain one before/after state. A content edit still follows complete candidate validation and the existing resource history path.

The service's shared publication worker establishes an immutable, validated session witness after a full authored document. The witness contains the exact byte length of the canonical session JSON and a small map of current mixer values. Later scalar edits update that length by replacing the five actual JSON scalar tokens; UTF-8, escaping, float representation and all unchanged content remain accounted for. The complete prospective envelope, revisions and media metadata are counted before authority changes. Admission or serialization failure leaves authored state, versions and history intact. Ordinary track content edits run complete validation and establish their next immutable witness on a preparation worker. The witness is installed only after authority commits its private validated token; a speculative worker never seeds another author’s future revision. Until an available witness qualifies a scalar command, that command also uses the full preparation path.

Captured session content is shared through `Arc`; scalar publication does not clone clips, notes or automation. A pure test preserves 100,000 automation points through a gesture and authored Undo/Redo. `updateTrackControls` qualifies only when every other field is unchanged; comparing a supplied automation array still takes time proportional to that array. Dedicated scalar commands avoid that repeated request payload. Initial source capture, deep document encoding and graph preparation remain separate performance work. This is not a measured five-millisecond hardware guarantee.

## Negotiation and compatibility

A host advertising both the `compact-control-patches` feature and the `client.features` editor operation accepts a read-only declaration whose base64 JSON payload is `["compact-control-patches"]`. Viewers may declare it. The flag belongs to the authenticated connection, resets on reconnect and can be cleared with `[]`. Unknown names or more than 16 entries are rejected without changing the prior declaration.

Opted-in peers receive `RemoteSessionDelta.controlPatches` containing a track UUID and any supplied mixer scalars. Other peers receive full track replacements with the new field completely omitted. If a replacement exceeds the frame limit, the existing verified paged snapshot path is used. Coalescing intermediate authored publications likewise produces an atomic latest snapshot; it never exposes a revision gap as a scalar delta. The Rust SDK declares support only when both capabilities exist and applies a whole patch set only after preflighting all targets and values.

Each new-format patch contains at least one non-null scalar. Gain is finite `0...4`, pan is finite `-1...1`, and a delta has at most 1,024 case-insensitively unique patch targets. Empty or absent patch arrays are valid. This preserves the existing native server scalar range. The visible fader and persisted normalized volume automation keep their separate 0…1.5 physical scale; a non-automated saved scalar above that scale is retained. Swift/Rust/TypeScript full-track and patch validators share the 0…4 scalar bound.

## Bounds and remaining qualification

The immutable authored snapshot body remains capped at 64 MiB, with neutral transport, status, presence and devices. Those live fields replay through the existing events after Begin (or after a small complete snapshot), with at most four retained entries of 64 KiB each per connection. New peers seed capture-time metadata before worker preparation; later arrivals win. Authored admission and byte-witness calculation count exactly the same neutral body which is encoded, so later metadata growth cannot invalidate its byte limit. A production Rust SDK test verifies live transport, operator presence, status and device routing through paged installation; a separate exact-limit regression proves body bytes stay exactly 64 MiB after overlay growth.

Frames remain capped at 8 MiB, and retained encoded publication storage at the existing global budget. The immutable typed witness shares captured content but is not charged as serialized storage; typed session/candidate memory and transient serialization must be included in eventual large-session memory qualification. Per-connection ordinary queues, reserved interrupt replies, latest-only publication coalescing and page pacing retain their existing bounds.

Authored edits now commit independently during playback. The portable bridge retains authored, pending and callback-applied revisions internally. A latest-only worker prepares the newest graph from the same immutable snapshot source, reconstructs its DSP history at a future unwrapped playback clock, warms its bounded source pages and requests a checked aligned handoff. The callback validates the current graph token and boundary before adoption; ordinary edits do not seek the playhead. The previous page loader remains active until the new graph is actually acknowledged.

Preparation runs outside the control owner, with one pending source, one running task, three splice attempts and a 30-second cooperative cancellation deadline. Stop/Pause/Seek supersede preparation; cancellation and missed splice deadlines retain the previous playing graph. Only one publication may await callback acknowledgement, including cancelled or expired work, so a later exact-token cancellation cannot overwrite an older waiting candidate. An accepted authored receipt is never changed to rejection because audio preparation later fails. Existing bounded status events report preparation or an actionable failure, and Undo/save/export retain the canonical authored content. Frozen edits refresh on the next acknowledged Play/Seek. Snapshot capture and source cloning still have their documented scaling costs.

Pure manually pumped callback tests cover live edits, newest-revision catch-up, authored Undo, failed graph admission without history rollback, retained failure status under a busy authority lock, and Stop freezing playback without discarding the accepted edit. Cycle-aware engine handoffs are available, but the portable service does not yet expose loop routing; physical callback deadline measurements, actual mixed-client listening, Windows runtime qualification and desktop/hardware checks remain distinct gates. Near-limit concurrent old/new snapshot lease deferral and complete network interrupt admission latency remain performance work. No hardware is opened by these tests.

## Full track preparation boundary

Ordinary track naming, structural edits, clip edits, full replacements and noncompact controls now prepare on bounded authored workers. Complete candidate/model validation, source-reference checks, 64 MiB document admission, normal authority resource checks, history sizing, content fingerprints and the next publication witness remain required. Workers do not clone or inspect other authors’ Undo/Redo histories. Their private tokens cannot be deserialized or applied to another authority.

Commit rechecks cancellation, the current registered credential/role, the source revision, recording state and custody file identity/length/modification stamps. Source files are server-selected custody assets; unchanged stamp identity is not a claim of content SHA verification against a malicious same-owner process. Immutable import/recovery admission and the shared private-storage provider own content hashes and custody guarantees. A successful edit preserves current callback transport rather than restoring its captured value. Publication or graph preparation failure after commit cannot revoke the accepted authored outcome.

A pure regression sends a 100,000-point replacement and Stop through the same authenticated identity, outbox and real service request ledger while its full preparation worker is deliberately blocked. A manually pumped callback acknowledges Stop before preparation resumes; the later edit retains stopped transport and authored history. Separate tests reject cancellation, revoked credentials, stale authority instances/revisions, changed author/role and changed custody files without revision/history mutation. These tests do not qualify actual socket admission or hardware deadlines.

The captured source session still deep-clones under the control owner. Prepared commits presently conservatively fence the whole source revision, so a concurrent unrelated edit may require refresh; final resource-only concurrent admission remains a compatibility gate. Full Undo/Redo, history gesture coalescing, old-state destruction and media/synthesis commit paths also retain work on the owner. These costs and bounded near-limit snapshot replacement must be qualified before claiming large-session network interrupt latency.


## Authenticated endpoint request decoding

Each authenticated TLS connection now owns an ordinary FIFO decoder worker with one current frame and one waiting frame, at most 8 MiB per frame and 16 MiB of reserved raw input per peer. Input reservations share the existing 128 MiB serialized budget with snapshots and output; ordinary input/output and snapshot retention leave 2 MiB of global headroom for priority replies. Reservations roll back exactly on queue/global rejection and release on disconnect or worker completion. Typed candidate memory, TLS framing buffers and transient parse allocations remain part of separate memory qualification.

Only validated Stop, job cancellation and small heartbeats stay on the TLS reader. Reserved request shapes are capped at 1 KiB and may not carry track revision payloads; unknown fields still pass through normal strict validators and cannot reach authority. Oversized interrupt lookalikes reject before owner dispatch. A decode error closes the affected connection with its existing policy-failure path; the retained diagnostic is bounded to 512 characters. No request is automatically replayed.

The ordinary worker constructs a private validated request bound to credential, runtime identity and role, then enters the same current-role, exact content-digest and receipt ledger checks. Before dispatching a next authored request, it waits outside the Service mutex until the preceding same-peer authored job terminally completes or is cancelled. This preserves authored wire order instead of allowing a faster later preparation to win. Stop/cancel/heartbeats bypass that wait. Ordinary worker jobs retain the four-job global bound; callback interrupts have a separate reserve of two jobs per authenticated peer and eight globally.

Pure tests cover blocked decoder Stop dispatch, ordinary receipt order, two ordered authored commits, malformed/oversized interrupt rejection, exact byte rollback and independent peer/global interrupt capacity. The ignored `service::request_acceptance::verified_wss_stop_while_large_authored_candidate_is_blocked` gate uses the real verified WSS client and endpoint with only a manually pumped callback. It requires explicitly supplied private localhost fixture identity and CA paths in `SOUNDROOM_TEST_TLS_P12`, `SOUNDROOM_TEST_TLS_PASSWORD`, and `SOUNDROOM_TEST_TLS_CA`; execution is separate from source checks. It measures Stop acknowledgement while a 100,000-point candidate is held after validation, then verifies the later accepted edit retains stopped transport. This is not a hardware deadline or listening qualification.


## Outgoing connection readiness

An authenticated portable peer now has one nonblocking TLS owner. Its socket and
private outbox wake descriptor are waited together; queued replies and failures
wake the owner immediately instead of waiting for the former 25 ms read timeout.
Unix uses a private socketpair and `poll`; Windows uses loopback wake sockets and
`WSAPoll`. TLS handshake stays blocking with its original bounded timeouts. The
single listener uses readiness and a 250 ms external-atomic shutdown check;
connections use actual heartbeat, idle and write deadlines instead of a polling
interval. Shutdown wakes authenticated owners through their existing outboxes.

The TLS socket records whether its underlying I/O blocked on a read or a write.
A write requiring incoming TLS data therefore waits for reads rather than spinning
on a writable descriptor. Each active frame retains its byte reservation until it
finishes or is destroyed. Partial writes retry the same plaintext range, in at
most 16 KiB attempts; pending replies retain priority at message boundaries.
WebSocket control replies have at most 16 small pending frames. A stalled frame
has a two-second write deadline; closing shares one bounded one-second deadline
and never inserts a Close header into an incomplete message payload.

A current large binary message cannot be preempted by another binary receipt
without violating RFC 6455 ordering. Compact deltas, smaller updates and network
backpressure remain part of latency qualification. Pure tests cover wake/deadline
behavior, notification flooding, exact TLS retry slices/read interest and retained
active-frame byte accounting. The opt-in verified WSS acceptance test now prints
30 Stop samples (median/p95/max) and a two-second idle CPU/wait count including its
simulated callback. These are observations on the actual test host, not a claim
that optimized five-millisecond p95 or Windows/Linux runtime gates have passed.

Readiness follows [POSIX poll](https://man7.org/linux/man-pages/man3/poll.3p.html)
and [Microsoft WSAPoll](https://learn.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-wsapoll).
