.PHONY: format fmt format-check lint lint-swift lint-rust check protocol-check install-hooks

PYTHON ?= python3

format fmt:
	$(PYTHON) scripts/style.py format

format-check:
	$(PYTHON) scripts/style.py check

lint-swift:
	$(PYTHON) scripts/style.py check --language swift

lint-rust:
	$(PYTHON) scripts/style.py check --language rust
	cargo clippy --manifest-path portable/Cargo.toml --release --workspace --all-targets --all-features --locked -- -D warnings

lint: lint-swift lint-rust

protocol-check:
	bash protocol/check.sh

check: lint protocol-check

install-hooks:
	$(PYTHON) scripts/style.py install-hook
