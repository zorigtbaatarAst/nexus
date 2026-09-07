#!/usr/bin/env bash
# Is the `nexus` inside the benchmark image the `nexus` this tree builds? Compared by content.
#
# This exists because the answer was once "no" for 95 paid runs. `nexus-bench:latest` was built
# 2026-09-05 17:11; the lexical fallback landed 2026-09-06 17:42. The sweep was started as
# `bash scripts/eval/sweep.sh` (which does not rebuild) rather than `make bench` (which does),
# and the A1 arm was measured with code a day older than the tree the result was reported
# against. See docs/eval/tier2-result.md, "Correction: the arm ran a stale binary".
#
# Nothing caught it because the only provenance stamped was `nexus --version` — read off the
# HOST binary at that — and the version had not been bumped between the two commits, so both
# read `nexus 0.3.0`. A version string is a claim about a binary; a hash IS the binary. Only
# the hash can tell two builds of the same version apart, which is the entire failure mode.
#
# The image's copy is exact — scripts/eval/Dockerfile does
# `COPY target/release/nexus /usr/local/bin/nexus`, no strip, no rebuild — so equal bytes is
# the correct bar, not an approximation of one.
#
# Prints the image binary's sha256 on stdout for the caller to stamp. Exit 0 = same binary.
#
# There is deliberately no override. The refusal is only reachable by having rebuilt the tool
# and not the image, which is exactly the state in which no sweep should start, and the fix is
# one named command. An escape hatch here would be used by the same operator, in the same
# hurry, as the one who ran sweep.sh directly.
set -euo pipefail

IMAGE="${1:?usage: check_image_binary.sh <image> <tree-binary>}"
BIN="${2:?usage: check_image_binary.sh <image> <tree-binary>}"

[ -x "$BIN" ] || { echo "check_image_binary.sh: $BIN missing; run make release first" >&2; exit 1; }

# No pipes: a piped exit status is the pipe's, and `docker run` failing into `cut` would yield
# an empty hash with status 0 — a comparison that cannot fail is this whole branch's defect.
TREE_SUM="$(sha256sum "$BIN")"
IMAGE_SUM="$(docker run --rm --entrypoint sha256sum "$IMAGE" /usr/local/bin/nexus)" || {
  echo "check_image_binary.sh: could not read /usr/local/bin/nexus from $IMAGE" >&2
  echo "Build it with: make bench-image" >&2
  exit 1
}
TREE_SHA="${TREE_SUM%% *}"
IMAGE_SHA="${IMAGE_SUM%% *}"

if [ "$IMAGE_SHA" != "$TREE_SHA" ]; then
  echo "check_image_binary.sh: $IMAGE does not contain this tree's nexus." >&2
  echo "  image  /usr/local/bin/nexus   $IMAGE_SHA" >&2
  echo "  tree   $BIN   $TREE_SHA" >&2
  echo "Refusing to start a sweep that would be reported against code it did not run." >&2
  echo "This happened once already: 95 runs, \$37.62, an image a day older than the tree," >&2
  echo "and both binaries answering 'nexus 0.3.0' to --version." >&2
  echo "Fix it with:  make bench         (rebuilds the image, then sweeps)" >&2
  echo "         or:  make bench-image   (rebuild the image only)" >&2
  exit 1
fi

echo "$IMAGE_SHA"
