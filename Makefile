.PHONY: build test benchmark install-local reload reload-plugin reload-config

BINARY = herdr-mru-cycle
CARGO_TARGET = target/release/$(BINARY)
BIN_TARGET = bin/$(BINARY)
BENCHMARK = target/release/benchmark

build:
	cargo build --release
	mkdir -p bin
	cp $(CARGO_TARGET) $(BIN_TARGET)
	chmod +x $(BIN_TARGET)

test: build
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings
	cargo test
	cargo build --release --bin benchmark
	@set -e; \
	TMP=$$(mktemp -d); \
	trap 'rm -rf "$$TMP"' EXIT; \
	$(BENCHMARK) --sizes 5 50 500 --iterations 1 --warmup 0 --output-dir "$$TMP" >/dev/null; \
	test -f "$$TMP/report.md"; \
	test -f "$$TMP/samples.json"; \
	test -f "$$TMP/samples.csv"; \
	awk '/\| 5 \| cycle \| Rust / {found=1} END {exit !found}' "$$TMP/report.md"; \
	awk '/\| 50 \| focus-attention \| Rust / {found=1} END {exit !found}' "$$TMP/report.md"; \
	awk '/\| 500 \| pane.closed \| Rust / {found=1} END {exit !found}' "$$TMP/report.md"

benchmark: build
	cargo build --release --bin benchmark
	$(BENCHMARK)

install-local: build

reload: reload-plugin reload-config

reload-plugin: install-local
	herdr plugin unlink herdr.pane-switcher || true
	herdr plugin link $(CURDIR)

reload-config:
	herdr server reload-config
