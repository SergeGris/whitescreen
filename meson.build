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

# CARGO_HOME is deliberately NOT set here. build-aux/cargo.sh defaults it to
# <builddir>/cargo-home when the environment does not provide one, which keeps
# a plain `meson compile` self-contained. Setting it here would override a
# packager's own CARGO_HOME -- and distro packaging typically primes that cache
# in a separate prepare step, so the override made that fetch prime a directory
# the build then ignored, re-downloading every crate.

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
