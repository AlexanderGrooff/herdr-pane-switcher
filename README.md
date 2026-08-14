# MRU Panes (herdr-mru-cycle)

A [Herdr](https://herdr.dev) plugin that cycles panes in most-recently-used order across all workspaces and jumps to panes that need attention, in priority order: `blocked`, `done`, `idle`, `working`.

## Requirements

- Herdr >= 0.7.0
- Python 3
- macOS or Linux (uses `fcntl` and Unix-domain sockets)

## How it works

The plugin registers three actions and event hooks on `pane.focused` and `pane.closed`.

- On every focus/close event, the plugin updates the MRU history stored in `HERDR_PLUGIN_STATE_DIR`.
- `herdr.mru-panes.cycle` focuses the next pane in MRU order.
- `herdr.mru-panes.focus-attention` focuses the first pane by `agent_status` priority: `blocked`, then `done`, then `idle`, then `working`.
- `herdr.mru-panes.cycle-attention` cycles to the next pane in that same priority order after the current one, wrapping to the first.
- Repeated `cycle` invocations within `CYCLE_TIMEOUT_SECONDS` (1 second) continue cycling through the same MRU order instead of restarting.

## Loading the plugin

The plugin is loaded through Herdr's plugin registry. After loading, add a keybinding (see [Binding a key](#binding-a-key)) or run it manually.

### As a regular user from a local clone

```bash
git clone <repo-url> /path/to/herdr-mru-cycle
herdr plugin link /path/to/herdr-mru-cycle
herdr plugin list
herdr plugin action list --plugin herdr.mru-panes
```

### As a regular user from GitHub (if published)

```bash
herdr plugin install <owner>/herdr-mru-cycle
herdr plugin action list --plugin herdr.mru-panes
```

For non-interactive installation, add `--yes`.

## Binding a key

Add entries to `~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "prefix+tab"
type = "plugin_action"
command = "herdr.mru-panes.cycle"
description = "cycle MRU panes"

[[keys.command]]
key = "prefix+shift+tab"
type = "plugin_action"
command = "herdr.mru-panes.focus-attention"
description = "focus first attention pane"

[[keys.command]]
key = "prefix+a"
type = "plugin_action"
command = "herdr.mru-panes.cycle-attention"
description = "cycle attention panes"
```

Then reload Herdr's config or restart Herdr.

You can also invoke the actions manually:

```bash
herdr plugin action invoke herdr.mru-panes.cycle
herdr plugin action invoke herdr.mru-panes.focus-attention
herdr plugin action invoke herdr.mru-panes.cycle-attention
```

## Development

1. Link the working directory once:

   ```bash
   herdr plugin link /path/to/herdr-mru-cycle
   ```

2. Make sure `mru_tabs.py` is executable:

   ```bash
   chmod +x mru_tabs.py
   ```

3. Edit `mru_tabs.py` or `herdr-plugin.toml`. The script is executed fresh for each action/event, so Python changes take effect immediately. Changes to the manifest usually require restarting Herdr or re-linking the plugin.

4. Test by switching panes (to trigger events) or by invoking the action:

   ```bash
   herdr plugin action invoke herdr.mru-panes.cycle
   ```

5. Inspect logs:

   ```bash
   herdr plugin log list --plugin herdr.mru-panes
   ```

6. To remove the local link without deleting files:

   ```bash
   herdr plugin unlink herdr.mru-panes
   ```

## File layout

- `herdr-plugin.toml` — plugin manifest
- `mru_tabs.py` — executable Python script that talks to Herdr over its Unix socket
