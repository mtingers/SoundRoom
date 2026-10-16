# Browser access

The browser controls the Rust Soundroom server. Audio plays and records on the server computer; the browser does not receive an audio stream.

Build the web client with `npm --prefix web run build`. To expose a server on a trusted LAN, provision a TLS identity for the actual hostname, initialize a private server directory, grant an operator credential, and start the Rust service:

```sh
python3 scripts/provision-web-tls.py --output "$PWD/build/WebIdentity" \
  --host studio.local --host localhost --host 127.0.0.1
portable/target/release/soundroom-server init "$PWD/build/WebServerData" "Studio"
portable/target/release/soundroom-server grant "$PWD/build/WebServerData" \
  "Studio GUI" operator "$PWD/build/studio-operator.json"
SOUNDROOM_TLS_IDENTITY_PASSWORD="$(cat build/WebIdentity/identity.password)" \
SOUNDROOM_WEB_ORIGINS='https://studio.local:8443' \
  portable/target/release/soundroom-server serve "$PWD/build/WebServerData" \
  "$PWD/build/WebIdentity/server.p12" 0.0.0.0:8443 \
  --audio-default --web-assets "$PWD/web/dist"
```

This starts a separate audio host: stop another host before opening the same recording/output devices. The ordinary app's local helper uses its own loopback endpoint. Its obsolete Swift-server and `--web-*` GUI launch options are removed; configure LAN hosting through the Rust executable.

Transfer only `ca.crt` to clients, verify its fingerprint independently, and establish trust using the client operating system's certificate tools. Keep private keys, identity passwords and pairing credentials private. Do not bypass certificate or hostname validation. Open `https://studio.local:8443/`; WSS uses `/control` on the same port, and browser origins must match the configured HTTPS origin exactly.

Connect the macOS GUI to that endpoint with the WSS flags in [server/client design](SERVER_CLIENT_DESIGN.md). An authenticated operator can create a short-lived browser pairing code in Audio server → Browser access, bound to an exact approved name and View/Edit/Operate role. The browser uses that code once to obtain its own credential. Revoking a credential disconnects that client; it does not stop other clients or change the server's audio clock.

The Rust service bounds peers, queues, uploads and downloads; rejects unsafe archive paths and stale resource revisions; and verifies transferred content hashes. See [the Rust service documentation](portable/crates/server/README.md) for commands, limits and qualification details. Physical hardware, browser compatibility and cross-computer latency need validation in the deployment environment.
