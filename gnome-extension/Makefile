# OpenEffects GNOME Shell extension — install / pack / lint helpers.

NAME = openeffects@funinkina.co.in
SRC  = $(NAME)
DEST = $(HOME)/.local/share/gnome-shell/extensions/$(NAME)

.PHONY: all install uninstall enable disable reinstall pack lint logs restart nested

all: install

install:
	mkdir -p "$(DEST)"
	cp -r $(SRC)/. "$(DEST)/"
	@echo "Installed to $(DEST)"
	@echo "Wayland: log out and back in, then 'make enable'."
	@echo "Xorg:    Alt+F2, type 'r', Enter, then 'make enable'."

uninstall:
	rm -rf "$(DEST)"
	@echo "Removed $(DEST)"

enable:
	gnome-extensions enable $(NAME)

disable:
	gnome-extensions disable $(NAME)

reinstall: uninstall install

# Build a redistributable zip (EGO-compatible). Bundles the extra JS modules and
# the icons dir that 'gnome-extensions pack' does not pick up by default.
pack:
	gnome-extensions pack --force \
	  --extra-source=dbus.js \
	  --extra-source=indicator.js \
	  --extra-source=sections.js \
	  --extra-source=icons \
	  $(SRC)
	@echo "Wrote $(NAME).shell-extension.zip"

# Syntax gate (parse only, no execution — shell resource imports aren't
# resolvable outside gnome-shell). Uses node --check if available.
lint:
	@command -v node >/dev/null 2>&1 || { echo "node not found; skipping lint"; exit 0; }
	@set -e; for f in $(SRC)/*.js; do node --check "$$f" && echo "ok:   $$f"; done

logs:
	journalctl -f -o cat /usr/bin/gnome-shell

restart:
	@echo "Wayland: log out and back in (gnome-shell can't restart in place)."
	@echo "Xorg:    Alt+F2, type 'r', Enter."
	@echo "Sandbox: 'make nested' opens a throwaway nested shell."

# Throwaway nested shell for testing without touching your session.
nested:
	dbus-run-session -- gnome-shell --nested --wayland
