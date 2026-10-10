PREFIX ?= /usr/local
DESTDIR ?=
BINDIR ?= $(PREFIX)/bin
DATADIR ?= $(PREFIX)/share
BUILD_DIR ?= target/release
CARGO ?= cargo

.PHONY: all build install

all: build

build:
	$(CARGO) build --release

# Build first as your normal user, then install (with sudo for system installs).
install:
	install -Dm755 "$(BUILD_DIR)/cursor-clip" "$(DESTDIR)$(BINDIR)/cursor-clip"
	install -Dm755 "$(BUILD_DIR)/cursor-clip-toggle" "$(DESTDIR)$(BINDIR)/cursor-clip-toggle"
	install -Dm644 assets/io.github.sirulex.cursor-clip.desktop "$(DESTDIR)$(DATADIR)/applications/io.github.sirulex.cursor-clip.desktop"
	install -Dm644 assets/io.github.sirulex.cursor-clip.svg "$(DESTDIR)$(DATADIR)/icons/hicolor/scalable/apps/io.github.sirulex.cursor-clip.svg"
