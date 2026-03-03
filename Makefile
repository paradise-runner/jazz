PORT ?= 7705
BIND ?= 0.0.0.0

.PHONY: build serve stop install-macos install-linux uninstall-macos uninstall-linux release

build:
	cargo build --release

serve: build
	./target/release/jazz --port $(PORT) --bind $(BIND) &
	@echo "Jazz running on http://$(BIND):$(PORT)"

stop:
	@lsof -ti :$(PORT) | xargs kill 2>/dev/null && echo "Stopped server on port $(PORT)" || echo "No server running on port $(PORT)"

install-macos: build
	@mkdir -p ~/Library/LaunchAgents
	@sed 's|__BINARY__|$(shell pwd)/target/release/jazz|g; s|__PORT__|$(PORT)|g' service/com.jazz.daemon.plist > ~/Library/LaunchAgents/com.jazz.daemon.plist
	launchctl load ~/Library/LaunchAgents/com.jazz.daemon.plist
	@echo "Jazz installed as macOS LaunchAgent"

uninstall-macos:
	launchctl unload ~/Library/LaunchAgents/com.jazz.daemon.plist 2>/dev/null || true
	rm -f ~/Library/LaunchAgents/com.jazz.daemon.plist
	@echo "Jazz macOS LaunchAgent removed"

install-linux: build
	@sudo cp service/jazz.service /etc/systemd/user/jazz.service
	@sed -i 's|__BINARY__|$(shell pwd)/target/release/jazz|g; s|__PORT__|$(PORT)|g' /etc/systemd/user/jazz.service
	systemctl --user daemon-reload
	systemctl --user enable --now jazz
	@echo "Jazz installed as systemd user service"

uninstall-linux:
	systemctl --user disable --now jazz 2>/dev/null || true
	sudo rm -f /etc/systemd/user/jazz.service
	systemctl --user daemon-reload
	@echo "Jazz systemd service removed"

release:
	@if [ -z "$(VERSION)" ]; then echo "Usage: make release VERSION=0.2.0"; exit 1; fi
	@sed -i '' 's/^version = ".*"/version = "$(VERSION)"/' Cargo.toml 2>/dev/null || sed -i 's/^version = ".*"/version = "$(VERSION)"/' Cargo.toml
	git add -A
	git commit -m "release v$(VERSION)"
	git tag -a "v$(VERSION)" -m "v$(VERSION)"
	git push origin main --tags
	@echo "Tagged v$(VERSION) — GitHub Actions will build and release"
