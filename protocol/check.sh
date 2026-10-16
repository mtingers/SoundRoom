#!/usr/bin/env bash
set -euo pipefail
contract_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$contract_root"
python3 protocol/generate.py --check
python3 protocol/test_generator.py
contract_test_dir="$(mktemp -d "${TMPDIR:-/tmp}/soundroom-contract.XXXXXX")"
trap 'rm -rf "$contract_test_dir"' EXIT
tsc --target ES2022 --module commonjs --strict --outDir "$contract_test_dir/ts" protocol/validate.ts
node protocol/test.cjs "$contract_test_dir/ts/validate.js"
rustc --crate-type lib --out-dir "$contract_test_dir" protocol/generated/contract.rs
