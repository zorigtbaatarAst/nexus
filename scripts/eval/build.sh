#!/usr/bin/env bash
# Build a generated fixture and run its tests, offline. Installed into the run image as
# `fixture-build`, so the grader's L0 and the image's own pre-warm check are the same command
# and cannot drift apart.
#
# Offline on purpose: `mvn -o`, `gradle --offline`, `npm ci --offline`. A benchmark run that
# reaches Maven Central measures Maven Central, and one that reaches it *sometimes* measures
# the weather. Everything these need is warmed into the image at build time.
#
# Detection is by build file rather than by fixture name — a table of names here would be a
# second place the corpus is described, and the two would eventually disagree.
set -euo pipefail

cd "${1:-.}"
FOUND=0

# -DfailIfNoTests=true because a Maven module with no tests passes `test` in a way that looks
# exactly like success. This fixture corpus has tests in every module; losing them should be
# a red build, not a green one.
if [ -f pom.xml ]; then
  FOUND=1
  mvn -B -o -DfailIfNoTests=true test
fi

# next-storefront: two builds, one repository. Neither compiler sees the other side, which is
# the property Family B measures.
if [ -f api/pom.xml ]; then
  FOUND=1
  ( cd api && mvn -B -o -DfailIfNoTests=true test )
fi

if [ -f settings.gradle ]; then
  FOUND=1
  gradle --offline --no-daemon --console=plain test
fi

if [ -f web/package.json ]; then
  FOUND=1
  ( cd web && npm ci --offline --no-audit --no-fund && npm run build && npm test )
fi

# A fixture with no build system recognised would otherwise exit 0 here and be graded as a
# passing build, which is the failure this whole script exists to prevent.
if [ "$FOUND" -eq 0 ]; then
  echo "fixture-build: no pom.xml, settings.gradle or web/package.json under $(pwd)" >&2
  exit 1
fi
