# workspace-indicator

A Hyprland-only workspace OSD in Rust. Shows a click-through pill on the focused
monitor when the active workspace or focused monitor changes, then fades away.
It does not modify Hyprland configuration or keybindings.

Based on [ZanzyTHEbar/workspace-indicator](https://github.com/ZanzyTHEbar/workspace-indicator).

Uses Smithay's client toolkit for Wayland layer-shell and shared-memory buffers.
The Hyprland event socket drives updates; there is no polling while hidden.
Keyboard input is disabled and the input region is empty. The overlay reserves
no desktop space. IPC responses, event lines, the update queue, and render
buffer count are bounded. Socket errors terminate the process; systemd owns
restarts.

## Build and run

```console
nix build
./result/bin/workspace-indicator config.json
```

The one argument is a JSON configuration file:

```json
{
    "slots": 5,
    "margin": 60,
    "display_ms": 900,
    "fade_ms": 200,
    "background": "#0f111a",
    "active": "#875afb",
    "occupied": "#e0e0e0",
    "empty": "#4b526d"
}
```

Slots are fixed, numbered 1 through `slots` (1–32). Occupancy is global across
monitors, matching Hyprland's global workspace IDs. Special/named workspaces are
not represented. Workspace IDs outside the configured range have no active dot.
The first state read on startup does not reveal the overlay.

The margin and geometry use logical pixels. Integer output scaling up to 4× is
supported; fractional scales use compositor resampling. There is deliberately no
config reload, theme discovery, installer, CLI framework, or alternate
compositor support. Restart the service after changing configuration.

## Desktop setup

This standalone prototype is not enabled in `/etc/nixos`. The desktop uses
Waybar's native `hyprland/workspaces` module instead, configured in
`/etc/nixos/modules/home-manager/waybar.nix`, with always-visible markers at the
top center. No separate OSD process or service is needed.

## Verification

```console
nix develop -c cargo fmt --check
nix develop -c cargo test
nix develop -c cargo clippy --all-targets -- -D warnings
```

Tests explore fade monotonicity and expiration and premultiplied-alpha rendering
across all supported slot counts and integer scales. A live compositor is still
needed to check placement, input pass-through, and monitor switching visually.
