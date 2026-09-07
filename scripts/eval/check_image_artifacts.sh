#!/usr/bin/env bash
# Is everything the benchmark image bakes from this tree still this tree's copy? By content.
#
# This exists because the answer was once "no" for 95 paid runs. `nexus-bench:latest` was built
# 2026-09-05 17:11; the lexical fallback landed 2026-09-06 17:42. The sweep was started as
# `bash scripts/eval/sweep.sh` (which does not rebuild) rather than `make bench` (which does),
# and the A1 arm was measured with code a day older than the tree the result was reported
# against. See docs/eval/tier2-result.md, "Correction: the arm ran a stale binary".
#
# Nothing caught it because the only provenance stamped was `nexus --version` — read off the
# HOST binary at that — and the version had not been bumped between the two commits, so both
# read `nexus 0.3.0`. A version string is a claim about a file; a hash IS the file. Only the
# hash can tell two builds of the same version apart, which is the entire failure mode.
#
# All three are exact copies — scripts/eval/Dockerfile COPYs each in with no rewrite, no strip
# and no rebuild — so equal bytes is the correct bar, not an approximation of one.
#
# `bench-mtime` joined the table with the tokio corpus. It decides which files cargo considers
# stale, so drift there does not change a grade's *value* but can change whether the thing
# graded is the agent's edit or the image's pre-warmed copy of it — a silent false green, and
# the reason it is compared rather than trusted.
#
# `nexus` is not the only one that matters, and is not the worst one. `fixture-build` IS the L0
# build-and-grade path: drift there changes *grades*, where drift in `nexus` only changes the
# context an arm is given. When this check was first written it covered `nexus` alone, and the
# review that widened it found `build.sh` already drifted on the machine it ran on — the same
# failure with a different file name, and one that would have printed a clean provenance line.
#
# NOT covered, and this is a statement about the image rather than an omission: `target/fixtures`.
# The Dockerfile COPYs the corpus to /warm and /verify, uses it to warm the offline dependency
# caches and to prove the fixtures build, and `rm -rf`s both inside the same RUN layers. The
# final image therefore contains no copy of the corpus at all — verified: /warm and /verify do
# not exist — and there is nothing to hash it against. A run does not use one either: run.sh
# clones each fixture from the HOST's target/fixtures at run time (run.sh:57). What the image
# does retain is ~/.m2, ~/.gradle and node_modules warmed from whatever corpus built it, which
# is a real staleness but not a content comparison any path here can make: no file in the image
# corresponds to a file in the tree. Regenerating the corpus mid-sweep is the live hazard there,
# and it is flagged where it bites, at sweep.sh's resume guard.
#
# Prints "<image path> <sha256>" per artifact on stdout for the caller to stamp. Exit 0 = the
# image is this tree's.
#
# There is deliberately no override. The refusal is only reachable by having rebuilt something
# and not the image, which is exactly the state in which no sweep should start, and the fix is
# one named command. An escape hatch here would be used by the same operator, in the same hurry,
# as the one who ran sweep.sh directly.
set -euo pipefail

IMAGE="${1:?usage: check_image_artifacts.sh <image> <tree-root>}"
TREE="${2:?usage: check_image_artifacts.sh <image> <tree-root>}"

# Every COPY in scripts/eval/Dockerfile that takes a file from the tree: image path, tree path.
# Adding a COPY there means adding a line here — that is the intended coupling, and the reason
# this table is a table rather than three inline comparisons.
ARTIFACTS="
/usr/local/bin/nexus         target/release/nexus
/usr/local/bin/nexus-hook    scripts/eval/nexus-hook.sh
/usr/local/bin/fixture-build scripts/eval/build.sh
/usr/local/bin/bench-mtime   scripts/eval/bench_mtime.sh
"

IMAGE_PATHS=""
while read -r image_path tree_path; do
  [ -n "$image_path" ] || continue
  [ -f "$TREE/$tree_path" ] || {
    echo "check_image_artifacts.sh: $TREE/$tree_path missing; run make release first" >&2
    exit 1
  }
  IMAGE_PATHS="$IMAGE_PATHS $image_path"
done <<<"$ARTIFACTS"

# No pipes: a piped exit status is the pipe's, and `docker run` failing into `cut` would yield
# an empty hash with status 0 — a comparison that cannot fail is this whole branch's defect.
# One container for all of them; sha256sum takes several operands.
# shellcheck disable=SC2086  # IMAGE_PATHS is a deliberate word-split list of literal paths
IMAGE_SUMS="$(docker run --rm --entrypoint sha256sum "$IMAGE" $IMAGE_PATHS)" || {
  echo "check_image_artifacts.sh: could not read $IMAGE_PATHS from $IMAGE" >&2
  echo "Build it with: make bench-image" >&2
  exit 1
}

# Every mismatch is reported, not just the first: an operator who rebuilds for one file and is
# then refused for the next has been made to do the same work twice.
MISMATCHES=""
STAMP_LINES=""
while read -r image_path tree_path; do
  [ -n "$image_path" ] || continue
  image_sha="$(printf '%s\n' "$IMAGE_SUMS" | awk -v p="$image_path" '$2 == p { print $1 }')"
  [ -n "$image_sha" ] || { echo "check_image_artifacts.sh: no hash for $image_path in image" >&2; exit 1; }
  tree_sum="$(sha256sum "$TREE/$tree_path")"
  tree_sha="${tree_sum%% *}"
  STAMP_LINES="$STAMP_LINES$image_path $image_sha
"
  if [ "$image_sha" != "$tree_sha" ]; then
    MISMATCHES="$MISMATCHES  $tree_path
    image  $image_path  $image_sha
    tree   $TREE/$tree_path  $tree_sha
"
  fi
done <<<"$ARTIFACTS"

if [ -n "$MISMATCHES" ]; then
  echo "check_image_artifacts.sh: $IMAGE was not built from this tree." >&2
  printf '%s' "$MISMATCHES" >&2
  echo "Refusing to start a sweep that would be reported against files it did not run." >&2
  echo "This happened once already: 95 runs, \$37.62, an image a day older than the tree," >&2
  echo "and both binaries answering 'nexus 0.3.0' to --version." >&2
  echo "Fix it with:  make bench         (rebuilds the image, then sweeps)" >&2
  echo "         or:  make bench-image   (rebuild the image only)" >&2
  exit 1
fi

printf '%s' "$STAMP_LINES"
