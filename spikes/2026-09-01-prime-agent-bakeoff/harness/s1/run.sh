#!/bin/sh
# usage: run.sh <scenario.mjs>...  (sequential; env set for the disposable Prime home)
S=${CG_S:?set CG_S to the disposable bake-off root}
export CG_S="$S" HOME="$S/bakeoff/prime/home" PRIME_AGENT_TELEMETRY=0 PRIME_AGENT_INSTALL_UV=0
unset PI_CODING_AGENT_DIR
cd "$S/bakeoff/s1"
for f in "$@"; do
  echo "########## $f"
  node "$f" </dev/null 2>&1; rc=$?
  echo "########## $f exit=$rc"; echo "$f exit=$rc $(grep -c '^\[.*PASS' "evidence/${f%.mjs}.log") pass $(grep -c '^\[.*FAIL' "evidence/${f%.mjs}.log") fail" >> evidence/summary.txt
done
