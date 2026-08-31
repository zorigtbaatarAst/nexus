BIN     := target/release/bughunter
PREFIX  ?= $(HOME)/.local

.PHONY: help build release test lint fmt check install uninstall clean demo smoke

help:
	@echo "build    debug build"
	@echo "release  optimized single binary -> $(BIN)"
	@echo "test     unit tests + architecture boundary tests"
	@echo "lint     clippy, warnings denied"
	@echo "check    fmt + lint + test — what CI runs"
	@echo "install  copy the binary to $(PREFIX)/bin"
	@echo "smoke    scan a public Spring repo and assert the cascade works"
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
	@case ":$$PATH:" in *":$(PREFIX)/bin:"*) ;; \
	  *) echo "note: $(PREFIX)/bin is not on your PATH — add it:"; \
	     echo "  export PATH=\"$(PREFIX)/bin:\$$PATH\"" ;; esac

uninstall:
	rm -f $(PREFIX)/bin/bughunter
	@echo "removed $(PREFIX)/bin/bughunter"
	@echo "project data in each repository's .nexus/ was left alone"

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

# What CI runs: index a real repository, then assert a no-op rescan reports nothing.
smoke: release
	@repo=$$(mktemp -d); out=$$(mktemp -d); \
	git clone --depth 1 -q https://github.com/spring-projects/spring-petclinic.git $$repo; \
	$(CURDIR)/$(BIN) --project $$repo scan   --json > $$out/scan.json; \
	$(CURDIR)/$(BIN) --project $$repo rescan --json > $$out/rescan.json; \
	python3 scripts/check_smoke.py $$out/scan.json $$out/rescan.json; \
	rm -rf $$repo $$out
