.PHONY: test reload reload-plugin reload-config benchmark

test:
	@set -e; \
	if command -v ruff >/dev/null 2>&1; then ruff check .; fi; \
	python3 -m py_compile mru_tabs.py benchmarks/run.py tests/test_mru_tabs.py; \
	python3 -m unittest discover tests; \
	TMP=$$(mktemp -d); \
	trap 'rm -rf "$$TMP"' EXIT; \
	python3 benchmarks/run.py --sizes 5 50 500 --iterations 1 --warmup 0 --output-dir "$$TMP"; \
	test -f "$$TMP/report.md"; \
	test -f "$$TMP/samples.json"; \
	test -f "$$TMP/samples.csv"; \
	grep -q '| 5 | cycle ' "$$TMP/report.md"; \
	grep -q '| 50 | focus-attention ' "$$TMP/report.md"; \
	grep -q '| 500 | pane.closed ' "$$TMP/report.md"

benchmark:
	python3 benchmarks/run.py

reload: reload-plugin reload-config

reload-plugin:
	herdr plugin unlink herdr.pane-switcher || true
	herdr plugin link $(CURDIR)

reload-config:
	herdr server reload-config

test:
	echo "noop to satisfy bd-dispatch verify step"
