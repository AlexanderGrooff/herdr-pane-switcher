# Agent notes for herdr-pane-switcher

This is a Herdr plugin that cycles panes in most-recently-used order and jumps to panes that need attention (`blocked` or `done`).

## Files

- `herdr-plugin.toml` — manifest declaring plugin id `herdr.pane-switcher`, the `cycle`, `focus-attention`, and `cycle-attention` actions, plus `pane.focused`/`pane.closed` event hooks.
- `pane_switcher.py` — executable Python 3 script, run by Herdr, not directly by users. It uses only the standard library (`fcntl`, `json`, `os`, `socket`, `time`, `pathlib`).

## Runtime

- Herdr injects `HERDR_SOCKET_PATH`, `HERDR_BIN_PATH`, `HERDR_PLUGIN_STATE_DIR`, and event context at runtime. Action commands also receive `HERDR_PLUGIN_ACTION_ID`.
- The script makes raw JSON socket requests to `pane.list` and `pane.focus`.
- State is stored in `HERDR_PLUGIN_STATE_DIR/state.json` with `fcntl` file locking.
- `CYCLE_TIMEOUT_SECONDS = 1.0` determines whether repeated `cycle` invocations continue through the same MRU order.
- `focus-attention` jumps to the first pane by `agent_status` priority: `blocked`, then `done`, then `idle`, then `working`.
- `cycle-attention` jumps to the next pane in that same priority order after the current one, wrapping to the first.

## Loading the plugin

```bash
herdr plugin link /absolute/path/to/herdr-pane-switcher
herdr plugin list --plugin herdr.pane-switcher
herdr plugin action list --plugin herdr.pane-switcher
```

For end users installing from a published GitHub repo:

```bash
herdr plugin install <owner>/herdr-pane-switcher
```

## Development workflow

1. Link the repo with `herdr plugin link /absolute/path/to/herdr-pane-switcher`.
2. Edit `pane_switcher.py` and test immediately with `herdr plugin action invoke herdr.pane-switcher.cycle`.
3. For manifest changes, unlink and re-link, or restart Herdr.
4. Watch plugin logs with `herdr plugin log list --plugin herdr.pane-switcher`.

When keybinding the actions for a regular user, add them to `~/.config/herdr/config.toml`.
Prefix-mode is the safest default, because `ctrl+tab`/`ctrl+shift+tab` are
commonly owned by macOS, Ghostty, or other terminals and may never reach Herdr.

```toml
[[keys.command]]
key = "prefix+tab"
type = "plugin_action"
command = "herdr.pane-switcher.cycle"
description = "cycle MRU panes"

[[keys.command]]
key = "prefix+shift+tab"
type = "plugin_action"
command = "herdr.pane-switcher.focus-attention"
description = "focus first attention pane"

[[keys.command]]
key = "prefix+a"
type = "plugin_action"
command = "herdr.pane-switcher.cycle-attention"
description = "cycle attention panes"
```

If you prefer direct chords with no prefix, Herdr's documented safe modifier
family is `ctrl+alt` (Control+Option on a Mac). On macOS with Ghostty and
`macos-option-as-alt = true`, plain `alt` works as the same modifier and can be
used for single-modifier chords.

```toml
[[keys.command]]
key = "ctrl+alt+tab"
type = "plugin_action"
command = "herdr.pane-switcher.cycle"
description = "cycle MRU panes"

[[keys.command]]
key = "ctrl+alt+a"
type = "plugin_action"
command = "herdr.pane-switcher.focus-attention"
description = "focus first attention pane"

[[keys.command]]
key = "ctrl+alt+c"
type = "plugin_action"
command = "herdr.pane-switcher.cycle-attention"
description = "cycle attention panes"
```

## Constraints

- `min_herdr_version = "0.7.0"`
- `platforms = ["macos", "linux"]` only, because the script uses `fcntl` and Unix-domain sockets.
