#!/usr/bin/env bash
# Pin every source file in a tree to one fixed, old timestamp.
#
#   bench_mtime.sh <tree>
#
# Cargo decides whether a crate needs rebuilding by comparing source mtimes against the
# artifacts in its target directory. A `git clone && git checkout` stamps every file with the
# time of the checkout, which is always *newer* than the image's pre-warmed target directory —
# so a fresh clone of the same commit that was warmed rebuilds the whole workspace anyway and
# the warm image buys nothing. Measured on tokio at `--features full`: ~20 s a build, twice a
# cell, ~75 cells.
#
# Pinning the tree to a timestamp older than the image makes the warm artifacts win, and the
# comparison stays honest because it is a comparison, not a bypass: anything written *after*
# this runs — the agent's edits, `git apply` of the diff being graded — carries a current mtime
# and is rebuilt. That ordering is the whole contract, and it is why this is called before the
# patch is applied and never after.
#
# Installed into the run image as `bench-mtime` so the pre-warm and the two callers on the host
# share one definition of "old". Three copies of an epoch constant is three chances for the
# pre-warm and the runs to disagree about which side of it a file falls on.
set -euo pipefail

# 2020-09-13. Any date comfortably before the image is built will do; it is pinned rather than
# computed so that two images built weeks apart normalise to the same instant.
EPOCH=1600000000

TREE="${1:?usage: bench_mtime.sh <tree>}"
[ -d "$TREE" ] || { echo "bench_mtime.sh: $TREE is not a directory" >&2; exit 1; }

# .git is pruned: it is not a build input, it holds tens of thousands of loose objects in a
# fresh clone, and rewriting their mtimes is the slowest part of the walk by an order of
# magnitude. -h so a symlink's own timestamp moves rather than its target's.
find "$TREE" -name .git -prune -o -print0 | xargs -0 --no-run-if-empty touch -h -d "@$EPOCH"
