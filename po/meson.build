# No --language here, deliberately. POTFILES.in mixes Rust source with the
# desktop entry and the AppStream metadata, and xgettext picks the right
# parser for each from its extension -- forcing one would read the desktop
# entry as C and come back with nothing from it.
#
# `mark` is this project's no-op marker for strings that live in a const table
# and are looked up later; without the keyword, xgettext would walk straight
# past the colour names and the region labels.
i18n.gettext(
  meson.project_name(),
  args: [
    '--from-code=UTF-8',
    '--keyword=gettext',
    '--keyword=mark',
    '--add-comments=TRANSLATORS:',
    '--package-name=' + meson.project_name(),
    '--package-version=' + meson.project_version(),
    '--msgid-bugs-address=https://github.com/SergeGris/whitescreen/issues',
  ],
)
