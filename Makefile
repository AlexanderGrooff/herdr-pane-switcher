.PHONY: reload reload-plugin reload-config

reload: reload-plugin reload-config

reload-plugin:
	herdr plugin unlink herdr.pane-switcher || true
	herdr plugin link $(CURDIR)

reload-config:
	herdr server reload-config
