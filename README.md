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
- GLib ≥ 2.80 (what GTK 4.14 builds against)
- Meson ≥ 0.62 (for the install path)
- gettext (for the translations; `msgfmt` and `xgettext`)

## Windows

The repository is a workspace of three crates:

| Crate | What it is |
| ----- | ---------- |
| `whitescreen` (root) | the GTK 4 application — everything described above |
| `core/` → `whitescreen-core` | what the app knows that is not about a toolkit: colours, the geometry of every fill, color temperature, monitor-name matching, placeholder substitution. No dependencies at all, and no display needed to test it. |
| `platform/windows/` → `whitescreen-win` | the native Win32 front end |

Windows has no `wlr-layer-shell`, and does not need one: a borderless
`WS_POPUP` sized to a monitor and kept on `HWND_TOPMOST` is exactly the
surface this app wants, and it is an ordinary window. `whitescreen-win`
implements that natively — no GTK, no Wayland, no fallback mode:

- **Monitor enumeration** through `EnumDisplayMonitors`, keyed by the display
  device (`\\.\DISPLAY1`) and labelled with the adapter's own string ("Dell
  U2415"). `--monitor` patterns match either, through the same matcher the
  GTK build uses.
- **One overlay per monitor** — topmost, no taskbar button, cursor hidden,
  and any key or click takes them down, by the same rule as on Wayland (a
  bare key dismisses; <kbd>Ctrl</kbd>/<kbd>Alt</kbd>/<kbd>Win</kbd>
  combinations are shortcuts and pass through). Windows delivers keystrokes
  only to the focused window, so with several screens covered a key works on
  the focused overlay and a click works on any of them — the same limitation
  the GTK build has in its fallback mode, for the same reason.
- **Every fill painted with GDI** — flat, region, wedge, grid and the
  drifting noise. A region with no second color uses a colour-keyed layered
  window, so the uncovered half really does show the desktop through it
  rather than painting it black.
- **`SetThreadExecutionState`** to hold the display awake, the counterpart of
  the idle inhibitor.

What keeps the two front ends honest is that neither owns the answers. The
bands of a wedge, the halves of a region, the color of a 3200 K key light and
the `--monitor` matcher all live in `whitescreen-core` and are tested there,
so a fix to the wedge's seam arithmetic cannot land on one platform only.

### Portable

The Windows build is one executable and nothing else: unzip it and run it,
from a USB stick if you like. No installer, no registry, nothing written
anywhere, and no Visual C++ redistributable to install first — the C runtime
is linked statically (see `.cargo/config.toml`).

Every push to the default branch publishes it alongside the Flatpak bundle:

```
https://github.com/SergeGris/whitescreen/releases/download/continuous/whitescreen-windows-x86_64.zip
```

It is driven entirely from the command line, which is also how you would use
it from a shortcut:

```sh
whitescreen.exe --color red --all                 # fill every screen
whitescreen.exe --pattern grid --monitor "DISPLAY1"
whitescreen.exe --color "#FFB16E" --brightness 60 # a dim key light
whitescreen.exe --list-monitors --json
```

`--monitor` matches the device name (`\\.\DISPLAY1`) or the model ("Dell
U2415"), with the same shell patterns the GTK build accepts. `--color` takes
`#RRGGBB` or one of the names the chips use; the GTK build additionally
accepts any CSS colour, because it has GDK to parse them.

To build it yourself:

```sh
cargo build --release -p whitescreen-win   # on Windows
cargo test -p whitescreen-core             # anywhere
```

> **Not done yet:** the control window. Only the overlay half is native so
> far — the colour chips, the monitor sidebar, the cycle queue and the
> command line are still GTK-only, so on Windows there is as yet nothing to
> drive the overlays *from*. That is the next piece, and it is a bigger one:
> it is most of the UI.

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
2. Pick a color — a preset, a **Key light** temperature, or **Custom** for the
   full picker.
3. Press **Show on selected**.
4. Press any key on an overlay to dismiss every one of them, or **Hide ALL**.

Everything you choose is remembered: the color, the custom color, the
brightness, the ticked monitors and their per-monitor colors, the fill mode,
the auto-hide delay, and the cycle queue and its interval all come back on the
next launch. They live in `$XDG_CONFIG_HOME/whitescreen/settings.ini`, which
is a plain key file you can edit or delete.

Two switches are deliberately *not* remembered: **Cycle** and **Identify**.
Anything that changes the color you asked for is saved; those two are the ones
that would make something extra appear the next time you press Show — a
sequence flashing across every screen, and a badge on each of them — and
restoring either would spring it on you with nothing on screen to say why.
(Auto-hide is saved even though it acts on its own, because all it ever does
is take away what you asked for, after a delay you chose.)

Any key dismisses the overlays, not just <kbd>Esc</kbd> — a full-screen color
with no other keyboard function should not be something you can get stuck
behind. Key *combinations* are left alone, so <kbd>Ctrl</kbd>+<kbd>Q</kbd>
still quits from an overlay.

Which is what leaves room for the overlay's own shortcuts:

| On an overlay | |
| ------------- | --- |
| <kbd>Ctrl</kbd>+<kbd>→</kbd> / <kbd>Ctrl</kbd>+<kbd>↓</kbd> / <kbd>Ctrl</kbd>+<kbd>N</kbd> | Next color in the queue |
| <kbd>Ctrl</kbd>+<kbd>←</kbd> / <kbd>Ctrl</kbd>+<kbd>↑</kbd> / <kbd>Ctrl</kbd>+<kbd>P</kbd> | Previous color |
| <kbd>Ctrl</kbd>+<kbd>Space</kbd> | Pause and resume the cycle timer |
| any other key | Dismiss every overlay |

Stepping works whether or not the timer is running, which is the point: a
dead-pixel sweep at your own pace never needs the control window, and the
control window is underneath the color you are looking at.

**Custom** shows the color it will apply, and starts on cyan. Clicking it
reopens the picker; selecting it again just re-applies what you last chose.

**Key light** is the second row of chips: 3200 K, 4300 K, 5600 K and 6500 K,
the temperatures a camera's white-balance presets are named after. A display
pressed into service as a soft light has to match the other lights in the
room, and those are sold by temperature — this is the color of a black body
at that temperature, so the panel matches a tungsten fixture or a
daylight-balanced LED without a trip through the picker.

**Brightness** scales whatever color is chosen on its way to the screens,
from 5 % to 100 %. A panel at full output is usually too bright to light a
face with, and dimming it this way keeps the color you picked — the chips
stay marked, and the queue still recognises the step it is on.

**Hide after** takes the overlays down again after the given number of
minutes, or never at 0. The exerciser wants minutes of running, and the room
it is in may not have anyone in it.

**Identify** flashes each monitor's connector name (`DP-1`, `HDMI-A-2`, …) on the
screen itself, so you always know which physical panel you're looking at.

**Cycle** steps through a queue of colors, from 0.5 to 60 seconds each, so a
dead-pixel check does not mean looking away from the panel to click. It is a
mode rather than a transport: arming it does nothing on its own, and the queue
plays only while an overlay is up. Press **Show on selected** and it starts;
press a key on the overlay or **Hide ALL** and it stops, putting back the color
it took over from. It stays armed, so the next Show picks it up again. (Arming
it while the overlays are already up starts it there and then, since they cover
the window the button is in.)

The queue starts as the standard sequence — white and black drive every subpixel fully on
and fully off, red/green/blue light one at a time, cyan/magenta/yellow light
them in pairs, which is where a single dead subpixel stops hiding behind its
neighbours.

The strip under the Cycle button *is* the queue, and it is yours to edit:
click a swatch to put that color on screen, **double-click** it (or
right-click) to change that step's color, the **×** on it to drop the step,
**+** to append whatever color is showing now, and the reset button to go back
to the default sequence. It holds up to 16 steps.

The queue and the preset chips stay in step with each other: whichever of them
holds the color currently on screen is the one marked. Click black in the queue
and the **Black** chip is selected; press the **Green** chip and the green step
is ringed. The chips grey out while the queue is playing, because the timer owns
the color then — and it carries on from the step already showing rather than
jumping back to the top.

(The 0.5 s floor is on purpose: a full-screen color flashing faster than that
approaches the rate photosensitive-epilepsy guidance warns about.)

### Fill modes

Four modes under the action row, all off by default and only one at a time —
noise over half a screen exercises half the pixels, and a wedge behind a grid
is two patterns answering different questions.

**Region** covers only half of each screen — left, right, top or bottom. The
rest is left unpainted, so whatever is behind it stays visible: that is how you
judge a color against the desktop beside it rather than from memory. Tick
**Second color** and the other half is filled too, which turns it into a split
field — the honest way to compare two panels or two halves of one, since the
eye is far better at an edge than at an absolute.

**Exerciser** drifts a block of random noise across each screen. Driving a
subpixel hard through many values for a few minutes is the established way to
free a stuck one, and confining it to a moving block is both how the technique
works and what keeps a whole screen from flashing.

**Wedge** lays the chosen color out as a staircase of 16 bands, from black at
the left to full output at the right. A flat field finds a pixel that is dead;
a ramp finds the faults that only exist *between* levels — banding, a mis-set
gamma, black crush at the bottom of the range — because the eye is far better
at an edge between two bands than at any absolute level. On white it is the
usual grey step wedge; on red it checks one channel on its own.

**Grid** draws one-pixel lines of the chosen color on black, eight pixels
apart. It answers whether one screen pixel is still one panel pixel: at 1:1
the lines are even and all the same weight, and anything resampling the image
— a scaled output, a projector, a TV doing overscan — turns them into a moiré
or into lines of two different widths. The lines are one *device* pixel, so
they stay a hairline on a HiDPI screen.

Note that an overlay still takes input over its whole surface, including the
transparent half — you can see what is behind it, not click it.

### Which monitor am I changing?

With more than one monitor attached, the Choose Color card grows an **Applies
to** selector in its heading, and that is the answer to the question the card
could not previously be asked:

- **All monitors** — the colors drive every screen that has not been given one
  of its own. If some have, the card says so underneath: *"HEADLESS-2 keeps a
  color of its own"*, so "all" never quietly means "all but that one".
- **a monitor by name** — every color you pick now goes to that screen alone,
  and the others are left exactly as they are. This is how you move a color
  onto one screen: aim the card and click.

You can aim it from either end. Clicking a monitor's swatch in the sidebar
points the card at that monitor, and the swatch takes the accent ring to say
so — the same ring the cycle queue puts on the step that is showing, because
it means the same thing: this is the one. Picking a monitor in **Applies to**
moves the ring in the sidebar. They are two views of one choice, not two
controls that happen to overlap.

The card always shows what it is editing. Aim it at a monitor and the preview,
the marked chip and the queue's ring all switch to that monitor's color; aim
it back at all of them and they switch back to the shared one. Nothing is
changed by aiming — it only decides where the next color lands.

A monitor given its own color this way behaves exactly like one colored from
the sidebar, because it is the same thing: per-monitor colors switch on, and
its row's swatch stops following the chips.

With one monitor attached the selector is not shown, because there is nothing
to choose between.

The cycle is the exception, and deliberately: the sequence plays onto the
screens, so it always drives the shared color and never pins a monitor, no
matter where the card is aimed. Clicking a swatch in the strip yourself is an
ordinary choice in the card and does follow the aim.

### Per-monitor colors

Tick **Per-monitor colors** in the sidebar and each row grows a swatch — the
same swatch the chips and the queue use, so a color reads the same wherever it
appears in the window.

A row that has not been given a color of its own shows the color from the
card, because that is what that screen is displaying, and it keeps following
the chips until that monitor is given one. Once it is, it stops following: a
monitor given its own color keeps it while the chips drive the rest, which is
what you want for telling screens apart, or for holding a reference gray on one
panel while you tint another. Clearing the tick box leaves the colors
remembered for when you switch it back on, and they are saved between runs
like everything else.

A swatch behaves like one in the cycle queue: **click** it to point the card
at that monitor and then choose from the chips, or **double-click** (or
right-click) to go straight to the picker for it. The picker is titled for the
screen it belongs to, so a dialog that has lost its window still says what it
is for.

`--monitor-color NAME=COLOR` does the same from a keybind or a script, and
switches the mode on for you — asking for a per-monitor color is asking for
per-monitor colors. A color set this way behaves like one set in the sidebar:
brightness still scales it, the fill modes still apply to it (a wedge on that
screen ramps *its* color), and the cycle leaves it alone while it drives the
others.

While an overlay is up the session is kept awake, so a screen used as a key
light or left on a test pattern does not blank halfway through. Both routes are
best-effort — `zwp_idle_inhibit_manager_v1` where the compositor has it, and a
session manager or the inhibit portal otherwise.

## Command line

Every flag works on an instance that is already running as well as on a cold
start — the invocation is forwarded to the window that is already open — so
these are usable as compositor keybinds:

```sh
whitescreen --color '#FF0000' --all --show    # fill every screen red
whitescreen --monitor DP-1 --color white --show
whitescreen --monitor 'DP-*' --color white --show   # every DisplayPort screen
whitescreen --cycle --interval 1 --all --show # play the queue, one second a step
whitescreen --pattern grid --all --show       # check for 1:1 pixel mapping
whitescreen --pattern wedge --all --show      # check for banding
whitescreen --color '#FFB16E' --brightness 60 --all --show   # a dim key light
whitescreen --exerciser --auto-hide 10 --all --show          # run it for ten minutes
whitescreen --queue '#fff,#000,red' --cycle --all --show     # a queue of your own
whitescreen --all --monitor-color DP-1=white --monitor-color 'HDMI-*=#808080' --show
whitescreen --hide                            # dismiss, from a keybind
whitescreen --list-monitors                   # the names --monitor accepts
whitescreen --list-monitors --json            # the same, for a script
```

| Flag | |
| ---- | --- |
| `-c, --color COLOR` | A name, `#RRGGBB`, or any CSS color. Note that `green` is CSS green (`#008000`); pass `#00FF00` for the Green chip. |
| `-m, --monitor NAME` | Repeat for several. `NAME` may be a shell pattern — `*`, `?` and `[...]` classes, including `[!...]` — matched against the connector names. **Quote it**: the shell would otherwise try to expand it against filenames, and zsh refuses outright when nothing matches. A plain name with no wildcard is remembered even when that screen is not plugged in, and is ticked the moment it appears; a pattern that matches nothing is reported, along with the names that are there. |
| `-M, --monitor-color NAME=COLOR` | Give the monitors matching `NAME` a color of their own, and switch per-monitor colors on. `NAME` is matched exactly as `--monitor` matches it, quoting and all. Repeat for several. Coloring a monitor does not select it — ticking a row and setting its color are two actions in the sidebar too. |
| `--no-monitor-colors` | Switch per-monitor colors off again, so every screen follows the shared color. The colors themselves are kept, exactly as unticking the box in the sidebar keeps them. |
| `-a, --all` | Every attached monitor. |
| `-s, --show`, `-H, --hide` | Put the overlays up, or take them down. |
| `-y, --cycle` | Arm cycle mode, so showing plays the queue. |
| `-i, --interval SECONDS` | Seconds per cycle step, clamped to the 0.5–60 the window offers. |
| `-p, --pattern NAME` | What covers each screen: `flat`, `region`, `exerciser`, `wedge` or `grid`. |
| `-x, --exerciser` | Shorthand for `--pattern exerciser`. |
| `-r, --region SIDE` | Cover only this half — `full`, `left`, `right`, `top`, `bottom` — which implies `--pattern region`. |
| `-S, --second-color COLOR` | Fill the other half of a region instead of leaving it clear. |
| `-b, --brightness PERCENT` | Scale the color on its way to the screens, 5–100. |
| `-A, --auto-hide MINUTES` | Take the overlays down after this long; 0 leaves them up. |
| `-q, --queue COLORS` | Replace the cycle queue with this comma-separated list. All of it has to parse, or none of it is used. |
| `-I, --identify` | Flash each monitor's connector name on the screen itself. Needs layer-shell. |
| `-l, --list-monitors` | Print the monitor names and resolutions, then exit without opening a window. |
| `-j, --json` | With `--list-monitors`, print JSON: name, size, scale, refresh rate and model. |
| `-V, --version` | Print the version and exit. |

`--help` lists them all.

### Scripting

Output comes back to the terminal that typed the command, not to the one that
started the app. That is worth stating because it is not free: an invocation
forwarded to an instance already running is *handled* by that instance, so
`--list-monitors` had to be routed back to the caller deliberately. Without
that, piping it to a script would have produced nothing at all whenever a
window happened to be open.

The exit code says whether the invocation was accepted: `0` when every flag
was understood, `1` when any of them was not — a color that does not parse, a
pattern that matches no monitor, a `NAME=COLOR` pair missing its `=`. The
flags that *were* good still take effect; a typo in one is not a reason to
ignore the others.

```sh
whitescreen --list-monitors --json | jq -r '.[].name'
whitescreen --color "$1" --all --show || notify-send "not a color: $1"
```

### No window unless you want one

An invocation that puts something on the screens does not open the control
window. `whitescreen --color red --all --show` from a keybind fills the screens
and nothing else appears; dismissing the overlay ends the process, so nothing
lingers invisibly. `whitescreen --hide` with nothing running has nothing to do
and leaves immediately.

Everything else opens the window as before: a bare `whitescreen`, and any
invocation that only configures (`--color red` on its own leaves the app open
on red). A bare `whitescreen` is also how you summon the window from an
instance that started headless — after that it behaves like any other session,
and dismissing the overlays no longer quits.

## Translating

Every string the user can read goes through gettext, including the `--help`
text, the desktop entry and the AppStream summary. There are no translations
yet; adding one is:

```sh
meson setup _build
ninja -C _build whitescreen-pot          # regenerate po/whitescreen.pot
msginit -i po/whitescreen.pot -l de -o po/de.po
echo de >> po/LINGUAS                    # one language code per line
```

Then fill in `po/de.po` and rebuild. An untranslated build and a missing
translation both fall back to the English text, one string at a time, so a
half-finished catalogue is perfectly usable.

Strings with a value in them use numbered placeholders — `Step {0} — {1}` —
rather than bare `{}`, so that a translation can put the values in whatever
order the language needs. A placeholder with no argument behind it is left
visible rather than crashing the app.

`po/POTFILES.in` lists the files xgettext reads. A new source file with a
translatable string in it has to be added there by hand, or its strings are
silently left out.

## Troubleshooting

| Variable | Effect |
| -------- | ------ |
| `LD_PRELOAD=/usr/lib/libgtk4-layer-shell.so` | Last-resort workaround if the app falls back to fullscreen windows on a compositor that does support `wlr-layer-shell` (niri, Sway, Hyprland, KDE). gtk4-layer-shell interposes on `libwayland-client` and has to be loaded first. This binary links it directly, so the normal load order already satisfies that — needing this points at an unusual loader setup. Adjust the path to wherever your distribution installs the library. |
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
