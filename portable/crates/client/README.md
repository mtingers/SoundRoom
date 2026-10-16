# Portable control SDK

`Client<WssTransport>` connects to the hardware host using HTTPS-origin WSS with
normal platform certificate and hostname verification. A private CA PEM can be
added explicitly. There is no plaintext fallback, trust-all switch, root PSK in
browser credentials, or audio device ownership in this crate.

The client compiles the shared generated Rust models and validates nested records,
safe integers, exact decimal UInt64 seeds, command/editor tags and known typed
payloads. The server still owns permissions, references, device availability,
resource revisions, authored history, jobs, auditions and DSP.

Run the client on one control worker and poll regularly (e.g. 50 ms). UI render or
audio callback threads must never call the blocking transport, authentication,
file hashing, state validation or transfer APIs. `ControlTransport` is the
replaceable boundary for a maintained WebSocket implementation.

```rust,no_run
use soundroom_client::{auth::Credential, client::Client, transport::WssConfig};
let credential = Credential::load(std::path::Path::new("pairing.json"))?;
let config = WssConfig::new("wss://studio-mac.local:49153/control");
let mut client = Client::connect(&config, credential, "Studio terminal", None)?;
while client.connection != soundroom_client::client::ConnectionState::Ready {
    client.poll(std::time::Duration::from_millis(50))?;
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

`view` contains selected tracks/clips, cursor, scroll, zoom and panel state. It is
local metadata, serializable by the owning client, and survives snapshot installs
and reconnects. Authored edits and transport events are shared host state.

Snapshots install atomically only after ordered, revision-pinned pages and the
complete document SHA-256 match. Pages are 512 KiB, total documents at most 64 MiB;
newer deltas are retained with bounded count/bytes and applied after installation.
Transport, status, presence and routing telemetry received during assembly is retained.
A forward revision gap requests a fresh snapshot. No partially assembled session
replaces the previous one.

Admission reserves eight request slots and 64 KiB for transport/interrupt controls.
Ordinary requests, inflight/uncertain request bytes, receipts and deferred deltas
have explicit count/byte bounds. `flush` sends at most eight frames, placing Stop,
Seek, cancellation and note-off controls ahead of bulk traffic. The framing codec
accepts only canonical uncompressed RFC 6455 frames; tests cover fragmentation,
UTF-8, masking, reserved bits/opcodes, control frames, length overflow, queue
flooding and close status. Close has a one-second deadline.

Every request retains its ID and exact bytes. Disconnects or missing receipts
leave sent edits uncertain, and reconnect never replays them automatically.
`reconcile_exact` is an explicit operator decision to resend the original ID and
bytes while the same host runtime's bounded receipt cache is known to remain
valid. It must not be used after server restart/cache expiry. `abandon` releases
that retained request without making a new edit. A higher-level UI must explain
uncertainty instead of blindly generating a new request ID. Reconnect preserves
the client runtime ID and uses the bounded jittered `reconnect_delay` helper.

`upload_file` hashes local data, sends 256 KiB chunks, verifies every receipt and
then finishes at the requested track/frame. Only the leaf name and bytes cross
the wire. A target's observed revision is included in FinishUpload. An uncertain
chunk/final receipt stops the transfer for reconciliation; it is never replayed
automatically. Download verifies manifest size, offsets, completion and SHA-256,
removes incomplete files on failure and installs a new destination atomically
without overwriting existing files.

One-time pairing codes can be redeemed with `Credential::pair` over verified WSS;
`write_new` creates a private new credential file without overwriting an existing
identity. Secret-bearing credentials/grants omit Debug. Unix acquisition uses
owner-only regular-file permissions, a single link and O_NOFOLLOW. Windows
acquisition opens non-inheritable, non-reparse handles and verifies current-user
ownership, one explicit owner-only allow ACE, a protected DACL and one file link.
Broad, inherited or unfamiliar ACLs are rejected rather than repaired. Windows
files are created with a protected owner-only descriptor, and service locks use
exclusive kernel sharing. Other platforms still require a qualified provider.

## Validation and remaining qualification

```sh
CARGO_HOME=/private/tmp/soundroom-cargo cargo test --offline \
  --manifest-path portable/Cargo.toml -p soundroom-client
CARGO_HOME=/private/tmp/soundroom-cargo cargo build --offline \
  --manifest-path portable/Cargo.toml -p soundroom-client --example wss_probe
portable/target/debug/examples/wss_probe \
  wss://localhost:49153/control pairing.json 'Approved client name' ca.pem \
  https://localhost:49153
```

The probe uses the actual TLS/WSS transport and native challenge/receipt flow;
unit fake transports only verify state and failure rules. Cross-platform targets,
Windows ACL runtime qualification, Windows/Linux TLS trust integration and independent
hardware behavior remain qualification gates. Cached dependency pins make the
current offline build reproducible; release requires current advisory review,
upgrade qualification and a maintained WebSocket library audit/replacement.
SHA-1 exists only for RFC 6455's public Upgrade nonce checksum. Authentication uses
the maintained SHA-256 primitive with standard HMAC and RFC 4231 test vectors.

Windows source qualification includes `portable/CI/windows.ps1` and five dedicated
ACL/credential tests. This Mac cannot install the Windows standard library under
its managed permissions/network, so actual Windows compilation and execution
remain external gates. A host-only signature check of the Win32 provider API
passed; it is not counted as a Windows target build or runtime pass.

The provider follows [Microsoft file security](https://learn.microsoft.com/en-us/windows/win32/fileio/file-security-and-access-rights), [handle security descriptor inspection](https://learn.microsoft.com/en-us/windows/win32/api/aclapi/nf-aclapi-getsecurityinfo) and [protected DACL syntax](https://learn.microsoft.com/en-us/windows/win32/secauthz/security-descriptor-string-format).

The SDK declares `compact-control-patches` through read-only `client.features`
only when both the feature and operation are advertised by the connected host.
It declares once per authenticated connection, retries bounded queue pressure,
and re-declares with a new request ID after reconnect without replaying edits.
Sparse scalar patches validate all targets/duplicates/bounds before mutation and
preserve untouched clip, pattern, effect and automation content. Hosts without
negotiation continue using complete track/snapshot messages.


## Connection responsiveness

Ordinary frame JSON/model validation, capabilities and ordered snapshot assembly,
SHA-256 verification and full document decoding run on one FIFO worker. `poll`
keeps transport ownership on the calling control worker, reads the connection on
every call, and processes strict heartbeats and already-correlated receipt
envelopes of at most 4 KiB without waiting for ordinary decoding. Unknown or
larger replies remain FIFO ordinary work. Receipts affect request correlation,
 not installed authored state; state notices retain ordinary wire order.

`poll(Duration::ZERO)` only advances available work; submitting a frame does not
imply its model is already installed. Use connection state, notices and request
IDs rather than counting calls. Poll regularly even while syncing. Resetting a
connection invalidates queued and in-progress results with a checked local epoch,
clears private transfer overlays, and never replays requests. Retained assembly
is released by the worker on its next input or idle check (at most 250 ms).

Raw ordinary admission is at most 32 MiB and 2,048 frames, including a frame being
processed. Decoded outputs are at most 128 entries, weighted by source JSON bytes
with a 64 MiB bound; completed page transfers are charged by the full assembled
body, not their tiny End frame. Assembly itself remains at most 64 MiB. Bounds
are separate, not a claim about total heap: JSON trees, typed records, the old
installed snapshot, and transient conversion allocations also require memory
qualification. Queue overflow or invalid input disconnects rather than silently
losing state. Reservations remain charged until their actual queued objects drop.

State installation, deferred-delta application and retirement of the previous
large model still happen on the control owner. Their optimized latency and total
peak memory remain explicit performance gates. The pure dense test blocks End
verification for a 100,000-point paged document and verifies heartbeat replies,
exact Stop receipt correlation, FIFO state installation and reconnect isolation;
actual mixed-client WSS performance is qualified separately on the real endpoint.
