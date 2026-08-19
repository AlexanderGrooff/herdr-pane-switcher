.PHONY: build test benchmark install-local reload reload-plugin reload-config

BINARY = herdr-mru-cycle
CARGO_TARGET = target/release/$(BINARY)
BIN_TARGET = bin/$(BINARY)

build:
	cargo build --release
	mkdir -p bin
	cp $(CARGO_TARGET) $(BIN_TARGET)
	chmod +x $(BIN_TARGET)

test: build
	cargo fmt --check
	cargo clippy -- -D warnings
	cargo test
	@set -e; \
	if command -v ruff >/dev/null 2>&1; then ruff check .; fi; \
	python3 -m py_compile benchmarks/baseline.py benchmarks/run.py tests/test_golden.py tests/test_manifest.py; \
	python3 -m unittest discover tests; \
	TMP=$$(mktemp -d); \
	trap 'rm -rf "$$TMP"' EXIT; \
	python3 benchmarks/run.py --impl rust --sizes 5 50 500 --iterations 1 --warmup 0 --output-dir "$$TMP"; \
	test -f "$$TMP/report.md"; \
	test -f "$$TMP/samples.json"; \
	test -f "$$TMP/samples.csv"; \
	grep -q '| 5 | cycle ' "$$TMP/report.md"; \
	grep -q '| 50 | focus-attention ' "$$TMP/report.md"; \
	grep -q '| 500 | pane.closed ' "$$TMP/report.md"

benchmark: build
	python3 benchmarks/run.py

install-local: build

reload: reload-plugin reload-config

reload-plugin: install-local
	herdr plugin unlink herdr.pane-switcher || true
	herdr plugin link $(CURDIR)

reload-config:
	herdr server reload-config
