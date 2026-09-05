#!/bin/sh
# Run the Command Governor conformance suite.
#
# The suite is small, product-oriented and entirely black-box against the
# pinned Prime Agent substrate. There is no Command Governor runtime left to
# test: ADR 0010's composition-first review, and the stock-client proof it
# required, deleted it. What remains is the only thing that will tell a future
# session when the substrate's behaviour changes underneath the distribution.
#
#   conformance/tier1    credential-free record checks: the component manifest
#                        against pins/SHA256SUMS, the install-root lockfile and
#                        the installed binary; one authority per concern; the
#                        package pin policy; every JSON document parses.
#   conformance/runtime  isolated real Prime supervisors driven ONLY through
#                        stock `prime-agent` clients: the D1/D2/D8 product
#                        invariants, that the live daemon speaks the protocol
#                        pins.json records, that every pinned package actually
#                        registers on it, and that Prime's autonomous gate is
#                        owned by the host rather than by the model.
#
# The runtime tier needs three things beyond node: python3 (the interactive TUI
# is the only stock client that creates a resident session and it refuses to
# run without a tty), uv on PATH (Prime bootstraps its Python kernel with it;
# the fixtures set PRIME_AGENT_INSTALL_UV=0 so no test curl-pipes an
# installer), and npm plus network access (conformance/runtime/package-load
# installs every pinned package project-scoped into a disposable fixture
# project). Every fixture shares one version-keyed kernel venv, so that
# bootstrap is paid once per machine rather than once per root.
#
# Runtime tests are sequential because they kill supervisors and workers. The
# run ends with a sweep of exactly the fixture roots this run created;
# survivors are a failure, not a warning. See conformance/sweep.ts for why a
# `ps` sweep alone cannot see a leaked Prime process.
#
# Usage: scripts/conformance.sh [extra node --test arguments]
#   CG_KEEP_ROOTS=1  keep fixture roots for inspection (the sweep then only
#                    checks for live daemons and referencing processes)
#   CG_VERBOSE=1     echo each fixture's own notes to stdout
#   CG_KERNEL_VENV   override the shared Prime kernel venv location
set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH='' cd -- "$script_dir/.." && pwd)

fail() {
	printf 'conformance: %s\n' "$1" >&2
	exit 1
}

command -v node >/dev/null 2>&1 || fail 'node is not on PATH'
command -v python3 >/dev/null 2>&1 ||
	fail 'python3 is not on PATH; the runtime tier drives the stock interactive client on a pty'
command -v uv >/dev/null 2>&1 ||
	fail 'uv is not on PATH; Prime needs it to bootstrap the Python kernel the D1/D2 tool path exercises'
command -v npm >/dev/null 2>&1 ||
	fail 'npm is not on PATH; the package-load tier installs every pinned package project-scoped'

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

# Each runtime fixture appends the root it created here, so the sweep acts on
# exactly this run's roots and leaves any other agent's /tmp/cg-* alone.
run_dir=$(mktemp -d "${TMPDIR:-/tmp}/cg-conformance-run.XXXXXX") || fail 'cannot create a run directory'
CG_RUN_MANIFEST="$run_dir/roots"
export CG_RUN_MANIFEST
: >"$CG_RUN_MANIFEST"
cleanup() { rm -rf "$run_dir"; }
trap cleanup EXIT INT TERM

printf 'conformance: typecheck (tsc --noEmit)\n'
./node_modules/.bin/tsc --noEmit || fail 'typecheck failed'
printf 'conformance: typecheck clean\n\n'

printf 'conformance: tier 1 / records, pins and policy\n'
node --test "$@" 'conformance/tier1/**/*.test.ts' </dev/null

printf '\nconformance: runtime / isolated pinned Prime, stock clients only\n'
node --test --test-concurrency=1 "$@" 'conformance/runtime/**/*.test.ts' </dev/null

printf '\nconformance: process and fixture sweep\n'
node conformance/sweep.ts "$CG_RUN_MANIFEST" || fail 'the conformance run left something behind'
