# MRU Panes (herdr-mru-cycle)

A [Herdr](https://herdr.dev) plugin that cycles panes in most-recently-used order across all workspaces and jumps to panes that need attention (`blocked` or `done`).

## Requirements

- Herdr >= 0.7.0
- Python 3
- macOS or Linux (uses `fcntl` and Unix-domain sockets)

## How it works

The plugin registers three actions and event hooks on `pane.focused` and `pane.closed`.

- On every focus/close event, the plugin updates the MRU history stored in `HERDR_PLUGIN_STATE_DIR`.
- `alex.mru-tabs.cycle` focuses the next pane in MRU order.
- `alex.mru-tabs.focus-attention` focuses the first pane whose `agent_status` is `blocked` or `done`, preferring `blocked`.
- `alex.mru-tabs.cycle-attention` cycles to the next `blocked`/`done` pane after the current one, wrapping to the first.
- Repeated `cycle` invocations within `CYCLE_TIMEOUT_SECONDS` (1 second) continue cycling through the same MRU order instead of restarting.

## Loading the plugin

The plugin is loaded through Herdr's plugin registry. After loading, add a keybinding (see [Binding a key](#binding-a-key)) or run it manually.

### As a regular user from a local clone

```bash
git clone <repo-url> /path/to/herdr-mru-cycle
herdr plugin link /path/to/herdr-mru-cycle
herdr plugin list
herdr plugin action list --plugin alex.mru-tabs
```

### As a regular user from GitHub (if published)

```bash
herdr plugin install <owner>/herdr-mru-cycle
herdr plugin action list --plugin alex.mru-tabs
```

For non-interactive installation, add `--yes`.

## Binding a key

Add entries to `~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "prefix+tab"
type = "plugin_action"
command = "alex.mru-tabs.cycle"
description = "cycle MRU panes"

[[keys.command]]
key = "prefix+shift+tab"
type = "plugin_action"
command = "alex.mru-tabs.focus-attention"
description = "focus first attention pane"

[[keys.command]]
key = "prefix+ctrl+tab"
type = "plugin_action"
command = "alex.mru-tabs.cycle-attention"
description = "cycle attention panes"
```

Then reload Herdr's config or restart Herdr.

You can also invoke the actions manually:

```bash
herdr plugin action invoke alex.mru-tabs.cycle
herdr plugin action invoke alex.mru-tabs.focus-attention
herdr plugin action invoke alex.mru-tabs.cycle-attention
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
   herdr plugin action invoke alex.mru-tabs.cycle
   ```

5. Inspect logs:

   ```bash
   herdr plugin log list --plugin alex.mru-tabs
   ```

6. To remove the local link without deleting files:

   ```bash
   herdr plugin unlink alex.mru-tabs
   ```

## File layout

- `herdr-plugin.toml` — plugin manifest
- `mru_tabs.py` — executable Python script that talks to Herdr over its Unix socket
