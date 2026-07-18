APP         := ScreenshotR
BIN         := screenshotr
BUNDLE_ID   := com.keithsimon.screenshotr
SIGN_ID     ?= Apple Development: Keith Simon (H68H9Z4PD3)

VERSION     := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
ARCH        := $(shell uname -m)

DIST        := dist/$(APP).app
INSTALL_DIR := $(HOME)/Applications
APP_PATH    := $(INSTALL_DIR)/$(APP).app
PLIST       := $(HOME)/Library/LaunchAgents/$(BUNDLE_ID).plist
TOKEN_FILE  := $(HOME)/.config/screenshotr/token
UID         := $(shell id -u)

PKG_NAME    := screenshotr-$(VERSION)-$(ARCH)
PKG_DIR     := dist/$(PKG_NAME)
PKG_TAR     := dist/$(PKG_NAME).tar.gz

.PHONY: all build bundle sign verify token install load unload logs status uninstall clean dist

all: bundle

build:
	cargo build --release

bundle: build
	rm -rf $(DIST)
	mkdir -p $(DIST)/Contents/MacOS
	cp packaging/Info.plist $(DIST)/Contents/Info.plist
	cp target/release/$(BIN) $(DIST)/Contents/MacOS/$(BIN)
	$(MAKE) --no-print-directory sign

# Signing runs last and unconditionally. It must be last because the signature
# seals Info.plist; it must be unconditional because a single ad-hoc build under
# this bundle ID poisons the TCC row and costs the Screen Recording grant.
sign:
	codesign --force --options runtime \
	  --sign "$(SIGN_ID)" \
	  --identifier $(BUNDLE_ID) \
	  $(DIST)
	@$(MAKE) --no-print-directory verify

# Guard rail. An ad-hoc signature produces a designated requirement that is a
# literal cdhash, which changes every build and silently resets the TCC grant.
# A cert-backed DR pins identifier + certificate chain and survives rebuilds.
verify:
	@codesign --verify --strict --verbose=2 $(DIST)
	@if codesign -d -r- $(DIST) 2>&1 | grep -q 'cdhash'; then \
	  echo "FATAL: ad-hoc designated requirement — TCC grant will reset every build"; \
	  exit 1; \
	else \
	  echo "OK: certificate-based designated requirement"; \
	fi
	@codesign -d -r- $(DIST) 2>&1 | grep designated || true

token:
	@mkdir -p $(dir $(TOKEN_FILE))
	@if [ ! -s $(TOKEN_FILE) ]; then \
	  LC_ALL=C tr -dc 'A-Za-z0-9' < /dev/urandom | head -c 48 > $(TOKEN_FILE); \
	  echo "generated new token"; \
	fi
	@chmod 600 $(TOKEN_FILE)
	@echo "token file: $(TOKEN_FILE)"

install: bundle token
	mkdir -p $(INSTALL_DIR)
	rm -rf $(APP_PATH)
	cp -R $(DIST) $(APP_PATH)
	sed -e 's|@BUNDLE_ID@|$(BUNDLE_ID)|g' \
	    -e 's|@APP_PATH@|$(APP_PATH)|g' \
	    -e 's|@HOME@|$(HOME)|g' \
	    packaging/launchagent.plist.in > $(PLIST)
	plutil -lint $(PLIST)
	$(MAKE) --no-print-directory load

load:
	-launchctl bootout gui/$(UID)/$(BUNDLE_ID) 2>/dev/null
	launchctl bootstrap gui/$(UID) $(PLIST)
	launchctl kickstart -k gui/$(UID)/$(BUNDLE_ID)
	@echo "loaded $(BUNDLE_ID)"

unload:
	-launchctl bootout gui/$(UID)/$(BUNDLE_ID)

status:
	@launchctl print gui/$(UID)/$(BUNDLE_ID) 2>/dev/null | grep -E "state|pid|last exit" || echo "not loaded"

logs:
	tail -f $(HOME)/Library/Logs/screenshotr.log $(HOME)/Library/Logs/screenshotr.err.log

uninstall: unload
	rm -f $(PLIST)
	rm -rf $(APP_PATH)
	@echo "removed app and agent; token left at $(TOKEN_FILE)"

# Redistributable tarball: signed .app plus a standalone installer.
dist: bundle
	rm -rf $(PKG_DIR) $(PKG_TAR)
	mkdir -p $(PKG_DIR)
	cp -R $(DIST) $(PKG_DIR)/$(APP).app
	cp packaging/install.sh packaging/uninstall.sh $(PKG_DIR)/
	chmod +x $(PKG_DIR)/install.sh $(PKG_DIR)/uninstall.sh
	cp README.md $(PKG_DIR)/
	# COPYFILE_DISABLE stops bsdtar from emitting AppleDouble ._ files, which
	# would land inside the bundle on extraction and break signature checks.
	COPYFILE_DISABLE=1 tar -czf $(PKG_TAR) -C dist $(PKG_NAME)
	rm -rf $(PKG_DIR)
	@echo
	@echo "package: $(PKG_TAR) ($$(du -h $(PKG_TAR) | cut -f1))"
	@shasum -a 256 $(PKG_TAR)
	@echo
	@echo "Copy to the target Mac WITHOUT quarantine (scp/rsync keeps it clean):"
	@echo "  scp $(PKG_TAR) <host>:~/"
	@echo "  ssh <host> 'tar xzf $(PKG_NAME).tar.gz && ./$(PKG_NAME)/install.sh'"

clean:
	cargo clean
	rm -rf dist
