# White Screen

Fill any monitor with a solid color — for camera tests, lighting, and backlight checks.

White Screen puts a flat, uniform color across one or more of your displays. No
title bar, no shadows, no wallpaper bleeding through, and no cursor in the frame —
things an ordinary maximized window can't give you.

## Uses

- Checking a panel for dead or stuck pixels
- Inspecting LCD backlight bleed and clouding on a black or white field
- Using a display as a soft key light for a webcam or camera
- Eyeballing color uniformity and tint across several monitors

## Requirements

White Screen is a Wayland application. It needs a compositor implementing
**`wlr-layer-shell`**:

| Compositor | Supported |
| ---------- | --------- |
| Niri, Sway, Hyprland, Wayfire, river | yes |
| KDE Plasma 6 | yes |
| GNOME / Mutter | **no** — Mutter does not implement `wlr-layer-shell` |
| X11 | **no** |

Build-time dependencies:

- Rust ≥ 1.92
- GTK ≥ 4.14
- libadwaita ≥ 1.5
- gtk4-layer-shell ≥ 1.0
- Meson ≥ 0.62 (for the install path)

## Building

### With Meson (recommended — installs the desktop entry, icons, and metadata)

```sh
meson setup _build --prefix=/usr/local
meson compile -C _build
meson install -C _build
```

Options:

| Option | Default | Description |
| ------ | ------- | ----------- |
| `-Dgamma=true` | `false` | Watch `zwlr-gamma-control-v1` and show when another client (wlsunset, gammastep, wl-gammarelay) holds a color filter |

### With cargo only (binary, no desktop integration)

```sh
cargo build --release
./target/release/whitescreen
```

## Usage

1. Tick the monitors you want to cover in the sidebar.
2. Pick a color — a preset, or **Custom** for the full picker.
3. Press **Show on selected**.
4. Press <kbd>Esc</kbd> on an overlay to dismiss it, or **Hide ALL**.

**Identify** flashes each monitor's connector name (`DP-1`, `HDMI-A-2`, …) on the
screen itself, so you always know which physical panel you're looking at.

## Before publishing

These items are placeholders and must be checked against reality before a
release or a Flathub submission:

- **Screenshot** — `data/io.github.SergeGris.WhiteScreen.metainfo.xml` points at
  `data/screenshots/main-window.png`, which does not exist yet. Flathub review
  rejects placeholder screenshots.
- **gtk4-layer-shell pin** — `build-aux/io.github.SergeGris.WhiteScreen.json`
  pins tag `v1.1.1`. Verify that tag exists and add the matching `commit` hash;
  Flathub requires a fixed commit for git sources. (CI does not use this pin —
  it resolves the newest release tag at build time.)
- **Icons** — `data/icons/` contains hand-written SVGs that have not been
  reviewed at their rendered sizes (128px and 16px).

## License

GPL-3.0-or-later. See [COPYING](COPYING).
