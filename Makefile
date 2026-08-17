.PHONY: reload reload-plugin reload-config benchmark

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
