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

### From the project's Flatpak repository (recommended)

Every push to the default branch publishes an OSTree repository to GitHub
Pages, so ordinary `flatpak update` works. Add the remote once:

```sh
flatpak remote-add --user --no-gpg-verify whitescreen \
    https://sergegris.github.io/whitescreen/whitescreen.flatpakrepo
flatpak install --user whitescreen io.github.SergeGris.WhiteScreen
```

From then on:

```sh
flatpak update
```

`--no-gpg-verify` is needed because the repository is unsigned; the download
itself is still over HTTPS. See *Signing the repository* below.

> **Repository owner:** this requires **Settings → Pages → Source = GitHub
> Actions** to be enabled once. Until then the deploy job fails and the URL
> above 404s.

### One-off bundle (no auto-update)

```sh
wget https://github.com/SergeGris/whitescreen/releases/download/continuous/whitescreen.flatpak
flatpak install --user ./whitescreen.flatpak
```

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

## Signing the repository

The published repository is unsigned, which is why users pass
`--no-gpg-verify`. To sign it:

1. Generate a key: `gpg --quick-gen-key "White Screen CI" default default never`
2. Export the private key and store it as the repository secret
   `FLATPAK_GPG_KEY`: `gpg --export-secret-keys --armor <key-id> | base64 -w0`
3. In `.github/workflows/flatpak.yml`, import the key and add
   `--gpg-sign=<key-id>` to both `flatpak-builder` and
   `flatpak build-update-repo`, then add the base64 of the *public* key as a
   `GPGKey=` line in the generated `.flatpakrepo`.

Users can then add the remote without `--no-gpg-verify`.

For a distribution channel that needs none of this infrastructure, submit the
app to [Flathub](https://github.com/flathub/flathub/blob/master/README.md) —
`build-aux/io.github.SergeGris.WhiteScreen.json` is the manifest to submit.

## Before publishing

These items are placeholders and must be checked against reality before a
release or a Flathub submission:

- **Screenshot** — `data/io.github.SergeGris.WhiteScreen.metainfo.xml` points at
  `data/screenshots/main-window.png`, which does not exist yet. Flathub review
  rejects placeholder screenshots.
- **gtk4-layer-shell commit hash** — `build-aux/io.github.SergeGris.WhiteScreen.json`
  pins tag `v1.3.0` (confirmed to be the current release). Flathub additionally
  requires a fixed `commit` for git sources; the commit for that tag begins
  `1c963c5`, so add the full 40-character hash before submitting. (CI does not
  use this pin — it resolves the newest release tag at build time.)
- **Icons** — `data/icons/` contains hand-written SVGs that have not been
  reviewed at their rendered sizes (128px and 16px).

## License

GPL-3.0-or-later. See [COPYING](COPYING).
