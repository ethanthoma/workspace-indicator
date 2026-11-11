# workspace-indicator

A workspace indicator for Hyprland. The active marker stretches and slides
between workspace dots. It stays visible on the focused monitor and lets clicks
pass through to the bar underneath.

Based on
[ZanzyTHEbar/workspace-indicator](https://github.com/ZanzyTHEbar/workspace-indicator).
This version is written in Rust and runs as a separate Wayland layer surface,
not a Waybar module.

## Run

Inside a Hyprland session:

```console
nix build
./result/bin/workspace-indicator config.json
```

## Configuration

Pass a JSON file as the only argument:

```json
{
  "slots": 5,
  "margin_top": 0,
  "height": 30,
  "animation_ms": 320,
  "active": "#e0e0e0",
  "occupied": "#8f93a2",
  "empty": "#4b526d",
  "urgent": "#f2283c"
}
```

- `slots`: workspace IDs to display, from 1 through this number (1–32).
- `margin_top`: offset from the output's top edge, in logical pixels (0–1000).
- `height`: surface height in logical pixels (24–64). Match your bar's height to
  place the dots over it.
- `animation_ms`: transition duration in milliseconds (1–2000).
- `active`, `occupied`, `empty`, `urgent`: colors in `#RRGGBB` format.

The background is transparent. The surface ignores the bar's reserved space so
it can occupy the same row. Disable the bar's own workspace markers to avoid
duplicates. Restart the indicator after changing its configuration.

Only numbered workspaces in the configured range are shown. Occupancy is shared
across monitors. Urgency is tracked from events received after startup and
clears when a workspace becomes active or empty.

Integer output scales from 1× to 4× are supported; fractional scaling uses
compositor resampling. Socket errors stop the process. Use a service manager if
you want automatic restarts.

## Development

```console
nix develop -c cargo fmt --check
nix develop -c cargo test --release
nix develop -c cargo clippy --all-targets -- -D warnings
```

Tests check animation bounds, convergence, rapid reversals, and rendered pixels
across supported slot counts and scales. The integration test requires a running
Hyprland session:

```console
nix develop -c cargo test --release --test wayland -- --ignored
```

It uses real Wayland with simulated Hyprland IPC and does not switch your
workspaces. Check placement and animation timing on screen.
