BIN     := target/release/nexus
CAP_BIN := target/release/bughunter
PREFIX  ?= $(HOME)/.local

.PHONY: help build release test lint fmt check eval-selftest install uninstall clean demo smoke \
        fixtures fixtures-verify bench-image bench

help:
	@echo "build    debug build"
	@echo "release  optimized binaries -> nexus and bughunter"
	@echo "test     unit tests + architecture boundary tests"
	@echo "lint     clippy, warnings denied"
	@echo "check    fmt + lint + analyse.py self-test + test — what CI runs"
	@echo "eval-selftest  the benchmark analysis' own statistics self-test (stdlib, ~0.2s)"
	@echo "install  copy the binary to $(PREFIX)/bin"
	@echo "smoke    scan a public Spring repo and assert the cascade works"
	@echo "demo     scan a repository and prove the incremental cascade (REPO=path)"
	@echo "fixtures         build the benchmark corpus -> target/fixtures"
	@echo "fixtures-verify  prove the corpus is reproducible (CI gate)"
	@echo "bench-image      build the benchmark run image, with its offline caches warmed"
	@echo "bench            Tier 2: 95 agent runs in containers (costs money, takes hours)"
	@echo "eval             measure resolution accuracy against a SCIP oracle (needs an indexer)"

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

# scripts/eval/analyse.py's --self-test is the only guard on the numbers a ship-or-delete
# decision is read from, and until now no target ran it: `make check` was vacuous for that file.
# Pure stdlib, ~0.2s, no build — it belongs in front of the compile so a broken statistic is
# reported in the first second rather than the tenth minute.
eval-selftest:
	python3 scripts/eval/analyse.py --self-test

check: fmt lint eval-selftest test

# The benchmark corpus of docs/architecture/13-evaluation.md §3. Written under target/
# because it is already git-ignored: a generated repository inside the working tree would be
# walked by Nexus's own scan, and a fixture that plants a bug on purpose has no business
# turning up in its author's findings.
fixtures:
	cargo run --quiet --bin nexus -- fixture generate --force

# The determinism gate. Generates every fixture twice and fails if any sha moved.
#
# Cheap enough for every CI run, and worth it: a corpus that drifts makes every measurement
# taken against it a measurement of the corpus rather than of Nexus.
fixtures-verify:
	cargo run --quiet --bin nexus -- fixture verify

# The benchmark's run image. Both prerequisites are real: the Dockerfile copies the host-built
# binary in, and it warms its offline dependency caches from the generated corpus. Never part
# of `make check` — it downloads a JDK image and a few hundred megabytes of dependencies.
IMAGE ?= nexus-bench:latest
bench-image: release fixtures
	docker build -t $(IMAGE) -f scripts/eval/Dockerfile .

# The Tier 2 benchmark: 95 agent runs in containers, real money, hours. Never part of
# `make check` — see docs/architecture-decisions.md and .superpowers/sdd/2026-09-04-tier2-benchmark/.
# sweep.sh is resumable and gates on scripts/eval/test_grade.sh before spending anything, so
# re-running this after an interruption is the expected way to finish a sweep, not a mistake —
# but only with the SAME stamp. Without one, sweep.sh mints a fresh stamp, starts a new run
# tree, and pays for every cell that was already done:
#
#   make bench STAMP=<the stamp the first invocation printed>
#
# An unset STAMP expands to empty, so a first run still mints its own.
bench: bench-image
	STAMP="$(STAMP)" ./scripts/eval/sweep.sh

install: release
	install -Dm755 $(BIN) $(PREFIX)/bin/nexus
	install -Dm755 $(CAP_BIN) $(PREFIX)/bin/bughunter
	@echo "installed $(PREFIX)/bin/nexus and $(PREFIX)/bin/bughunter"
	@case ":$$PATH:" in *":$(PREFIX)/bin:"*) ;; \
	  *) echo "note: $(PREFIX)/bin is not on your PATH — add it:"; \
	     echo "  export PATH=\"$(PREFIX)/bin:\$$PATH\"" ;; esac

uninstall:
	rm -f $(PREFIX)/bin/nexus $(PREFIX)/bin/bughunter
	@echo "removed $(PREFIX)/bin/nexus and $(PREFIX)/bin/bughunter"
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

# Measure resolution accuracy against a SCIP oracle. Needs an external indexer, so it is
# never part of `make check`:  make eval [REPO=/path] [LANG_KIND=rust|java]
eval:
	@cargo build --release --bin nexus --bin nexus-eval
	@PATH="$(CURDIR)/target/release:$$PATH" ./scripts/eval.sh $(REPO)
.PHONY: eval

# What CI runs: index a real repository, then assert a no-op rescan reports nothing.
smoke: release
	@repo=$$(mktemp -d); out=$$(mktemp -d); \
	git clone --depth 1 -q https://github.com/spring-projects/spring-petclinic.git $$repo; \
	$(CURDIR)/$(BIN) --project $$repo scan   --json > $$out/scan.json; \
	$(CURDIR)/$(BIN) --project $$repo rescan --json > $$out/rescan.json; \
	python3 scripts/check_smoke.py $$out/scan.json $$out/rescan.json; \
	rm -rf $$repo $$out
