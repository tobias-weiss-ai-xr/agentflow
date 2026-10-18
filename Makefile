.PHONY: all build release build-release build-all install install-cargo uninstall clean test clippy fmt check compliance

# Binary names
MAIN_BIN := af
EXAMPLE_BIN := example_agent
# Install destinations
PREFIX ?= /usr/local
BINDIR ?= $(PREFIX)/bin

all: build

# Build in debug mode
build:
	cargo build

# Build in release mode
release: build-release
build-release:
	cargo build --release

# Install the release binary
install: build-release
	mkdir -p $(BINDIR)
	cp target/release/$(MAIN_BIN) $(BINDIR)/$(MAIN_BIN)
	chmod +x $(BINDIR)/$(MAIN_BIN)
	@echo "Installed $(MAIN_BIN) to $(BINDIR)/$(MAIN_BIN)"

# Install to cargo bin directory
install-cargo: build-release
	cargo install --path . --force
	@echo "Installed via cargo"

# Uninstall
uninstall:
	@if [ -f $(BINDIR)/$(MAIN_BIN) ]; then \
		rm -f $(BINDIR)/$(MAIN_BIN); \
		echo "Uninstalled $(MAIN_BIN) from $(BINDIR)"; \
	else \
		echo "$(MAIN_BIN) not found in $(BINDIR)"; \
	fi
	@if [ -f $$HOME/.cargo/bin/$(MAIN_BIN) ]; then \
		rm -f $$HOME/.cargo/bin/$(MAIN_BIN); \
		echo "Uninstalled $(MAIN_BIN) from $$HOME/.cargo/bin"; \
	else \
		echo "$(MAIN_BIN) not found in $$HOME/.cargo/bin"; \
	fi

# Build everything (main + example_agent)
build-all: build-release
	@echo "Building example_agent..."
	cargo build --release --bin example_agent

# Clean build artifacts
clean:
	cargo clean

# Run all tests
test:
	cargo test --all-targets

# Run clippy
clippy:
	cargo clippy --all-targets -- -D warnings

# Run cargo fmt
fmt:
	cargo fmt

# Full check (fmt + clippy + test)
check: fmt clippy test

# Compliance check (existing)
compliance:
	python3 scripts/compliance.py
