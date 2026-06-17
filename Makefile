# lowtech — minimal Logitech HID++ tool. Run `make help` for targets.

CARGO ?= cargo
BIN := lowtech
UDEV_RULE := linux/42-logitech-hidpp.rules
UDEV_DEST := /etc/udev/rules.d/

.PHONY: help build release install uninstall install-udev uninstall-udev deps fmt clippy check test clean

help:
	@echo "lowtech — make targets:"
	@echo "  build           build the release binary (target/release/$(BIN))"
	@echo "  install         install via cargo (to ~/.cargo/bin)"
	@echo "  uninstall       remove the cargo-installed binary"
	@echo "  install-udev    [Linux] install udev rule for non-root device access"
	@echo "  uninstall-udev  [Linux] remove the udev rule"
	@echo "  deps            show the binary dynamic-library dependencies"
	@echo "  fmt clippy check test clean   usual cargo wrappers"

build release:
	$(CARGO) build --release

install:
	$(CARGO) install --path .

uninstall:
	$(CARGO) uninstall $(BIN)

# The working counterpart to Solaar's broken `make install_udev` (Linux only).
install-udev:
	@if [ "$$(uname -s)" = "Linux" ]; then sudo cp $(UDEV_RULE) $(UDEV_DEST) && sudo udevadm control --reload-rules && sudo udevadm trigger && echo "Installed - replug the receiver."; else echo "install-udev is Linux-only (macOS uses Input Monitoring, not udev)."; fi

uninstall-udev:
	@if [ "$$(uname -s)" = "Linux" ]; then sudo rm -f $(UDEV_DEST)$(notdir $(UDEV_RULE)) && sudo udevadm control --reload-rules && sudo udevadm trigger; else echo "install-udev is Linux-only."; fi

deps: build
	@if [ "$$(uname -s)" = "Darwin" ]; then otool -L target/release/$(BIN); else ldd target/release/$(BIN); fi

fmt:
	$(CARGO) fmt

clippy:
	$(CARGO) clippy --release

check:
	$(CARGO) check

test:
	$(CARGO) test

clean:
	$(CARGO) clean
