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

# The tool under test, built on the host and copied in — never built here, so every run uses
# a byte-identical binary and the image does not need a Rust toolchain.
COPY target/release/nexus /usr/local/bin/nexus

RUN npm install -g @anthropic-ai/claude-code@2.1.261

# The UserPromptSubmit hook helper, part of the pinned run environment. See the file itself for
# why the arms cannot just call `nexus context --task "$CLAUDE_USER_PROMPT"`.
COPY scripts/eval/nexus-hook.sh /usr/local/bin/nexus-hook

# How a fixture is built and tested — one definition, shared by the pre-warm below and by the
# grader's L0.
COPY scripts/eval/build.sh /usr/local/bin/fixture-build

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
COPY target/fixtures /verify
RUN set -eux; \
    for m in /verify/*.manifest.json; do fixture-build "${m%.manifest.json}"; done; \
    rm -rf /verify /root/.gradle/daemon

WORKDIR /work
