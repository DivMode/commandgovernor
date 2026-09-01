#!/bin/sh
# Run the Command Governor conformance suite.
#
# Tier 1 is credential-free and must pass on every change; it is the gate on any
# re-pin of the Pi runtime. Tier 2 needs a real provider credential and is
# skipped unless CG_CONFORMANCE_LIVE=1 -- its tests report as skipped with a
# stated reason and are never reported as passing.
#
# The runner is Node's own `node --test` with native type stripping. There is no
# test-framework dependency, and there is no build step: the same .ts files the
# suite imports are the ones Pi loads through jiti.
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

# The suite drives the real pinned runtime and imports the pinned packages, so
# refuse to start rather than reporting a wall of skips that looks like success.
install_root=$(node -e '
	const fs = require("node:fs");
	const doc = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
	process.stdout.write(String(doc.pi.installRoot));
' "$repo_root/pins/pins.json") || fail 'cannot read pi.installRoot from pins/pins.json'

[ -x "$repo_root/$install_root/node_modules/.bin/pi" ] ||
	fail "the pinned pi is not installed at $install_root. Run scripts/bootstrap.sh first."

[ -e "$repo_root/node_modules/@earendil-works" ] ||
	fail 'node_modules/@earendil-works is missing. Run scripts/bootstrap.sh first.'

cd "$repo_root"

printf 'conformance: tier 1 (credential-free)\n'
# A glob, not a directory: `node --test <dir>` tries to load the directory
# itself as a module on Node 24.
node --test "$@" 'conformance/tier1/**/*.test.ts'

printf '\nconformance: tier 2 (credentialed; CG_CONFORMANCE_LIVE=%s)\n' \
	"${CG_CONFORMANCE_LIVE:-unset}"
node --test "$@" 'conformance/tier2/**/*.test.ts'
