project('vinoopeneffects', 'rust',
  version: '0.1.4',
  meson_version: '>=1.2.0',
  license: 'GPL-3.0-or-later',
)

cargo_profile = get_option('cargo_profile')
cargo_target_dir = meson.project_source_root() / 'target' / cargo_profile
bindir = get_option('bindir')
datadir = get_option('datadir')

foreach bin : ['vinoopeneffects-daemon', 'vinoopeneffects-ctl', 'vinoopeneffects']
  install_data(cargo_target_dir / bin, install_dir: bindir, install_mode: 'rwxr-xr-x')
endforeach

install_data(
  'data/dbus/io.github.karanshukla.vinoopeneffects.Daemon1.xml',
  'data/dbus/io.github.karanshukla.vinoopeneffects.Effects1.xml',
  'data/dbus/io.github.karanshukla.vinoopeneffects.Devices1.xml',
  install_dir: datadir / 'dbus-1/interfaces',
)

dbus_service_conf = configuration_data()
dbus_service_conf.set('bindir', get_option('prefix') / bindir)
configure_file(
  input: 'data/dbus-services/io.github.karanshukla.vinoopeneffects.Daemon.service.in',
  output: 'io.github.karanshukla.vinoopeneffects.Daemon.service',
  configuration: dbus_service_conf,
  install_dir: datadir / 'dbus-1/services',
)

systemd_service_conf = configuration_data()
systemd_service_conf.set('bindir', get_option('prefix') / bindir)
configure_file(
  input: 'data/systemd/vinoopeneffects-daemon.service.in',
  output: 'vinoopeneffects-daemon.service',
  configuration: systemd_service_conf,
  install_dir: datadir / 'systemd/user',
)

install_data(
  'data/icons/io.github.karanshukla.vinoopeneffects.svg',
  install_dir: datadir / 'icons/hicolor/scalable/apps',
)
install_data(
  'data/icons/io.github.karanshukla.vinoopeneffects-symbolic.svg',
  install_dir: datadir / 'icons/hicolor/symbolic/apps',
)

install_data(
  'data/applications/io.github.karanshukla.vinoopeneffects.desktop',
  install_dir: datadir / 'applications',
)

# Models are not in the source tree (downloaded + sha256-verified at runtime),
# so a meson install ships none. Source/dev installs populate the per-user model
# dir via scripts/fetch-models.sh; distro packages bundle them (packaging/nfpm.yaml).

meson.add_install_script('meson_post_install.sh')
