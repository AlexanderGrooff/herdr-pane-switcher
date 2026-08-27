# Pane Switcher

A [Herdr](https://herdr.dev) plugin for switching panes across all workspaces:

- **Cycle MRU panes** — return to the pane you used most recently, then keep moving backward through pane history.
- **Focus attention** — jump to the highest-priority agent pane.
- **Cycle attention** — move through agent panes by priority.

## Requirements

- Herdr 0.7.5 or newer
- macOS or Linux on x86-64 or ARM64

Installing a release does not require Rust.

## Install

```bash
herdr plugin install AlexanderGrooff/herdr-pane-switcher
herdr plugin action list --plugin herdr.pane-switcher
```

The first command downloads the prebuilt binary for your platform and verifies its checksum. Herdr may ask for confirmation; add `--yes` for a non-interactive install. The second command should list:

- `herdr.pane-switcher.cycle`
- `herdr.pane-switcher.focus-attention`
- `herdr.pane-switcher.cycle-attention`

## Add keybindings

Add the following to `~/.config/herdr/config.toml`:

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

Apply the change:

```bash
herdr server reload-config
```

Prefix bindings are recommended because terminals or macOS often intercept `ctrl+tab`. If you prefer direct bindings, use Herdr's safe `ctrl+alt` modifier family, for example `ctrl+alt+tab`.

## What to expect

### Cycle MRU panes

After installation, the plugin silently records pane focus and close events. Press `prefix+tab` once to switch to the previously focused pane, even if it is in another workspace. Press it again within one second to move to the next pane in the same captured MRU order. Cycling wraps at the end.

History builds as you use Herdr; panes not yet seen by the plugin follow the order returned by Herdr. After a one-second pause, the next press starts a new cycle from the current history. With fewer than two panes, nothing moves.

### Focus or cycle attention panes

`prefix+shift+tab` selects the first pane in this priority order:

1. `blocked`
2. `done`
3. `idle`
4. `working`

`prefix+a` selects the next pane in that order and wraps to the first. Panes without one of these statuses are skipped. If no pane has a recognized status, nothing moves.

You can test the actions without keybindings:

```bash
herdr plugin action invoke herdr.pane-switcher.cycle
herdr plugin action invoke herdr.pane-switcher.focus-attention
herdr plugin action invoke herdr.pane-switcher.cycle-attention
```

## Troubleshooting

Confirm that Herdr loaded the plugin and inspect its logs:

```bash
herdr plugin list --plugin herdr.pane-switcher
herdr plugin action list --plugin herdr.pane-switcher
herdr plugin log list --plugin herdr.pane-switcher
```

If an action works when invoked manually but not from the keyboard, the terminal or operating system is probably consuming the key combination. Use the prefix bindings above or a `ctrl+alt` binding.

## Build from source

Building requires Rust, Cargo, and `make`:

```bash
git clone https://github.com/AlexanderGrooff/herdr-pane-switcher.git
cd herdr-pane-switcher
make build
herdr plugin link "$PWD"
```

Re-run `make build` after Rust changes. Run `make reload` after manifest changes. To verify the project or run its benchmark:

```bash
make test
make benchmark
```

Remove a local link without deleting the checkout:

```bash
herdr plugin unlink herdr.pane-switcher
```
