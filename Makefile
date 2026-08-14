.PHONY: reload reload-plugin reload-config

reload: reload-plugin reload-config

reload-plugin:
	herdr plugin unlink herdr.mru-panes || true
	herdr plugin link $(CURDIR)

reload-config:
	herdr server reload-config
