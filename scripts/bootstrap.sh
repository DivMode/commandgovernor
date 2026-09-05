#!/bin/sh
# Reproducible bootstrap of the pinned Prime Agent substrate for Command Governor.
#
# Order matters. Every release asset is downloaded from the immutable GitHub
# release and verified against TWO authorities -- pins/pins.json and the
# release's own SHA256SUMS as committed in pins/SHA256SUMS -- before npm is
# allowed to run. The upstream SHA256SUMS is fetched again and diffed against
# the committed copy, so a re-published release cannot pass silently. The
# three sibling packages are installed by npm from the lockfile, whose sha512
# integrity values the conformance suite proves equal to the manifest's.
# The version assertion happens after install, against the binary produced.
#
# Nothing here depends on a `prime-agent` on PATH, and nothing is installed
# globally (a guard on the supported Mac forbids `npm install -g` anyway).
#
# Usage: scripts/bootstrap.sh
set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH='' cd -- "$script_dir/.." && pwd)
pins_json="$repo_root/pins/pins.json"
sha_sums="$repo_root/pins/SHA256SUMS"

fail() {
	printf 'bootstrap: %s\n' "$1" >&2
	exit 1
}

[ -f "$pins_json" ] || fail "missing pin record: $pins_json"
[ -f "$sha_sums" ] || fail "missing release checksums: $sha_sums"

# --- step 1: node is present and new enough --------------------------------

command -v node >/dev/null 2>&1 || fail 'node is not on PATH'
command -v npm >/dev/null 2>&1 || fail 'npm is not on PATH'
command -v curl >/dev/null 2>&1 || fail 'curl is not on PATH'

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

node_required=$(pins_field substrate.engines.node) || fail 'cannot read substrate.engines.node'
version=$(pins_field substrate.version) || fail 'cannot read substrate.version'
install_root_rel=$(pins_field substrate.installRoot) || fail 'cannot read substrate.installRoot'
vendor_rel=$(pins_field substrate.vendorDir) || fail 'cannot read substrate.vendorDir'
release_base=$(pins_field substrate.releaseBaseUrl) || fail 'cannot read substrate.releaseBaseUrl'
checksum_asset=$(pins_field substrate.checksumAsset) || fail 'cannot read substrate.checksumAsset'
install_root="$repo_root/$install_root_rel"
vendor="$repo_root/$vendor_rel"

node -e '
	const required = process.argv[1].replace(/^>=/, "");
	const parse = (v) => v.split(".").map((n) => Number.parseInt(n, 10) || 0);
	const [ra, rb, rc] = parse(required);
	const [ca, cb, cc] = parse(process.versions.node);
	const ok = ca > ra || (ca === ra && (cb > rb || (cb === rb && cc >= rc)));
	if (!ok) {
		console.error("node " + process.versions.node + " is older than the pinned floor " + required);
		process.exit(1);
	}
' "$node_required" || fail "node version floor not met (need $node_required)"

printf 'bootstrap: node %s satisfies %s\n' "$(node --version)" "$node_required"

# --- step 2: fetch the release assets and verify them twice ----------------

if command -v shasum >/dev/null 2>&1; then
	sha256_of() { shasum -a 256 "$1" | cut -d' ' -f1; }
elif command -v sha256sum >/dev/null 2>&1; then
	sha256_of() { sha256sum "$1" | cut -d' ' -f1; }
else
	fail 'no sha256 tool found (need shasum or sha256sum)'
fi

mkdir -p "$vendor"

# The release's checksum file, fetched fresh, must be byte-identical to the
# committed copy. A release whose assets were re-uploaded fails here.
curl -fsSL -o "$vendor/$checksum_asset.upstream" "$release_base$checksum_asset" ||
	fail "cannot download $checksum_asset from $release_base"
cmp -s "$vendor/$checksum_asset.upstream" "$sha_sums" ||
	fail "upstream $checksum_asset differs from the committed pins/SHA256SUMS; the release changed under the pin"
printf 'bootstrap: upstream %s matches the committed copy\n' "$checksum_asset"

asset_rows=$(node -e '
	const fs = require("node:fs");
	const doc = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
	const assets = doc.substrate && doc.substrate.assets;
	if (!Array.isArray(assets) || assets.length === 0) {
		console.error("pins.json: substrate.assets must be a non-empty array");
		process.exit(3);
	}
	for (const a of assets) {
		if (!a.name || !a.sha256 || !a.sha512) {
			console.error("pins.json: asset entry missing name/sha256/sha512");
			process.exit(3);
		}
		process.stdout.write(a.name + " " + a.sha256 + "\n");
	}
' "$pins_json") || fail 'cannot read substrate.assets'

printf '%s\n' "$asset_rows" | while IFS=' ' read -r name expected_sha; do
	[ -n "$name" ] || continue
	local_file="$vendor/$name"

	# Authority 1: the release's own checksum line.
	upstream_sha=$(awk -v n="$name" '$2 == n { print $1 }' "$sha_sums")
	[ -n "$upstream_sha" ] || fail "pins/SHA256SUMS has no line for $name"

	# Authority 2: our manifest. If they disagree, the manifest was edited away from the release.
	[ "$upstream_sha" = "$expected_sha" ] ||
		fail "pins.json sha256 for $name disagrees with pins/SHA256SUMS"

	if [ -f "$local_file" ] && [ "$(sha256_of "$local_file")" = "$expected_sha" ]; then
		printf 'bootstrap: %s already present and verified\n' "$name"
		continue
	fi
	rm -f "$local_file"
	curl -fsSL -o "$local_file" "$release_base$name" || fail "cannot download $name from $release_base"
	actual_sha=$(sha256_of "$local_file")
	[ "$actual_sha" = "$expected_sha" ] ||
		fail "checksum mismatch for $name
  expected $expected_sha
  actual   $actual_sha"
	printf 'bootstrap: verified %s\n' "$name"
done || exit 1

# --- step 3: install from the pinned lockfile ------------------------------

[ -f "$install_root/package.json" ] || fail "no package.json under $install_root_rel"
[ -f "$install_root/package-lock.json" ] || fail "no package-lock.json under $install_root_rel"

printf 'bootstrap: npm ci --ignore-scripts in %s\n' "$install_root_rel"
( cd "$install_root" && npm ci --ignore-scripts ) || fail 'npm ci failed'

# --- step 3a: vendored third-party packages --------------------------------
# A package whose only source is an npm tarball from an author with no public
# repository is vendored: the tarball is committed under pins/packages/, its
# sha512 must equal the manifest's integrity, and the committed patch under
# pins/patches/ is applied on top. Prime installs the result by path. Nothing
# here waits on a registry or an upstream.

vendored_count=$(node -p 'JSON.stringify((require(process.argv[1]).packages||[]).filter(p=>p.tarball))' "$pins_json" | node -p 'JSON.parse(require("fs").readFileSync(0,"utf8")).length') ||
	fail 'cannot read vendored packages from pins.json'
i=0
while [ "$i" -lt "$vendored_count" ]; do
	entry=$(node -p 'JSON.stringify((require(process.argv[1]).packages||[]).filter(p=>p.tarball)[+process.argv[2]])' "$pins_json" "$i") || fail 'cannot read vendored entry'
	tarball_rel=$(printf '%s' "$entry" | node -p 'JSON.parse(require("fs").readFileSync(0,"utf8")).tarball')
	dir_rel=$(printf '%s' "$entry" | node -p 'JSON.parse(require("fs").readFileSync(0,"utf8")).source.replace(/^\.\//,"")')
	expected_integrity=$(printf '%s' "$entry" | node -p 'JSON.parse(require("fs").readFileSync(0,"utf8")).integrity')
	patches=$(printf '%s' "$entry" | node -p '(JSON.parse(require("fs").readFileSync(0,"utf8")).patches||[]).join(" ")')
	tarball="$repo_root/$tarball_rel"
	[ -f "$tarball" ] || fail "vendored tarball $tarball_rel is missing"
	actual_integrity="sha512-$(openssl dgst -sha512 -binary "$tarball" | base64 | tr -d '\n')"
	[ "$actual_integrity" = "$expected_integrity" ] ||
		fail "vendored tarball $tarball_rel does not hash to pins.json integrity
  expected $expected_integrity
  actual   $actual_integrity"
	dir="$repo_root/$dir_rel"
	rm -rf "$dir"
	mkdir -p "$dir"
	tar -xzf "$tarball" -C "$dir" --strip-components=1 || fail "cannot extract $tarball_rel"
	for patch_rel in $patches; do
		( cd "$dir" && patch -p1 --silent < "$repo_root/$patch_rel" ) || fail "patch $patch_rel does not apply to $tarball_rel"
	done
	printf 'bootstrap: vendored %s -> %s (%s patch(es))\n' "$tarball_rel" "$dir_rel" "$(printf '%s' "$patches" | wc -w | tr -d ' ')"
	i=$((i + 1))
done

# --- step 3b: the repository's own tooling ---------------------------------

printf 'bootstrap: npm ci --ignore-scripts at the repository root\n'
( cd "$repo_root" && npm ci --ignore-scripts ) || fail 'root npm ci failed'

# --- step 4: what came out is what was pinned, and nothing else ------------

[ ! -e "$install_root/node_modules/@earendil-works/pi-coding-agent" ] ||
	fail 'upstream Pi (@earendil-works/pi-coding-agent) is present in the Prime install root; the two must never be co-installed'
[ ! -e "$repo_root/node_modules/@earendil-works" ] ||
	fail 'the repository root node_modules carries an @earendil-works tree; remove it (a leftover from the upstream-Pi donor branch)'

for sibling in pi-agent-core pi-ai pi-tui; do
	sibling_version=$(node -p 'require(process.argv[1]).version' "$install_root/node_modules/@earendil-works/$sibling/package.json") ||
		fail "pinned sibling @earendil-works/$sibling is missing"
	[ "$sibling_version" = "$version" ] ||
		fail "@earendil-works/$sibling is $sibling_version, pins.json requires $version"
done

prime_bin="$install_root/node_modules/.bin/prime-agent"
[ -x "$prime_bin" ] || fail "pinned prime-agent binary not found at $install_root_rel/node_modules/.bin/prime-agent"
# prime-agent prints its version on stderr, so capture both streams.
installed_version=$("$prime_bin" --version </dev/null 2>&1 | tr -d '\r' | tail -n 1) || fail 'pinned prime-agent --version failed'
[ "$installed_version" = "$version" ] ||
	fail "pinned prime-agent reports $installed_version, pins.json requires $version"

cat <<EOT

bootstrap: ok
  pinned prime-agent  $version
  binary              $install_root_rel/node_modules/.bin/prime-agent
  conformance         scripts/conformance.sh
EOT
