#!/bin/sh
# Run the active Command Governor conformance suite.
#
# The suite is intentionally small and product-oriented:
#
#   conformance/tier1    credential-free pin/policy checks plus the minimum
#                        unit coverage for temporary D1/D2 compatibility shims;
#   conformance/runtime  isolated real Prime supervisors exercising the public
#                        pinned-runtime behavior: D1, D2, D8, environment
#                        boundary, and the surviving S1 regressions.
#
# Historical standalone-Rust tests are not an active second oracle. See
# docs/testing.md and docs/research/2026-09-01-rust-invariant-catalog.md.
#
# Runtime tests are sequential because they kill supervisors/workers. The run
# ends with a process sweep; survivors are a failure, not a warning.
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

printf 'conformance: tier 1 / policy + temporary-workaround units\n'
node --test "$@" 'conformance/tier1/**/*.test.ts' </dev/null

printf '\nconformance: runtime / isolated pinned Prime\n'
node --test --test-concurrency=1 "$@" 'conformance/runtime/**/*.test.ts' </dev/null

printf '\nconformance: process sweep\n'
survivors=$(ps -axo pid=,command= | awk '/\/tmp\/cg-[A-Za-z0-9]+\// && !/awk/ { print }')
if [ -n "$survivors" ]; then
	printf '%s\n' "$survivors" >&2
	fail 'processes referencing a conformance fixture survived the run'
fi
printf 'conformance: no fixture processes survived\n'
