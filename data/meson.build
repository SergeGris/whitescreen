desktop_file = app_id + '.desktop'
install_data(
  desktop_file,
  install_dir: datadir / 'applications',
)

metainfo_file = app_id + '.metainfo.xml'
install_data(
  metainfo_file,
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

# ── Validation (skipped when the tools are absent) ───────────────────────────
desktop_file_validate = find_program('desktop-file-validate', required: false)
if desktop_file_validate.found()
  test(
    'validate-desktop',
    desktop_file_validate,
    args: [meson.current_source_dir() / desktop_file],
    suite: 'data',
  )
endif

appstreamcli = find_program('appstreamcli', required: false)
if appstreamcli.found()
  test(
    'validate-metainfo',
    appstreamcli,
    args: ['validate', '--no-net', '--explain', meson.current_source_dir() / metainfo_file],
    suite: 'data',
  )
endif
