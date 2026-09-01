#!/bin/sh
# Reproducible bootstrap of the pinned Pi runtime for Command Governor.
#
# Installs the exact Pi release recorded in pins/pins.json into a repo-local
# node_modules tree, using upstream's own installer lockfile. Nothing here
# depends on a `pi` that happens to be on PATH; the point of the exercise is a
# copy of Pi whose provenance we can name.
#
# Order matters. The sha256 verification happens before npm is allowed to read
# the lockfile, so a tampered pin cannot cause a download. The version assertion
# happens after install, against the binary that was actually produced.
#
# Usage: scripts/bootstrap.sh
set -eu

# --- locate the repository -------------------------------------------------

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH='' cd -- "$script_dir/.." && pwd)
pins_json="$repo_root/pins/pins.json"
sha_sums="$repo_root/pins/SHA256SUMS"

fail() {
	printf 'bootstrap: %s\n' "$1" >&2
	exit 1
}

[ -f "$pins_json" ] || fail "missing pin record: $pins_json"
[ -f "$sha_sums" ] || fail "missing upstream checksums: $sha_sums"

# --- step 1: node is present and new enough --------------------------------

command -v node >/dev/null 2>&1 || fail 'node is not on PATH'

# Read one dotted field out of pins.json. node is the only interpreter we are
# entitled to assume, having just required it, so it reads our JSON too -- no
# jq dependency, and no second copy of the pin in a shell variable.
pins_field() {
	node -e '
		const fs = require("node:fs");
		const doc = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
		let value = doc;
		for (const key of process.argv[2].split(".")) {
			if (value === null || typeof value !== "object" || !(key in value)) {
				console.error("pins.json: missing field " + process.argv[2]);
				process.exit(3);
			}
			value = value[key];
		}
		if (typeof value !== "string") {
			console.error("pins.json: field " + process.argv[2] + " is not a string");
			process.exit(3);
		}
		process.stdout.write(value);
	' "$pins_json" "$1"
}

node_required=$(pins_field pi.engines.node) || fail 'cannot read pi.engines.node'
pi_version=$(pins_field pi.version) || fail 'cannot read pi.version'
install_root_rel=$(pins_field pi.installRoot) || fail 'cannot read pi.installRoot'
install_root="$repo_root/$install_root_rel"

node -e '
	const required = process.argv[1].replace(/^>=/, "");
	const parse = (v) => v.split(".").map((n) => Number.parseInt(n, 10) || 0);
	const [ra, rb, rc] = parse(required);
	const [ca, cb, cc] = parse(process.versions.node);
	const ok = ca > ra || (ca === ra && (cb > rb || (cb === rb && cc >= rc)));
	if (!ok) {
		console.error(
			"node " + process.versions.node + " is older than the pinned floor " + required,
		);
		process.exit(1);
	}
' "$node_required" || fail "node version floor not met (need $node_required)"

printf 'bootstrap: node %s satisfies %s\n' "$(node --version)" "$node_required"

# --- step 2: the committed pins are the bytes upstream published -----------

# We renamed the two release assets on the way in (an npm lockfile root has to
# be called package.json), so the checksum line has to be matched by the
# original asset name and applied to the local path.
if command -v shasum >/dev/null 2>&1; then
	sha256_of() { shasum -a 256 "$1" | cut -d' ' -f1; }
elif command -v sha256sum >/dev/null 2>&1; then
	sha256_of() { sha256sum "$1" | cut -d' ' -f1; }
else
	fail 'no sha256 tool found (need shasum or sha256sum)'
fi

asset_rows=$(node -e '
	const fs = require("node:fs");
	const doc = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
	const assets = doc.pi && doc.pi.assets;
	if (!Array.isArray(assets) || assets.length === 0) {
		console.error("pins.json: pi.assets must be a non-empty array");
		process.exit(3);
	}
	for (const a of assets) {
		if (!a.releaseName || !a.path || !a.sha256) {
			console.error("pins.json: asset entry missing releaseName/path/sha256");
			process.exit(3);
		}
		process.stdout.write(a.releaseName + " " + a.path + " " + a.sha256 + "\n");
	}
' "$pins_json") || fail 'cannot read pi.assets'

printf '%s\n' "$asset_rows" | while IFS=' ' read -r release_name asset_path expected_sha; do
	[ -n "$release_name" ] || continue
	local_file="$repo_root/$asset_path"
	[ -f "$local_file" ] || fail "pinned asset missing: $asset_path"

	# Authority 1: the checksum upstream published, keyed by the original name.
	upstream_sha=$(awk -v name="$release_name" '$2 == name { print $1 }' "$sha_sums")
	[ -n "$upstream_sha" ] || fail "pins/SHA256SUMS has no line for $release_name"

	# Authority 2: our own record. If these two ever disagree, the pin record
	# has been edited away from the release it claims to describe.
	[ "$upstream_sha" = "$expected_sha" ] ||
		fail "pins.json sha256 for $release_name disagrees with pins/SHA256SUMS"

	actual_sha=$(sha256_of "$local_file")
	[ "$actual_sha" = "$upstream_sha" ] ||
		fail "checksum mismatch for $asset_path
  expected $upstream_sha
  actual   $actual_sha"

	printf 'bootstrap: verified %s (%s)\n' "$asset_path" "$release_name"
done || exit 1

# --- step 3: install from the pinned lockfile ------------------------------

[ -f "$install_root/package.json" ] || fail "no package.json under $install_root_rel"
[ -f "$install_root/package-lock.json" ] || fail "no package-lock.json under $install_root_rel"

printf 'bootstrap: npm ci --ignore-scripts in %s\n' "$install_root_rel"
( cd "$install_root" && npm ci --ignore-scripts ) || fail 'npm ci failed'

# --- step 3b: the repository's own tooling ---------------------------------

# TypeScript and the Node type definitions, for `tsc --noEmit`. They are
# devDependencies of the distribution package, so `pi install` -- which runs
# `npm install --omit=dev` -- never fetches them for a consumer.
#
# This has to run BEFORE the symlinks below: `npm ci` deletes node_modules
# wholesale before installing, so links created first would be silently
# destroyed and every bare-specifier import would break.
#
# The Pi-provided packages are declared optional in `peerDependenciesMeta`.
# Without that, npm resolves them from the registry and installs a second,
# unpinned copy of the entire Pi tree next to the pinned one -- measured, not
# assumed. Optional is also the honest declaration: the host provides them.
printf 'bootstrap: npm ci --ignore-scripts at the repository root\n'
( cd "$repo_root" && npm ci --ignore-scripts ) || fail 'root npm ci failed'

# Make the pinned Pi packages resolvable as bare specifiers from anywhere in the
# repository, so `import { VERSION } from "@earendil-works/pi-coding-agent"` in
# harness/extensions/ means the pinned copy and nothing else. Pi's own loader
# resolves those imports from its module root; this is the same tree, reached
# the same way.
#
# Exactly the five packages Pi contractually provides to an extension, linked
# one by one. Symlinking the whole @earendil-works scope would be shorter and
# wrong in both directions: it exposes pi-client, pi-protocol and pi-telemetry,
# which an extension is NOT entitled to import and which would typecheck here
# and fail inside Pi -- and it misses typebox, which is entitled but is not
# under that scope. Getting this wrong turns the repository into a more
# permissive environment than the runtime, which is the failure mode where an
# extension passes every local check and breaks on load.
#
# The links live inside a real directory so the repository's existing
# `node_modules/` ignore rule covers them.
mkdir -p "$repo_root/node_modules/@earendil-works"
for pkg in pi-coding-agent pi-agent-core pi-ai pi-tui; do
	target="$install_root/node_modules/@earendil-works/$pkg"
	[ -d "$target" ] || fail "pinned package missing: @earendil-works/$pkg"
	rm -rf "$repo_root/node_modules/@earendil-works/$pkg"
	ln -s "../../$install_root_rel/node_modules/@earendil-works/$pkg" \
		"$repo_root/node_modules/@earendil-works/$pkg" ||
		fail "cannot link @earendil-works/$pkg"
done
[ -d "$install_root/node_modules/typebox" ] || fail 'pinned package missing: typebox'
rm -rf "$repo_root/node_modules/typebox"
ln -s "../$install_root_rel/node_modules/typebox" "$repo_root/node_modules/typebox" ||
	fail 'cannot link typebox'
printf 'bootstrap: linked the 5 pi-provided packages into node_modules/\n'

# --- step 4: the binary that came out is the version we pinned -------------

pi_bin="$install_root/node_modules/.bin/pi"
[ -x "$pi_bin" ] || fail "pinned pi binary not found at $install_root_rel/node_modules/.bin/pi"

installed_version=$("$pi_bin" --version 2>/dev/null) || fail 'pinned pi --version failed'
[ "$installed_version" = "$pi_version" ] ||
	fail "pinned pi reports $installed_version, pins.json requires $pi_version"

# --- step 5: say where it is -----------------------------------------------

cat <<EOF

bootstrap: ok
  pinned pi      $pi_version
  binary         $install_root_rel/node_modules/.bin/pi
  launch with    bin/cg-pi [pi arguments...]
  conformance    scripts/conformance.sh
EOF
