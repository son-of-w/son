# son-of-w: build, test and install the `son` binary.
#
#   make            show this list
#   make install    build the release binary and put `son` on your PATH

CARGO ?= cargo
BIN   := son

.DEFAULT_GOAL := help
.PHONY: help build release test fmt lint check install uninstall clean dist publish

help: ## Show the targets
	@awk 'BEGIN {FS = ":.*## "} /^[a-z-]+:.*## / {printf "  make %-10s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

build: ## Build a debug binary (target/debug/son)
	$(CARGO) build --locked

release: ## Build the optimized binary (target/release/son)
	$(CARGO) build --release --locked

test: ## Run all tests
	$(CARGO) test --locked

fmt: ## Format the code
	$(CARGO) fmt

lint: ## Check formatting and run clippy, as CI does
	$(CARGO) fmt --check
	$(CARGO) clippy --all-targets --locked -- -D warnings

check: lint test ## Everything CI runs: format, clippy, tests

# With no PREFIX, `cargo install` puts the binary in ~/.cargo/bin.
# With one, the binary is copied to $(PREFIX)/bin:
#   make install PREFIX=/usr/local      (may need sudo)
install: ## Install son (PREFIX=/usr/local to choose where)
ifdef PREFIX
	$(CARGO) build --release --locked
	install -d "$(PREFIX)/bin"
	install -m 755 "target/release/$(BIN)" "$(PREFIX)/bin/$(BIN)"
	@echo "installed $(PREFIX)/bin/$(BIN)"
else
	$(CARGO) install --path . --locked
endif
	@echo "Next: run 'son init' to set son up for your agents."

uninstall: ## Remove the installed binary (same PREFIX as install)
ifdef PREFIX
	rm -f "$(PREFIX)/bin/$(BIN)"
else
	$(CARGO) uninstall son-of-w
endif

dist: ## Build both Mac binaries into dist/ for the current version
	scripts/release.sh package

publish: ## Check, build, tag v<version> and publish a GitHub release from this Mac
	scripts/release.sh

clean: ## Remove build output
	$(CARGO) clean
	rm -rf dist
