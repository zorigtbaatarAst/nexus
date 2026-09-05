# The benchmark's run environment. Everything the five fixtures need to build and test, plus
# the agent and the tool under test. Pinned because a benchmark whose environment drifts
# measures the environment — so the base is pinned by digest, not by the floating 21-jdk tag,
# and the agent by an exact npm version.
FROM eclipse-temurin:21-jdk@sha256:85f00967bcc624fc19fa9c2cf124ea426a5363898e267141726f31f358c2e14b

RUN apt-get update && apt-get install -y --no-install-recommends \
      git curl unzip ca-certificates maven nodejs npm sqlite3 \
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

WORKDIR /work
