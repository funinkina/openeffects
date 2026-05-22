#!/bin/sh
set -eu

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t -f "${MESON_INSTALL_DESTDIR_PREFIX}/share/icons/hicolor" || true
fi

if command -v systemctl >/dev/null 2>&1; then
  systemctl --user daemon-reload >/dev/null 2>&1 || true
fi
