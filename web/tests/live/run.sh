#!/bin/sh
# Live browser-module acceptance against the real portable service (simulated
# callback, no audio device). Uses the existing private TLS fixture; does not
# change system trust: Node is told about the fixture CA for this run only.
set -eu
repo=$(cd "$(dirname "$0")/../../.." && pwd)
fixture=${SOUNDROOM_TLS_FIXTURE:-$repo/build/CrossPlatformBaseline/web-tls}
CARGO_INCREMENTAL=0 CARGO_HOME=${CARGO_HOME:-/private/tmp/soundroom-cargo} cargo build --offline --manifest-path "$repo/portable/Cargo.toml" -p soundroom-server --example write_acceptance_host
(cd "$repo/web" && tsc -p tsconfig.json)
SOUNDROOM_LIVE_WRITE_HOST="$repo/portable/target/debug/examples/write_acceptance_host" \
SOUNDROOM_LIVE_WRITE_PROJECT="$repo/portable/fixtures/modern-gain.soundroom" \
SOUNDROOM_TEST_TLS_P12="$fixture/server.p12" \
SOUNDROOM_TEST_TLS_PASSWORD="$(cat "$fixture/identity.password")" \
NODE_EXTRA_CA_CERTS="$fixture/ca.crt" \
node --test --test-force-exit --test-timeout=120000 "$repo/web/tests/live/live-write.test.mjs"
