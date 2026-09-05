/**
 * Pin and authority policies as falsifiable functions.
 *
 * The active architecture is composition-first (ADRs 0008-0010). An authority
 * record therefore answers not only "who owns this concern?" but also "why is
 * that owner allowed to exist here?" Assigned concerns are classified as
 * USE EXISTING, PLUGIN, or TEMP WORKAROUND; temporary workarounds must name the
 * condition that removes them.
 *
 * Both records live in `pins/pins.json` — `packages[]` and `concerns[]` — so
 * that "who owns this?" and "at what exact version?" cannot drift apart. These
 * functions never read a file: they are handed records, including deliberately
 * invalid ones, which is what lets the tests prove the checks can fail.
 */

export interface PinnedPackageRecord {
	readonly source?: unknown;
	readonly exactVersion?: unknown;
	readonly resolvedSha?: unknown;
	readonly integrity?: unknown;
	readonly license?: unknown;
	readonly reviewedAt?: unknown;
	readonly authority?: unknown;
}

export interface ConcernRecord {
	readonly concern?: unknown;
	readonly status?: unknown;
	readonly owner?: unknown;
	readonly plannedOwner?: unknown;
	readonly phase?: unknown;
	readonly note?: unknown;
	readonly disposition?: unknown;
	readonly removalCondition?: unknown;
}

export interface AuthoritiesRecord {
	readonly concerns?: unknown;
}

function isNonEmptyString(value: unknown): value is string {
	return typeof value === "string" && value.length > 0;
}

export function checkPackagePin(pkg: PinnedPackageRecord): string[] {
	const errors: string[] = [];
	const label = isNonEmptyString(pkg.source) ? pkg.source : "<no source>";

	if (!isNonEmptyString(pkg.source)) errors.push("package entry has no source");

	const hasExactVersion =
		typeof pkg.exactVersion === "string" && /^\d+\.\d+\.\d+/.test(pkg.exactVersion);
	const hasResolvedSha =
		typeof pkg.resolvedSha === "string" && /^[0-9a-f]{40}$/.test(pkg.resolvedSha);

	if (!hasExactVersion && !hasResolvedSha) {
		errors.push(
			`${label}: needs an exact npm version or a 40-character commit sha; a bare name, branch or tag is not a pin`,
		);
	}
	// A version resolves to whatever the registry serves today. The integrity
	// hash is what makes it the same bytes tomorrow, so it is not optional: npm
	// enforces it on install, and without it a re-published version installs
	// silently.
	if (typeof pkg.integrity !== "string" || !/^sha512-[A-Za-z0-9+/]+=*$/.test(pkg.integrity)) {
		errors.push(`${label}: needs an npm integrity hash in sha512- form; a version alone does not pin the bytes`);
	}
	if (!isNonEmptyString(pkg.authority)) {
		errors.push(`${label}: must name the authority it owns, so a second owner is visible`);
	}
	if (!isNonEmptyString(pkg.license)) errors.push(`${label}: must record the license it was reviewed under`);
	if (!isNonEmptyString(pkg.reviewedAt)) errors.push(`${label}: must record when it was reviewed, at that revision`);
	return errors;
}

export function checkPackageSet(
	packages: readonly PinnedPackageRecord[],
	knownConcerns: ReadonlySet<string>,
): string[] {
	const errors: string[] = [];
	const owners = new Map<string, string[]>();

	for (const pkg of packages) {
		errors.push(...checkPackagePin(pkg));
		if (isNonEmptyString(pkg.authority)) {
			if (!knownConcerns.has(pkg.authority)) {
				errors.push(
					`${String(pkg.source)}: claims authority '${pkg.authority}', which is not a concern in pins/pins.json`,
				);
			}
			const list = owners.get(pkg.authority) ?? [];
			list.push(String(pkg.source));
			owners.set(pkg.authority, list);
		}
	}

	for (const [concern, sources] of owners) {
		if (sources.length > 1) {
			errors.push(`${concern} is claimed by ${sources.join(" and ")}; Prime/Pi would resolve that authority collision outside our control`);
		}
	}
	return errors;
}

/**
 * `ownerExists` decides what a real owner is, and is supplied by the caller
 * rather than assumed here: an owner may be the pinned substrate, a package
 * pinned in `packages[]`, or a path in this repository, and only the caller
 * holds the manifest that settles the first two.
 */
export function checkAuthorities(
	doc: AuthoritiesRecord,
	ownerExists: (path: string) => boolean,
): string[] {
	const errors: string[] = [];

	if (!Array.isArray(doc.concerns)) {
		return ["concerns must be an array, so that two owners for one concern is representable"];
	}
	if (doc.concerns.length === 0) return ["concerns is empty"];

	const seen = new Map<string, number>();
	for (const entry of doc.concerns as ConcernRecord[]) {
		if (!isNonEmptyString(entry.concern)) {
			errors.push("a concern entry has no name");
			continue;
		}
		seen.set(entry.concern, (seen.get(entry.concern) ?? 0) + 1);

		if (entry.status !== "assigned" && entry.status !== "unassigned") {
			errors.push(`${entry.concern}: status must be assigned or unassigned`);
		}

		if (entry.status === "assigned") {
			if (!isNonEmptyString(entry.owner)) {
				errors.push(`${entry.concern}: assigned with no owner`);
			} else if (!ownerExists(entry.owner)) {
				errors.push(
					`${entry.concern}: owner ${entry.owner} does not exist — it is neither the pinned substrate, nor a package pinned in packages[], nor a path in this repository`,
				);
			}

			if (
				entry.disposition !== "USE EXISTING" &&
				entry.disposition !== "PLUGIN" &&
				entry.disposition !== "TEMP WORKAROUND"
			) {
				errors.push(`${entry.concern}: assigned concern needs disposition USE EXISTING, PLUGIN, or TEMP WORKAROUND`);
			}
			if (entry.disposition === "TEMP WORKAROUND" && !isNonEmptyString(entry.removalCondition)) {
				errors.push(`${entry.concern}: TEMP WORKAROUND needs an explicit removalCondition`);
			}
		} else if (entry.status === "unassigned") {
			if (!isNonEmptyString(entry.plannedOwner) || !isNonEmptyString(entry.phase)) {
				errors.push(`${entry.concern}: an unassigned concern must name a planned owner and a phase, so it cannot be adopted by accident`);
			}
		}

		if (!isNonEmptyString(entry.note)) errors.push(`${entry.concern}: needs a note`);
	}

	for (const [concern, count] of seen) {
		if (count > 1) errors.push(`${concern}: has ${count} owners; exactly one is allowed`);
	}
	return errors;
}
