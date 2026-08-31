BIN     := target/release/bughunter
PREFIX  ?= $(HOME)/.local

.PHONY: help build release test lint fmt check install clean demo

help:
	@echo "build    debug build"
	@echo "release  optimized single binary -> $(BIN)"
	@echo "test     unit tests + architecture boundary tests"
	@echo "lint     clippy, warnings denied"
	@echo "check    fmt + lint + test — what CI runs"
	@echo "install  copy the binary to $(PREFIX)/bin"
	@echo "demo     scan a repository and prove the incremental cascade (REPO=path)"

build:
	cargo build

release:
	cargo build --release

test:
	cargo test --workspace

lint:
	cargo clippy --workspace --all-targets -- -D warnings

fmt:
	cargo fmt --all

check: fmt lint test

install: release
	install -Dm755 $(BIN) $(PREFIX)/bin/bughunter
	@echo "installed to $(PREFIX)/bin/bughunter"

clean:
	cargo clean

# Prove the claim end to end on a real repository, without touching it:
#   make demo REPO=/path/to/a/java/project
REPO ?= .
demo: release
	@tmp=$$(mktemp -d) && git clone -q $(REPO) $$tmp && cd $$tmp && \
	  $(CURDIR)/$(BIN) init && $(CURDIR)/$(BIN) scan && \
	  echo && echo "--- rescan with nothing changed ---" && $(CURDIR)/$(BIN) rescan && \
	  echo && echo "--- rescan after reformatting every Java file ---" && \
	  find . -name '*.java' -exec sed -i 's/^\(\s*\)/\1\1/' {} + && \
	  $(CURDIR)/$(BIN) rescan && rm -rf $$tmp
