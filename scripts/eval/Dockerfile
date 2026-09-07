# The benchmark's run environment. Everything the five fixtures need to build and test, plus
# the agent and the tool under test. Pinned because a benchmark whose environment drifts
# measures the environment — so the base is pinned by digest, not by the floating 21-jdk tag,
# and the agent by an exact npm version.
FROM eclipse-temurin:21-jdk@sha256:85f00967bcc624fc19fa9c2cf124ea426a5363898e267141726f31f358c2e14b

# Maven, node and npm grade every run — they are the L0 build for four of the five fixture
# repositories — so they are pinned to exact archive versions rather than to whatever the
# Ubuntu archive holds on the day of a rebuild. The committed package-lock.json's guarantee
# is only as good as the npm that reads it.
#
# The failure mode of an exact pin is that a rebuild stops working once Ubuntu supersedes a
# package. That is the intended failure: a loud one-line version bump, made deliberately,
# beats an image that silently grades with a different toolchain than the one measured.
# git, curl, unzip, ca-certificates and sqlite3 are unpinned on purpose — nothing graded
# depends on their versions.
RUN apt-get update && apt-get install -y --no-install-recommends \
      git curl unzip ca-certificates sqlite3 \
      maven=3.9.12-1 \
      nodejs=22.22.1+dfsg+~cs22.19.15-1ubuntu1 \
      npm=9.2.0~ds3-1 \
    && rm -rf /var/lib/apt/lists/*

# Gradle comes from the distribution, not from apt: Ubuntu's `gradle` package is 4.4.1, which
# predates Java 9's module system and cannot run on the JDK 21 this image is built on. Pinned
# by version and verified by checksum.
ENV GRADLE_VERSION=8.10.2
RUN curl -fsSL -o /tmp/gradle.zip \
      "https://services.gradle.org/distributions/gradle-${GRADLE_VERSION}-bin.zip" \
 && echo "31c55713e40233a8303827ceb42ca48a47267a0ad4bab9177123121e71524c26  /tmp/gradle.zip" \
      | sha256sum -c - \
 && unzip -q /tmp/gradle.zip -d /opt \
 && ln -s "/opt/gradle-${GRADLE_VERSION}/bin/gradle" /usr/local/bin/gradle \
 && rm /tmp/gradle.zip

# Rust, for the tokio corpus (tests/fixtures/corpora/tokio). Pinned and checksummed like
# Gradle above, and installed from the standalone distribution rather than through rustup: one
# fewer moving version, and the same curl/sha256sum/unpack shape the rest of this file uses.
#
# This is NOT the toolchain the tool under test is built with — `nexus` is still built on the
# host and copied in below, so this image still cannot rebuild it and every run still uses a
# byte-identical binary. What it builds is the corpus.
#
# gcc and libc6-dev are rustc's linker and C runtime; nothing in the five tasks' dependency
# graphs needs anything further.
ENV RUST_VERSION=1.98.0
RUN apt-get update && apt-get install -y --no-install-recommends gcc libc6-dev xz-utils \
    && rm -rf /var/lib/apt/lists/* \
 && curl -fsSL -o /tmp/rust.tar.xz \
      "https://static.rust-lang.org/dist/rust-${RUST_VERSION}-x86_64-unknown-linux-gnu.tar.xz" \
 && echo "ed8ee2df70909c88cbaf87a6cfa3920dac00b537de12a6abe6906641e0f5952f  /tmp/rust.tar.xz" \
      | sha256sum -c - \
 && mkdir /tmp/rust && tar -xJf /tmp/rust.tar.xz -C /tmp/rust --strip-components=1 \
 && /tmp/rust/install.sh --prefix=/usr/local --components=rustc,cargo,rust-std-x86_64-unknown-linux-gnu \
 && rm -rf /tmp/rust /tmp/rust.tar.xz \
 && rustc --version && cargo --version

# The tool under test, built on the host and copied in — never built here, so every run uses
# a byte-identical binary and the image does not need a Rust toolchain of its own to do it.
COPY target/release/nexus /usr/local/bin/nexus

RUN npm install -g @anthropic-ai/claude-code@2.1.261

# The UserPromptSubmit hook helper, part of the pinned run environment. See the file itself for
# why the arms cannot just call `nexus context --task "$CLAUDE_USER_PROMPT"`.
COPY scripts/eval/nexus-hook.sh /usr/local/bin/nexus-hook

# How a fixture is built and tested — one definition, shared by the pre-warm below and by the
# grader's L0.
COPY scripts/eval/build.sh /usr/local/bin/fixture-build

# Pins a checked-out tree's mtimes below the pre-warm's, which is what makes the pre-warmed
# cargo target directory usable by a fresh clone at all. Shared by the pre-warm below and by
# run.sh/grade.sh on the host — see the file for why one definition of "old" matters.
COPY scripts/eval/bench_mtime.sh /usr/local/bin/bench-mtime

# --- the offline caches -------------------------------------------------------------------
#
# Every dependency the corpus needs, downloaded here and never during a run. `make fixtures`
# must have run on the host: the warm copy is the generated corpus itself, so the caches hold
# exactly what the runs ask for rather than what a hand-kept list of coordinates claims.
#
# This is the only place in the benchmark with network access to a package registry.
COPY target/fixtures /warm
RUN set -eux; \
    for p in /warm/spring-payments /warm/legacy-billing /warm/next-storefront/api; do \
      ( cd "$p" && mvn -B test ); \
    done; \
    ( cd /warm/acme-monorepo && gradle --no-daemon --console=plain test ); \
    ( cd /warm/next-storefront/web && npm ci --no-audit --no-fund ); \
    rm -rf /warm /root/.gradle/daemon

# The proof, on a clean copy: everything above builds and tests with no network at all. A
# failure here is a broken image, which is loud — the alternative is 75 runs that each score
# `passed: false` for a reason nothing in the results distinguishes from a bad agent.
#
# The loop is driven off the manifests, so it names the fixture repositories and not the
# `<name>.patches/` directories that sit beside them.
#
# A real-repository corpus is skipped here, by what its manifest says it is rather than by its
# name: `fixture-build` recognises pom.xml, settings.gradle and web/package.json, so a Cargo
# workspace would reach its "no build system recognised" refusal and fail the image build. That
# refusal is correct and stays — it is what keeps a fixture that lost its build file from
# grading green. The tokio corpus is proven by its own pre-warm below, which knows the crate
# and the feature flags that `fixture-build` deliberately does not.
COPY target/fixtures /verify
RUN set -eux; \
    for m in /verify/*.manifest.json; do \
      if grep -q '"role": "real-repository"' "$m"; then continue; fi; \
      fixture-build "${m%.manifest.json}"; \
    done; \
    rm -rf /verify /root/.gradle/daemon

# --- the tokio corpus: warm cargo, and prove every start state is red ------------------------
#
# The generated corpus above warms Maven, Gradle and npm from the fixtures. Cargo needs the
# same treatment and more of it: `cargo test -p tokio --features full` is 20-75 s from cold,
# twice per graded cell, ~75 cells. Warm, it is seconds.
#
# The checkout lands at /work — the same absolute path every run and every grading mounts —
# because cargo's fingerprints record absolute source paths. Warming it anywhere else would
# warm a directory no run uses.
#
# One target directory per task, not one shared: the five start states are five different
# commits of the same three crates, so a shared directory would have each task's warm-up
# invalidate the previous one's and the last task written would be the only warm one.
#
# The pre-warm script is generated beside the corpus by scripts/eval/tokio_fixture.sh, because
# it is the corpus that knows each task's start sha and its three cargo commands. It also
# ASSERTS the corpus: every start state must compile, its collateral tests must be green, and
# its own test must be RED. `docs/eval/tier2-corpus-verdict.md` is what a corpus that grades
# nothing costs, and this is the cheapest place to find out — a build failure instead of a
# sweep that reads as a result.
COPY target/fixtures/tokio /work
COPY target/fixtures/tokio.prewarm.sh /usr/local/share/tokio-prewarm.sh
RUN git config --global --add safe.directory /work \
 && bash /usr/local/share/tokio-prewarm.sh

# After the pre-warm, never before it: the warm-up resolves and downloads every dependency the
# five start states pin, and a run must not be able to reach a registry at all. grade.sh already
# runs `--network=none`; this is what makes the agent's own `cargo` calls in run.sh — which does
# have a network, for the API — fail loudly rather than silently pulling a different crate
# version than the one that was graded.
ENV CARGO_NET_OFFLINE=true

WORKDIR /work
