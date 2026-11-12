# Both files are templates: msgfmt merges the translated Name, Comment and
# summary in from the catalogues, which is why they are named `.in` and built
# rather than installed as they are. With no translations yet the output is
# the input, so nothing about an English install changes.
desktop_file = i18n.merge_file(
  input:       app_id + '.desktop.in',
  output:      app_id + '.desktop',
  type:        'desktop',
  po_dir:      meson.project_source_root() / 'po',
  install:     true,
  install_dir: datadir / 'applications',
)

metainfo_file = i18n.merge_file(
  input:       app_id + '.metainfo.xml.in',
  output:      app_id + '.metainfo.xml',
  type:        'xml',
  po_dir:      meson.project_source_root() / 'po',
  install:     true,
  install_dir: datadir / 'metainfo',
)

icondir = 'icons' / 'hicolor'

install_data(
  icondir / 'scalable' / 'apps' / (app_id + '.svg'),
  install_dir: datadir / icondir / 'scalable' / 'apps',
)

install_data(
  icondir / 'symbolic' / 'apps' / (app_id + '-symbolic.svg'),
  install_dir: datadir / icondir / 'symbolic' / 'apps',
)

# A raster icon alongside the scalable one. `appstreamcli compose` runs on the
# build host and needs to read an icon; shipping a PNG means that never depends
# on the host having gdk-pixbuf's SVG loader available.
install_data(
  icondir / '128x128' / 'apps' / (app_id + '.png'),
  install_dir: datadir / icondir / '128x128' / 'apps',
)

# ── Validation (skipped when the tools are absent) ───────────────────────────
#
# The *merged* files are what gets installed, so they are what is checked: a
# translation that broke the desktop entry would sail past a check of the
# template.
desktop_file_validate = find_program('desktop-file-validate', required: false)
if desktop_file_validate.found()
  test(
    'validate-desktop',
    desktop_file_validate,
    args: [desktop_file.full_path()],
    depends: desktop_file,
    suite: 'data',
  )
endif

appstreamcli = find_program('appstreamcli', required: false)
if appstreamcli.found()
  test(
    'validate-metainfo',
    appstreamcli,
    args: ['validate', '--no-net', '--explain', metainfo_file.full_path()],
    depends: metainfo_file,
    suite: 'data',
  )
endif
