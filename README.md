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

White Screen is built around **`wlr-layer-shell`**, which is what lets an
overlay sit above everything else without a window manager getting in the way.
Where that protocol is missing the app still runs, using ordinary fullscreen
windows instead, and says so in a banner:

| Compositor | Overlays | Notes |
| ---------- | -------- | ----- |
| Niri, Sway, Hyprland, Wayfire, river | layer-shell | full support |
| KDE Plasma 6 | layer-shell | full support |
| GNOME / Mutter | fullscreen windows | Mutter does not implement `wlr-layer-shell` |
| X11 | fullscreen windows | |

In fallback mode the color still fills each selected screen, but the overlay is
only above the windows it covers — a notification or an on-screen keyboard can
appear over it — and **Identify** is unavailable, because a badge cannot be
anchored to one monitor's corner without layer-shell. Each overlay is a
separate window there, so only the focused one hears the keyboard; clicking
an overlay dismisses them all.

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
4. Press any key on an overlay to dismiss every one of them, or **Hide ALL**.

Any key dismisses the overlays, not just <kbd>Esc</kbd> — a full-screen color
with no other keyboard function should not be something you can get stuck
behind. Key *combinations* are left alone, so <kbd>Ctrl</kbd>+<kbd>Q</kbd>
still quits from an overlay.

**Custom** shows the color it will apply, and starts on cyan. Clicking it
reopens the picker; selecting it again just re-applies what you last chose.

**Identify** flashes each monitor's connector name (`DP-1`, `HDMI-A-2`, …) on the
screen itself, so you always know which physical panel you're looking at.

**Cycle** steps through white, black, red, green and blue on a timer, from 0.5
to 60 seconds per color. A stuck subpixel only shows up on some of them, so a
dead-pixel check means seeing all five — this does the clicking for you while
you watch the panel. (The floor is 0.5 s on purpose: a full-screen color
flashing faster than that approaches the rate photosensitive-epilepsy guidance
warns about.)

While an overlay is up the session is kept awake, so a screen used as a key
light or left on a test pattern does not blank halfway through. Both routes are
best-effort — `zwp_idle_inhibit_manager_v1` where the compositor has it, and a
session manager or the inhibit portal otherwise.

### Settings

The selected color, the custom color, the ticked monitors and the cycle
interval are remembered in:

```
~/.config/whitescreen/settings.ini
```

It is a plain key file: edit it or delete it, and anything missing falls back
to the defaults. A monitor is remembered by connector name (or by its EDID
strings, for a panel that reports none), so the selection survives unplugging
and replugging a screen and follows a monitor that has moved to another port.

## Troubleshooting

| Variable | Effect |
| -------- | ------ |
| `LD_PRELOAD=/usr/lib/libgtk4-layer-shell.so` | Last-resort workaround if the app falls back to fullscreen windows on a compositor that does support `wlr-layer-shell` (niri, Sway, Hyprland, KDE). gtk4-layer-shell interposes on `libwayland-client` and has to be loaded first. This binary links it directly, so the normal load order already satisfies that — needing this points at an unusual loader setup. Adjust the path to wherever your distribution installs the library. |
| `WHITESCREEN_NO_LAYER_SHELL=1` | Force fallback mode (fullscreen windows, no Identify) on a compositor that does support `wlr-layer-shell`. This is how the fallback path is tested without a GNOME or X11 session. |
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
