#!/usr/bin/env bash
# Does `fixture-build` actually go red?
#
# `mvn test` on a project with no tests exits 0 and looks exactly like success; so does
# `gradle test` when the test sources are gone, because the task becomes NO-SOURCE rather
# than empty. A grader built on either would score every run as passing. This is the check
# that keeps `build.sh` honest: it breaks a fixture on purpose, one way per build system per
# stage, and asserts the exit code moves.
#
# Needs docker, the run image and `make fixtures`. Never part of `make check` — it is minutes
# of containers, and it is what you run after touching build.sh or a fixture's build files.
#
#   scripts/eval/build_selftest.sh [case-name-substring]
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
IMAGE="${IMAGE:-nexus-bench:latest}"
FILTER="${1:-}"
LOGS="$(mktemp -d)"
fails=0

# name | repo | commit | expected(pass|fail) | mutation
CASES=$(cat <<'EOF'
baseline-maven|spring-payments|c2|pass|true
baseline-node-and-maven|next-storefront|c2|pass|true
baseline-gradle|acme-monorepo|c2|pass|true
maven-zero-tests|spring-payments|c2|fail|rm -rf src/test
node-zero-tests|next-storefront|c2|fail|rm web/src/lib/orders.test.ts
gradle-zero-tests|acme-monorepo|c2|fail|rm -rf libs/common/src/test
no-build-system|spring-payments|c2|fail|rm pom.xml
javac-syntax|spring-payments|c2|fail|sed -i 's/private final PaymentRepository repository;/private final PaymentRepository repository/' src/main/java/mn/pay/PaymentService.java
surefire-assertion|spring-payments|c2|fail|sed -i 's/new BigDecimal("0.00")/new BigDecimal("9.99")/' src/test/java/mn/pay/PaymentServiceTest.java
javac-cross-file|next-storefront|c2|fail|sed -i 's/public BigDecimal getTotalAmount()/public String getTotalAmount()/' api/src/main/java/mn/shop/api/Order.java
tsc-type|next-storefront|c2|fail|sed -i 's/  totalAmount: number;/  totalAmount: NoSuchType;/' web/src/lib/orders.ts
vitest-assertion|next-storefront|c2|fail|sed -i 's|expect(url).toBe("/graphql")|expect(url).toBe("/nope")|' web/src/lib/orders.test.ts
gradle-assertion|acme-monorepo|c2|fail|sed -i 's/throw new IllegalArgumentException/if (false) throw new IllegalArgumentException/' libs/common/src/main/java/mn/acme/common/Money.java
gradle-sibling-compile|acme-monorepo|c2|fail|sed -i 's/public Money total(List<OrderEntity> orders) {/public Money total(List<OrderEntity> orders) { syntax error here/' services/orders/src/main/java/mn/acme/orders/OrderService.java
EOF
)

while IFS='|' read -r name repo cid want mutation; do
  [ -n "$name" ] || continue
  case "$name" in *"$FILTER"*) ;; *) continue ;; esac

  sha=$(python3 -c "
import json
m=json.load(open('$ROOT/target/fixtures/$repo.manifest.json'))
print(next(c['sha'] for c in m['commits'] if c['id']=='$cid'))") || { echo "no such commit $repo $cid"; exit 1; }

  W=$(mktemp -d)
  git clone -q "$ROOT/target/fixtures/$repo" "$W/repo"
  git -C "$W/repo" checkout -q "$sha"
  ( cd "$W/repo" && eval "$mutation" )

  # --network=none: the build has to come out of the image's caches or not at all.
  docker run --rm --network=none -v "$W/repo:/work:Z" \
    -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" "$IMAGE" bash -lc '
      git config --global --add safe.directory /work
      cd /work; fixture-build .; rc=$?
      chown -R "$HOST_UID:$HOST_GID" /work 2>/dev/null || true; exit $rc
    ' > "$LOGS/$name.log" 2>&1
  rc=$?

  if { [ "$want" = pass ] && [ "$rc" -eq 0 ]; } || { [ "$want" = fail ] && [ "$rc" -ne 0 ]; }; then
    printf 'ok    %-24s %-16s %s exit=%s\n' "$name" "$repo" "$cid" "$rc"
  else
    printf 'FAIL  %-24s %-16s %s exit=%s (wanted %s) log=%s\n' \
      "$name" "$repo" "$cid" "$rc" "$want" "$LOGS/$name.log"
    fails=$((fails + 1))
  fi

  # The container ran as root, so some of what it wrote is not ours to delete.
  docker run --rm -v "$W:/w" alpine:3 sh -c 'rm -rf /w' >/dev/null 2>&1
  rm -rf "$W" 2>/dev/null
done <<< "$CASES"

echo
if [ "$fails" -ne 0 ]; then
  echo "$fails case(s) failed; logs under $LOGS" >&2
  exit 1
fi
echo "every case behaved as declared"
rm -rf "$LOGS"
