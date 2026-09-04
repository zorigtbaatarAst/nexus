#!/usr/bin/env bash
# Measure resolution accuracy against a SCIP oracle.
#
# Deliberately NOT part of `make check`: it needs external toolchains and, for Java, a full
# project compile. Wiring that into the commit path gets it disabled inside a fortnight, which
# docs/architecture/13-evaluation.md §2 says in as many words about Tier 2.
set -euo pipefail

PROJECT="${1:-.}"
OUT="${OUT:-target/eval}"
mkdir -p "$OUT"

case "${LANG_KIND:-rust}" in
  rust)
    command -v rust-analyzer >/dev/null || {
      echo "rust-analyzer not on PATH — install it (rustup component add rust-analyzer)." >&2
      echo "The harness refuses to run without an oracle: a missing indexer says nothing" >&2
      echo "about the resolver, and a score computed from no oracle is not a score." >&2
      exit 1
    }
    rust-analyzer scip "$PROJECT" --output "$OUT/index.scip"
    ORACLE="rust-analyzer $(rust-analyzer --version | tr -d '\n')"
    ;;
  java)
    command -v scip-java >/dev/null || { echo "scip-java not on PATH" >&2; exit 1; }
    # scip-java fails closed: a non-zero build exit propagates and `aggregate` never runs, so
    # index.scip is never written. The per-file .scip files survive in the targetroot, so a
    # partial index is recoverable — and the run reports itself partial rather than silently
    # scoring against half a project.
    if ! scip-java index --output "$OUT/index.scip"; then
      echo "build failed; recovering a partial index from the targetroot" >&2
      scip-java aggregate "$PROJECT/target/scip-targetroot" --output "$OUT/index.scip"
    fi
    ORACLE="scip-java"
    ;;
  *) echo "LANG_KIND must be rust or java" >&2; exit 2 ;;
esac

nexus --project "$PROJECT" scan >/dev/null
nexus --project "$PROJECT" graph --edges "$OUT/edges.ndjson" >/dev/null
nexus-eval --edges "$OUT/edges.ndjson" --scip "$OUT/index.scip" --oracle "$ORACLE" "${@:2}"
