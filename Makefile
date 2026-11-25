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

.PHONY: all build check install uninstall dist deb appimage macos-app dmg icons clean help \
        fuzz-build fuzz fuzz-all fuzz-repro fuzz-merge fuzz-valgrind

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
	@echo "  make fuzz           fuzz one target (FUZZ=$(FUZZ), SECS=$(SECS)); stable Rust"
	@echo "  make fuzz-all       fuzz every target ($(FUZZ_TARGETS)) for SECS each"
	@echo "  make fuzz-repro     replay a crash: FUZZ=splice CRASH=fuzz/artifacts/splice/crash-..."
	@echo "  make fuzz-merge     add new coverage to the minimal corpus, fuzz/corpus/min/"
	@echo "  make fuzz-valgrind  replay the minimal corpus under Valgrind (memory errors)"
	@echo "Variables: PREFIX DESTDIR TARGET CARGO_FLAGS APPIMAGETOOL FUZZ SECS"

build:
	$(CARGO) build --release $(CARGO_FLAGS) $(TARGET_FLAG) -p blossom-app -p blsm

check:
	$(CARGO) fmt --all --check
	$(CARGO) clippy $(CARGO_FLAGS) --workspace --all-targets -- -D warnings
	$(CARGO) clippy $(CARGO_FLAGS) -p blsm -p blossom-app --no-default-features --all-targets -- -D warnings
	$(CARGO) clippy $(CARGO_FLAGS) --workspace --all-targets --all-features -- -D warnings
	$(CARGO) test $(CARGO_FLAGS) --workspace --all-features

# Fuzzing on stable Rust. The targets in fuzz/ use libFuzzer (from
# libfuzzer-sys, built with the system's C++ compiler); the flags are
# cargo-fuzz's coverage instrumentation without its sanitizer, the one part
# that needs nightly. Our crates have no unsafe code and every check is an
# assert, so the sanitizer would mostly watch dependencies.
FUZZ         ?= roundtrip
FUZZ_TARGETS := roundtrip splice document
SECS         ?= 60
FUZZ_TRIPLE  := $(shell rustc -vV | sed -n 's/^host: //p')
FUZZ_TARGET_DIR := fuzz/target
FUZZ_BIN     := $(FUZZ_TARGET_DIR)/$(FUZZ_TRIPLE)/release
FUZZ_SEEDS   := crates/blossom-core/tests/fixtures
FUZZ_RUSTFLAGS := -Cpasses=sancov-module \
  -Cllvm-args=-sanitizer-coverage-level=4 \
  -Cllvm-args=-sanitizer-coverage-inline-8bit-counters \
  -Cllvm-args=-sanitizer-coverage-pc-table \
  -Cllvm-args=-sanitizer-coverage-trace-compares \
  --cfg fuzzing -Cdebug-assertions -Coverflow-checks
ifeq ($(OS),Linux)
  FUZZ_RUSTFLAGS += -Cllvm-args=-sanitizer-coverage-stack-depth
endif

# --target keeps the instrumentation off build scripts, which would not link.
fuzz-build:
	RUSTFLAGS="$(FUZZ_RUSTFLAGS) $(RUSTFLAGS)" CARGO_TARGET_DIR=$(FUZZ_TARGET_DIR) \
	  $(CARGO) build --release $(CARGO_FLAGS) --manifest-path fuzz/Cargo.toml \
	  --target $(FUZZ_TRIPLE) --bins

# The corpus grows in fuzz/corpus/FUZZ, seeded from the test fixtures and
# the minimal corpus (below); a failing input is saved in fuzz/artifacts/FUZZ/.
fuzz: fuzz-build
	mkdir -p fuzz/corpus/$(FUZZ) fuzz/corpus/min/$(FUZZ) fuzz/artifacts/$(FUZZ)
	$(FUZZ_BIN)/$(FUZZ) -max_total_time=$(SECS) -print_final_stats=1 \
	  -artifact_prefix=fuzz/artifacts/$(FUZZ)/ fuzz/corpus/$(FUZZ) \
	  fuzz/corpus/min/$(FUZZ) $(FUZZ_SEEDS)

fuzz-all: fuzz-build
	for t in $(FUZZ_TARGETS); do \
	  $(MAKE) --no-print-directory fuzz FUZZ=$$t SECS=$(SECS) || exit 1; \
	done

# fuzz/corpus/min/<target> keeps only inputs that add coverage: libFuzzer's
# merge copies an input in when it reaches code the folder doesn't yet, so
# the folder grows with coverage, not with fuzzing time. Nothing is deleted.
fuzz-merge: fuzz-build
	for t in $(FUZZ_TARGETS); do \
	  mkdir -p fuzz/corpus/$$t fuzz/corpus/min/$$t && \
	  $(FUZZ_BIN)/$$t -merge=1 fuzz/corpus/min/$$t fuzz/corpus/$$t $(FUZZ_SEEDS) 2>&1 | \
	    grep -E "MERGE-OUTER: [0-9]+ new files" || exit 1; \
	done

# Replays the minimal corpus once under Valgrind's memcheck, which sees what
# a panic can't: reads and writes outside an allocation, or of freed or
# uninitialised memory, in any code, dependencies' unsafe code included
# (AddressSanitizer needs nightly). The replay uses its own build without
# coverage instrumentation: libFuzzer's comparison hooks copy whole
# registers, padding included, which Valgrind reports as uninitialised.
# On Arch, Valgrind fetches glibc's symbols through debuginfod (the system
# must be up to date); on Debian/Ubuntu install libc6-dbg.
VALGRIND       ?= valgrind
VALGRIND_FLAGS ?= -q --error-exitcode=99 --exit-on-first-error=yes --leak-check=no
REPLAY_TARGET_DIR := $(FUZZ_TARGET_DIR)/replay
REPLAY_BIN     := $(REPLAY_TARGET_DIR)/$(FUZZ_TRIPLE)/release
fuzz-valgrind: fuzz-merge
	RUSTFLAGS="--cfg fuzzing -Cdebug-assertions -Coverflow-checks $(RUSTFLAGS)" \
	  CARGO_TARGET_DIR=$(REPLAY_TARGET_DIR) \
	  $(CARGO) build --release $(CARGO_FLAGS) --manifest-path fuzz/Cargo.toml \
	  --target $(FUZZ_TRIPLE) --bins
	for t in $(FUZZ_TARGETS); do \
	  echo "== $$t under valgrind: $$(ls fuzz/corpus/min/$$t | wc -l) inputs" && \
	  $(VALGRIND) $(VALGRIND_FLAGS) $(REPLAY_BIN)/$$t -runs=0 fuzz/corpus/min/$$t || exit 1; \
	done

fuzz-repro: fuzz-build
	@test -n "$(CRASH)" || { echo "usage: make fuzz-repro FUZZ=splice CRASH=fuzz/artifacts/splice/crash-..." >&2; exit 1; }
	$(FUZZ_BIN)/$(FUZZ) "$(CRASH)"

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
