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

### With Meson (installs the desktop entry, icons, and metadata)

```sh
meson setup _build --prefix=/usr/local
meson compile -C _build
meson install -C _build
```

Options:

| Option | Default | Description |
| ------ | ------- | ----------- |
| `-Dgamma=true` | `false` | Watch `zwlr-gamma-control-v1` and show when another client (wlsunset, gammastep, wl-gammarelay) holds a color filter |

### From the project's Flatpak repository (recommended — supports `flatpak update`)

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

The Flatpak build targets the `org.gnome.Platform` runtime pinned in
`build-aux/io.github.SergeGris.WhiteScreen.json`. GNOME runtimes go end-of-life
about a year after release, so that pin needs bumping periodically; CI prints
the runtime's status on every build and fails if the branch is gone.

> **Repository owner:** this requires **Settings → Pages → Source = GitHub
> Actions** to be enabled once. Until then the deploy job fails and the URL
> above 404s.

### Arch Linux / CachyOS

A PKGBUILD lives in `packaging/arch/`. It builds the default branch, so no AUR
account or published release is needed:

```sh
curl -O https://raw.githubusercontent.com/SergeGris/whitescreen/master/packaging/arch/PKGBUILD
makepkg -si
```

Or from a clone:

```sh
git clone https://github.com/SergeGris/whitescreen.git
cd whitescreen/packaging/arch
makepkg -si
```

`makepkg` clones the repository itself, so it always builds current `master`
regardless of the checkout you launched it from. Re-run it to update.

Once the package is published to the AUR it installs the usual way:

```sh
yay -S whitescreen-git
```

To publish it: clone `ssh://aur@aur.archlinux.org/whitescreen-git.git`, copy in
`PKGBUILD`, regenerate the metadata with `makepkg --printsrcinfo > .SRCINFO`,
then commit and push both files.

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

## Troubleshooting

| Variable | Effect |
| -------- | ------ |
| `LD_PRELOAD=/usr/lib/libgtk4-layer-shell.so` | Last-resort workaround if the app reports *"Compositor not supported"* on a compositor that does support `wlr-layer-shell` (niri, Sway, Hyprland, KDE). gtk4-layer-shell interposes on `libwayland-client` and has to be loaded first. This binary links it directly, so the normal load order already satisfies that — needing this points at an unusual loader setup. Adjust the path to wherever your distribution installs the library. |
| `WHITESCREEN_NO_GAMMA=1` | Do not start the gamma-control monitor. The indicator stays at "inactive". Use this to rule the background Wayland prober in or out when diagnosing a crash or hang. |

Under Flatpak:

```sh
flatpak run --env=WHITESCREEN_NO_GAMMA=1 io.github.SergeGris.WhiteScreen
```

To get a symbolized backtrace, install the debug extension and run under gdb in
the SDK:

```sh
flatpak install --user whitescreen io.github.SergeGris.WhiteScreen.Debug
flatpak run --devel --command=gdb io.github.SergeGris.WhiteScreen \
    -ex run -ex "thread apply all bt full"
```

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

- **Screenshot** — the metainfo has no `<screenshots>` block. Flathub requires
  at least one. Add a real PNG under `data/screenshots/`, commit it, and
  reference its `raw.githubusercontent.com` URL on the **master** branch (the
  earlier placeholder pointed at `main`, which does not exist here).
- **gtk4-layer-shell commit hash** — `build-aux/io.github.SergeGris.WhiteScreen.json`
  pins tag `v1.3.0` (confirmed to be the current release). Flathub additionally
  requires a fixed `commit` for git sources; the commit for that tag begins
  `1c963c5`, so add the full 40-character hash before submitting. (CI does not
  use this pin — it resolves the newest release tag at build time.)
- **Symbolic icon** — `data/icons/hicolor/symbolic/` is a hand-written SVG that
  has not been reviewed at its rendered size (16px). The 128x128 PNG and the
  scalable SVG share the same design.

## License

GPL-3.0-or-later. See [COPYING](COPYING).
