# Pane Switcher (herdr-pane-switcher)

A [Herdr](https://herdr.dev) plugin that cycles panes in most-recently-used order across all workspaces and jumps to panes that need attention, in priority order: `blocked`, `done`, `idle`, `working`.

## Requirements

- Herdr >= 0.7.5
- macOS or Linux (uses Unix-domain sockets and file locking)
- A Rust toolchain is required to build the plugin and run its test/benchmark harnesses

## How it works

The plugin is a single Rust binary (`bin/herdr-mru-cycle`) that is executed by Herdr for each action and event.

- On every `pane.focused`/`pane.closed` event, the plugin updates the MRU history stored in `HERDR_PLUGIN_STATE_DIR`.
- `herdr.pane-switcher.cycle` focuses the next pane in MRU order.
- `herdr.pane-switcher.focus-attention` focuses the first pane by `agent_status` priority: `blocked`, then `done`, then `idle`, then `working`.
- `herdr.pane-switcher.cycle-attention` cycles to the next pane in that same priority order after the current one, wrapping to the first.
- Repeated `cycle` invocations within the 1-second timeout continue cycling through the same MRU order instead of restarting.

State is stored in `HERDR_PLUGIN_STATE_DIR/state.bin` with advisory file locking so multiple concurrent invocations are safe.

## Loading the plugin

The plugin is loaded through Herdr's plugin registry. After loading, add a keybinding (see [Binding a key](#binding-a-key)) or run it manually.

### As a regular user from a local clone

```bash
git clone <repo-url> /path/to/herdr-pane-switcher
cd /path/to/herdr-pane-switcher
make build
herdr plugin link /path/to/herdr-pane-switcher
herdr plugin list
herdr plugin action list --plugin herdr.pane-switcher
```

### As a regular user from GitHub (if published)

```bash
herdr plugin install <owner>/herdr-pane-switcher
herdr plugin action list --plugin herdr.pane-switcher
```

For non-interactive installation, add `--yes`.

## Binding a key

Add entries to `~/.config/herdr/config.toml`:

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

Then reload Herdr's config or restart Herdr.

You can also invoke the actions manually:

```bash
herdr plugin action invoke herdr.pane-switcher.cycle
herdr plugin action invoke herdr.pane-switcher.focus-attention
herdr plugin action invoke herdr.pane-switcher.cycle-attention
```

## Development

The Makefile runs the Rust toolchain, integration tests, and a micro-benchmark:

```bash
make build       # cargo build --release + copy binary to bin/
make test        # fmt, clippy, cargo test, integration tests, benchmark sanity
```

Link the built plugin once:

```bash
herdr plugin link /path/to/herdr-pane-switcher
herdr plugin list
herdr plugin action list --plugin herdr.pane-switcher
```

Then edit `src/main.rs` and re-run `make build` (or `make install-local`) to update the binary. The Rust binary is executed fresh for each action/event, so changes take effect immediately after rebuilding.

For manifest changes, unlink and re-link the plugin, or restart Herdr:

```bash
herdr plugin unlink herdr.pane-switcher
make install-local
herdr plugin link /path/to/herdr-pane-switcher
herdr server reload-config
```

Or use the convenience target:

```bash
make reload
```

Inspect logs:

```bash
herdr plugin log list --plugin herdr.pane-switcher
```

To remove the local link without deleting files:

```bash
herdr plugin unlink herdr.pane-switcher
```

### Benchmarking

Run the Rust benchmark harness:

```bash
make benchmark
```

The harness reports fixture/action progress to stderr and bounds each plugin
process to 15 seconds, so a stalled socket cannot make the run appear silent or
hang indefinitely. The default run is intentionally comprehensive; use
`target/release/benchmark --iterations 10 --warmup 1` for a quicker local check.

This writes `benchmarks/output/report.md`, `benchmarks/output/samples.json`, and `benchmarks/output/samples.csv`.

## File layout

- `herdr-plugin.toml` — plugin manifest
- `src/main.rs` — Rust implementation of the plugin binary
- `src/bin/benchmark.rs` — Rust-only benchmark harness
- `src/test_support.rs` — shared mock server, fixtures, and plugin runner for Rust tests and benchmarks
- `tests/plugin.rs` — Rust integration tests for plugin behavior
- `tests/manifest.rs` — static checks for plugin manifest and repository layout
