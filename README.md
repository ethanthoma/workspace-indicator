# workspace-indicator

An always-visible Hyprland workspace indicator, centered over Waybar's top row.
The active dot stretches into a capsule, slides to the next workspace, and
contracts back into a dot. Rapid switches retarget the current capsule without
resetting its position. The leading edge uses cubic ease-out and the trailing
edge uses cubic ease-in, following the motion in
[ZanzyTHEbar/workspace-indicator](https://github.com/ZanzyTHEbar/workspace-indicator).

The indicator is a separate click-through Wayland layer surface, not a Waybar
module. It uses Smithay's client toolkit and shared-memory rendering. Wayland
frame callbacks pace animation at the compositor's refresh rate. At rest it
blocks on events without a redraw timer. It never takes keyboard focus, changes
keybindings, or reserves additional desktop space.

## Build and run

```console
nix build
./result/bin/workspace-indicator config.json
```

The one argument is a JSON configuration file:

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

`height` should match Waybar's height. `margin_top` is the distance from the
physical output's top edge in logical pixels; the indicator ignores Waybar's
exclusive zone so both occupy the same row. Only the workspace dots and animated
capsule are drawn; the surrounding surface is transparent. The surface follows
the focused monitor.

Slots are fixed, numbered 1 through `slots` (1–32). Occupancy is global across
monitors, matching Hyprland's workspace IDs. Special/named workspaces are not
represented. Workspace IDs outside the configured range have no active marker.
The first state read immediately shows the current workspace without animation.

Colors come from the desktop palette: text for active, subtle for occupied,
muted for empty, and red only for urgent. Urgency is tracked from Hyprland's
`urgent` events and cleared when the workspace becomes active or empty. Urgency
that predates process startup is not reconstructed.

Integer output scaling from 1× through 4× is supported; fractional scales use
compositor resampling. IPC responses, event lines, the update queue, and render
buffers are bounded. Socket failures terminate the process; systemd owns
restarts. There is no config reload, theme discovery, installer, CLI framework,
or alternate compositor support. Restart after changing configuration.

## Desktop setup

`/etc/nixos` pins this Git repository as the `workspace-indicator` flake input.
Its Home Manager module supplies palette colors, shares Waybar's configured
height, and runs a user service tied to `hyprland-session.target`. The native
Waybar workspace module is disabled to avoid duplicate markers. The Hyprland
layer rule disables compositor animations for this surface; the renderer owns
the capsule animation.

After committing changes here, update the pin and rebuild:

```console
cd /etc/nixos
nix flake update workspace-indicator
sudo nixos-rebuild switch --flake .#desktop
```

The local input requires this checkout at its current path. Use a remote Git URL
when deploying to another machine. If activation does not start the service in
an already-running session, start it explicitly:

```console
systemctl --user start workspace-indicator.service
```

## Verification

```console
nix develop -c cargo fmt --check
nix develop -c cargo test --release
nix develop -c cargo clippy --all-targets -- -D warnings
```

Tests explore every source/target pair across 32 slots, both directions,
convergence, and deterministic rapid reversals. Pixel tests check the moving
capsule and premultiplied alpha across all supported slot counts and scales.

Run the opt-in integration test inside a Hyprland session:

```console
nix develop -c cargo test --release --test wayland -- --ignored
```

It uses real Wayland with simulated Hyprland IPC to exercise startup, frame
callbacks, buffer reuse, rapid switches, reversal, urgency, and remaining alive
after the animation ends. It does not change the user's actual workspaces.
Placement and visual timing still need an on-screen check.
