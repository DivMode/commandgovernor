#!/bin/sh
# Run the Command Governor conformance suite.
#
# Tier 1 is credential-free and must pass on every change; it is the gate on
# any re-pin of the substrate and on every merge (the CI harness job runs this
# script). It has two halves:
#
#   conformance/tier1    pure: manifest, protocol facts against the pinned
#                        build, classifier, ledger, registry, paths, env,
#                        authorities, roles, ids, JSON;
#   conformance/runtime  real: each file starts its own isolated Prime
#                        supervisor (own socket, HOME, agent dir, mock model)
#                        and drives the pinned daemon protocol -- D1, D2, D8,
#                        the environment boundary, the S1 regressions.
#
# The runner is Node's own `node --test` with native type stripping. There is
# no test-framework dependency and no build step.
#
# The run ends with a process sweep: any process still referencing a fixture
# root is a failure, not a warning.
#
# Usage: scripts/conformance.sh [extra node --test arguments]
set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH='' cd -- "$script_dir/.." && pwd)

fail() {
	printf 'conformance: %s\n' "$1" >&2
	exit 1
}

command -v node >/dev/null 2>&1 || fail 'node is not on PATH'

install_root=$(node -e '
	const fs = require("node:fs");
	const doc = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
	process.stdout.write(String(doc.substrate.installRoot));
' "$repo_root/pins/pins.json") || fail 'cannot read substrate.installRoot from pins/pins.json'

[ -x "$repo_root/$install_root/node_modules/.bin/prime-agent" ] ||
	fail "the pinned prime-agent is not installed at $install_root. Run scripts/bootstrap.sh first."

cd "$repo_root"

[ -x "$repo_root/node_modules/.bin/tsc" ] ||
	fail 'typescript is not installed. Run scripts/bootstrap.sh first.'

printf 'conformance: typecheck (tsc --noEmit)\n'
./node_modules/.bin/tsc --noEmit || fail 'typecheck failed'
printf 'conformance: typecheck clean\n\n'

printf 'conformance: tier 1 / pure\n'
node --test "$@" 'conformance/tier1/**/*.test.ts' </dev/null

printf '\nconformance: tier 1 / runtime (isolated Prime supervisors)\n'
# Sequential on purpose: several files kill supervisors and workers, and the
# process sweep at the end must see a quiet table.
node --test --test-concurrency=1 "$@" 'conformance/runtime/**/*.test.ts' </dev/null

printf '\nconformance: process sweep\n'
survivors=$(ps -axo pid=,command= | awk '/\/tmp\/cg-[A-Za-z0-9]+\// && !/awk/ { print }')
if [ -n "$survivors" ]; then
	printf '%s\n' "$survivors" >&2
	fail 'processes referencing a conformance fixture survived the run'
fi
printf 'conformance: no fixture processes survived\n'
