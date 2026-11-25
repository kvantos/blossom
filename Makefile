# Build, install and package Blossom (the `blossom` app and the `blsm` CLI).
#
#   make                    build both binaries (release)
#   make install            install for you, under ~/.local (no sudo)
#   sudo make install       install for everyone, under /usr/local
#   make uninstall          remove what install put there (same user/PREFIX)
#   make help               every target
#
# `make install` never runs cargo, so `sudo make install` doesn't leave
# root-owned files in target/. Build first as yourself.

APP_ID      := io.github.kvantos.Blossom
VERSION     := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)

# For a normal user ~/.local (binaries in ~/.local/bin); as root /usr/local.
PREFIX      ?= $(if $(filter 0,$(shell id -u)),/usr/local,$(HOME)/.local)
DESTDIR     ?=
BINDIR      ?= $(PREFIX)/bin
DATADIR     ?= $(PREFIX)/share

CARGO       ?= cargo
CARGO_FLAGS ?= --locked
# A Rust target triple, e.g. x86_64-apple-darwin; empty for this machine.
TARGET      ?=
CARGO_TARGET_DIR ?= target
export CARGO_TARGET_DIR

ifeq ($(TARGET),)
  RELEASE   := $(CARGO_TARGET_DIR)/release
  TARGET_FLAG :=
  ARCH      ?= $(shell uname -m)
else
  RELEASE   := $(CARGO_TARGET_DIR)/$(TARGET)/release
  TARGET_FLAG := --target $(TARGET)
  ARCH      ?= $(firstword $(subst -, ,$(TARGET)))
endif
ifeq ($(ARCH),arm64)
  override ARCH := aarch64
endif
DEB_ARCH    := $(if $(filter x86_64,$(ARCH)),amd64,$(if $(filter aarch64,$(ARCH)),arm64,$(ARCH)))
MAC_ARCH    := $(if $(filter aarch64,$(ARCH)),arm64,$(ARCH))

OS          := $(shell uname -s)
DIST        := dist
ICON_SIZES  := 16 32 48 64 128 256 512
APPIMAGETOOL ?= appimagetool
INSTALL     ?= install

BINS        := blossom blsm

.PHONY: all build check install uninstall dist deb appimage macos-app dmg icons clean help

all: build

help:
	@echo "Blossom $(VERSION)"
	@echo "  make [build]        build blossom and blsm in release mode"
	@echo "  make check          format check, clippy and tests (what CI runs)"
	@echo "  make install        install into PREFIX: ~/.local, or /usr/local as root"
	@echo "  make uninstall      remove the installed files"
	@echo "  make dist           Linux .tar.gz in $(DIST)/"
	@echo "  make deb            Debian/Ubuntu .deb in $(DIST)/ (needs dpkg-deb)"
	@echo "  make appimage       AppImage in $(DIST)/ (needs appimagetool)"
	@echo "  make dmg            macOS .app in a .dmg in $(DIST)/ (on macOS)"
	@echo "  make clean          remove build output"
	@echo "Variables: PREFIX DESTDIR TARGET CARGO_FLAGS APPIMAGETOOL"

build:
	$(CARGO) build --release $(CARGO_FLAGS) $(TARGET_FLAG) -p blossom-app -p blsm

check:
	$(CARGO) fmt --all --check
	$(CARGO) clippy $(CARGO_FLAGS) --workspace --all-targets -- -D warnings
	$(CARGO) clippy $(CARGO_FLAGS) --workspace --all-targets --all-features -- -D warnings
	$(CARGO) test $(CARGO_FLAGS) --workspace --all-features

# The built binaries must exist; install doesn't build (see the top).
$(RELEASE)/blossom $(RELEASE)/blsm:
	@echo "error: $@ is missing; run 'make' first (as yourself, not root)" >&2
	@exit 1

# $(call put,MODE,SOURCE,DEST): install one file, creating its folder
# (BSD install on macOS has no -D).
put = mkdir -p "$(dir $(3))" && $(INSTALL) -m$(1) "$(2)" "$(3)"

install: $(addprefix $(RELEASE)/,$(BINS))
	$(INSTALL) -d "$(DESTDIR)$(BINDIR)"
	$(INSTALL) -m755 $(addprefix $(RELEASE)/,$(BINS)) "$(DESTDIR)$(BINDIR)/"
	$(call put,644,LICENSE,$(DESTDIR)$(DATADIR)/licenses/blossom/LICENSE)
	$(call put,644,README.md,$(DESTDIR)$(DATADIR)/doc/blossom/README.md)
ifneq ($(OS),Darwin)
	$(call put,644,packaging/linux/$(APP_ID).desktop,$(DESTDIR)$(DATADIR)/applications/$(APP_ID).desktop)
	$(call put,644,packaging/linux/$(APP_ID).metainfo.xml,$(DESTDIR)$(DATADIR)/metainfo/$(APP_ID).metainfo.xml)
	$(call put,644,assets/blossom.svg,$(DESTDIR)$(DATADIR)/icons/hicolor/scalable/apps/$(APP_ID).svg)
	for s in $(ICON_SIZES); do \
	  $(call put,644,assets/icons/$$s.png,$(DESTDIR)$(DATADIR)/icons/hicolor/$${s}x$${s}/apps/$(APP_ID).png) || exit 1; \
	done
endif
ifeq ($(DESTDIR),)
ifneq ($(OS),Darwin)
	sed -i.bak 's|^Exec=blossom |Exec=$(BINDIR)/blossom |' "$(DATADIR)/applications/$(APP_ID).desktop"
	rm -f "$(DATADIR)/applications/$(APP_ID).desktop.bak"
endif
	@case ":$$PATH:" in *":$(BINDIR):"*) ;; \
	  *) echo "note: $(BINDIR) is not on your PATH; add it to run blossom and blsm by name" ;; esac
endif

uninstall:
	for b in $(BINS); do rm -f "$(DESTDIR)$(BINDIR)/$$b"; done
	rm -rf "$(DESTDIR)$(DATADIR)/licenses/blossom" "$(DESTDIR)$(DATADIR)/doc/blossom"
	rm -f "$(DESTDIR)$(DATADIR)/applications/$(APP_ID).desktop"
	rm -f "$(DESTDIR)$(DATADIR)/metainfo/$(APP_ID).metainfo.xml"
	rm -f "$(DESTDIR)$(DATADIR)/icons/hicolor/scalable/apps/$(APP_ID).svg"
	for s in $(ICON_SIZES); do \
	  rm -f "$(DESTDIR)$(DATADIR)/icons/hicolor/$${s}x$${s}/apps/$(APP_ID).png"; \
	done

# A tarball laid out like a prefix (bin/, share/), so
# `sudo tar -xzf FILE -C /usr/local --strip-components=1` installs it.
TAR_NAME := blossom-$(VERSION)-linux-$(ARCH)
dist: $(addprefix $(RELEASE)/,$(BINS))
	rm -rf "$(DIST)/$(TAR_NAME)"
	$(MAKE) --no-print-directory install DESTDIR="$(CURDIR)/$(DIST)/$(TAR_NAME)" PREFIX=
	tar --owner=0 --group=0 -C "$(DIST)" -czf "$(DIST)/$(TAR_NAME).tar.gz" "$(TAR_NAME)"
	rm -rf "$(DIST)/$(TAR_NAME)"
	@echo "$(DIST)/$(TAR_NAME).tar.gz"

DEB_ROOT := $(DIST)/deb
DEB_FILE := $(DIST)/blossom_$(VERSION)_$(DEB_ARCH).deb
deb: $(addprefix $(RELEASE)/,$(BINS))
	rm -rf "$(DEB_ROOT)"
	$(MAKE) --no-print-directory install DESTDIR="$(CURDIR)/$(DEB_ROOT)" PREFIX=/usr
	rm -rf "$(DEB_ROOT)/usr/share/licenses"
	$(call put,644,LICENSE,$(DEB_ROOT)/usr/share/doc/blossom/copyright)
	$(INSTALL) -d "$(DEB_ROOT)/DEBIAN"
	sed -e 's/@VERSION@/$(VERSION)/' -e 's/@ARCH@/$(DEB_ARCH)/' \
	    -e "s/@SIZE@/$$(du -sk "$(DEB_ROOT)/usr" | cut -f1)/" \
	    packaging/debian/control > "$(DEB_ROOT)/DEBIAN/control"
	dpkg-deb --root-owner-group -Zxz --build "$(DEB_ROOT)" "$(DEB_FILE)"
	rm -rf "$(DEB_ROOT)"
	@echo "$(DEB_FILE)"

APPDIR   := $(DIST)/AppDir
APPIMAGE := $(DIST)/Blossom-$(VERSION)-$(ARCH).AppImage
appimage: $(addprefix $(RELEASE)/,$(BINS))
	rm -rf "$(APPDIR)"
	$(MAKE) --no-print-directory install DESTDIR="$(CURDIR)/$(APPDIR)" PREFIX=/usr
	ln -s usr/bin/blossom "$(APPDIR)/AppRun"
	cp packaging/linux/$(APP_ID).desktop "$(APPDIR)/"
	cp assets/blossom.svg "$(APPDIR)/$(APP_ID).svg"
	ARCH=$(ARCH) $(APPIMAGETOOL) --no-appstream "$(APPDIR)" "$(APPIMAGE)"
	rm -rf "$(APPDIR)"
	@echo "$(APPIMAGE)"

# macOS: Blossom.app (with blsm inside too), then a .dmg holding the app,
# a link to /Applications and blsm for the command line.
MAC_DIR  := $(DIST)/macos-$(MAC_ARCH)
MAC_APP  := $(MAC_DIR)/Blossom.app
DMG      := $(DIST)/Blossom-$(VERSION)-macos-$(MAC_ARCH).dmg
macos-app: $(addprefix $(RELEASE)/,$(BINS))
	rm -rf "$(MAC_DIR)"
	$(INSTALL) -d "$(MAC_APP)/Contents/MacOS" "$(MAC_APP)/Contents/Resources" "$(MAC_DIR)/Blossom.iconset"
	$(INSTALL) -m755 $(addprefix $(RELEASE)/,$(BINS)) "$(MAC_APP)/Contents/MacOS/"
	sed 's/@VERSION@/$(VERSION)/' packaging/macos/Info.plist > "$(MAC_APP)/Contents/Info.plist"
	for s in 16 32 128 256 512; do \
	  cp assets/icons/$$s.png "$(MAC_DIR)/Blossom.iconset/icon_$${s}x$${s}.png" && \
	  cp assets/icons/$$((s * 2)).png "$(MAC_DIR)/Blossom.iconset/icon_$${s}x$${s}@2x.png" || exit 1; \
	done
	iconutil -c icns -o "$(MAC_APP)/Contents/Resources/Blossom.icns" "$(MAC_DIR)/Blossom.iconset"
	rm -rf "$(MAC_DIR)/Blossom.iconset"
	@echo "$(MAC_APP)"

dmg: macos-app
	$(INSTALL) -m755 "$(RELEASE)/blsm" "$(MAC_DIR)/blsm"
	ln -s /Applications "$(MAC_DIR)/Applications"
	rm -f "$(DMG)"
	hdiutil create -volname "Blossom $(VERSION)" -srcfolder "$(MAC_DIR)" -ov -format UDZO "$(DMG)"
	rm -rf "$(MAC_DIR)"
	@echo "$(DMG)"

# For maintainers: redraw the PNG icons from assets/blossom.svg.
icons:
	for s in $(ICON_SIZES) 1024; do \
	  rsvg-convert -w $$s -h $$s assets/blossom.svg -o assets/icons/$$s.png || exit 1; \
	done

clean:
	$(CARGO) clean
	rm -rf "$(DIST)"
