project(
  'whitescreen',
  version: '1.0.0',
  license: 'GPL-3.0-or-later',
  meson_version: '>= 0.62.0',
)

app_id = 'io.github.SergeGris.WhiteScreen'

cargo = find_program('cargo', required: true)
gnome = import('gnome')

prefix     = get_option('prefix')
bindir     = prefix / get_option('bindir')
datadir    = prefix / get_option('datadir')

cargo_env = environment()
cargo_env.set('CARGO_HOME', meson.project_build_root() / 'cargo-home')

cargo_options = ['--manifest-path', meson.project_source_root() / 'Cargo.toml']

if get_option('gamma')
  cargo_options += ['--features', 'gamma']
endif

# Only Meson's plain 'debug' buildtype maps to cargo's dev profile. Everything
# else — release, debugoptimized, minsize, plain — must produce an optimised
# binary, since distro packagers commonly build with 'plain' or 'debugoptimized'.
if get_option('buildtype') == 'debug'
  rust_profile = 'debug'
else
  rust_profile = 'release'
  cargo_options += ['--release']
endif

cargo_build = custom_target(
  'cargo-build',
  build_by_default: true,
  build_always_stale: true,
  output: meson.project_name(),
  console: true,
  install: true,
  install_dir: bindir,
  env: cargo_env,
  command: [
    meson.project_source_root() / 'build-aux' / 'cargo.sh',
    meson.project_build_root(),
    meson.project_source_root(),
    '@OUTPUT@',
    rust_profile,
    meson.project_name(),
    cargo_options,
  ],
)

subdir('data')

gnome.post_install(
  gtk_update_icon_cache: true,
  update_desktop_database: true,
)

summary({
  'prefix'      : prefix,
  'bindir'      : bindir,
  'datadir'     : datadir,
  'buildtype'   : get_option('buildtype'),
  'gamma'       : get_option('gamma'),
}, section: 'Configuration')
