project('openeffects', 'rust',
  version: '0.1.0',
  meson_version: '>=1.2.0',
  license: 'GPL-3.0-or-later',
)

cargo_profile = get_option('cargo_profile')
cargo_target_dir = meson.project_source_root() / 'target' / cargo_profile
bindir = get_option('bindir')
datadir = get_option('datadir')

foreach bin : ['openeffectsd', 'openeffectsctl', 'openeffects']
  install_data(cargo_target_dir / bin, install_dir: bindir, install_mode: 'rwxr-xr-x')
endforeach

install_data(
  'data/dbus/org.openeffects.Daemon1.xml',
  'data/dbus/org.openeffects.Effects1.xml',
  'data/dbus/org.openeffects.Devices1.xml',
  install_dir: datadir / 'dbus-1/interfaces',
)

dbus_service_conf = configuration_data()
dbus_service_conf.set('bindir', get_option('prefix') / bindir)
configure_file(
  input: 'data/dbus-services/org.openeffects.Daemon.service.in',
  output: 'org.openeffects.Daemon.service',
  configuration: dbus_service_conf,
  install_dir: datadir / 'dbus-1/services',
)

systemd_service_conf = configuration_data()
systemd_service_conf.set('bindir', get_option('prefix') / bindir)
configure_file(
  input: 'data/systemd/openeffectsd.service.in',
  output: 'openeffectsd.service',
  configuration: systemd_service_conf,
  install_dir: datadir / 'systemd/user',
)

install_data(
  'data/icons/openeffects.svg',
  install_dir: datadir / 'icons/hicolor/scalable/apps',
)

install_data(
  'data/applications/openeffects.desktop',
  install_dir: datadir / 'applications',
)

# Models are not in the source tree (downloaded + sha256-verified at runtime),
# so a meson install ships none. Source/dev installs populate the per-user model
# dir via scripts/fetch-models.sh; distro packages bundle them (packaging/nfpm.yaml).

meson.add_install_script('meson_post_install.sh')
