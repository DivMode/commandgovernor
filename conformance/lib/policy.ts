/**
 * The pin and authority policies, as functions.
 *
 * They live here rather than inline in the assertions for one reason: both
 * policies currently run over collections that are legitimately empty --
 * `pins.json` `packages[]` has no entries yet -- and an assertion that loops
 * over an empty array passes without evaluating anything. That is a test which
 * cannot come back negative, which is not a test.
 *
 * Extracting the rules lets the suite run them against fabricated records that
 * violate each one, so the checks are shown to fail before they are trusted to
 * pass.
 */

export interface PinnedPackageRecord {
	readonly source?: unknown;
	readonly exactVersion?: unknown;
	readonly resolvedSha?: unknown;
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
}

export interface AuthoritiesRecord {
	readonly concerns?: unknown;
}

function isNonEmptyString(value: unknown): value is string {
	return typeof value === "string" && value.length > 0;
}

/**
 * Violations of the third-party pin policy for one package entry.
 *
 * Pi keeps no lockfile for the packages it installs and treats any git ref as
 * `pinned` -- a branch or a mutable tag included. Only an exact npm version or
 * a 40-character commit SHA is immutable, so only those count here.
 */
export function checkPackagePin(pkg: PinnedPackageRecord): string[] {
	const errors: string[] = [];
	const label = isNonEmptyString(pkg.source) ? pkg.source : "<no source>";

	if (!isNonEmptyString(pkg.source)) {
		errors.push("package entry has no source");
	}

	const hasExactVersion =
		typeof pkg.exactVersion === "string" && /^\d+\.\d+\.\d+/.test(pkg.exactVersion);
	const hasResolvedSha =
		typeof pkg.resolvedSha === "string" && /^[0-9a-f]{40}$/.test(pkg.resolvedSha);

	if (!hasExactVersion && !hasResolvedSha) {
		errors.push(
			`${label}: needs an exact npm version or a 40-character commit sha; a bare name, branch or tag is not a pin`,
		);
	}
	if (!isNonEmptyString(pkg.authority)) {
		errors.push(
			`${label}: must name the authority it owns, so a second owner is visible`,
		);
	}
	if (!isNonEmptyString(pkg.license)) {
		errors.push(`${label}: must record the license it was reviewed under`);
	}
	if (!isNonEmptyString(pkg.reviewedAt)) {
		errors.push(`${label}: must record when it was reviewed, at that revision`);
	}
	return errors;
}

/** Violations across the whole pinned-package set, including shared authorities. */
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
					`${String(pkg.source)}: claims authority '${pkg.authority}', which is not a concern in harness/authorities.json`,
				);
			}
			const list = owners.get(pkg.authority) ?? [];
			list.push(String(pkg.source));
			owners.set(pkg.authority, list);
		}
	}

	for (const [concern, sources] of owners) {
		if (sources.length > 1) {
			errors.push(
				`${concern} is claimed by ${sources.join(" and ")}; Pi would resolve that silently by load order`,
			);
		}
	}
	return errors;
}

/**
 * Violations of the one-authority-per-concern policy.
 *
 * Note the shape this depends on: `concerns` must be an array. An object keyed
 * by concern could not express two owners for one key, and the duplicate rule
 * below would be unfalsifiable.
 */
export function checkAuthorities(
	doc: AuthoritiesRecord,
	ownerExists: (path: string) => boolean,
): string[] {
	const errors: string[] = [];

	if (!Array.isArray(doc.concerns)) {
		return ["concerns must be an array, so that two owners for one concern is representable"];
	}
	if (doc.concerns.length === 0) {
		return ["concerns is empty"];
	}

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
					`${entry.concern}: owner ${entry.owner} does not exist in the repository`,
				);
			}
		} else if (entry.status === "unassigned") {
			if (!isNonEmptyString(entry.plannedOwner) || !isNonEmptyString(entry.phase)) {
				errors.push(
					`${entry.concern}: an unassigned concern must name a planned owner and a phase, so it cannot be adopted by accident`,
				);
			}
		}
		if (!isNonEmptyString(entry.note)) {
			errors.push(`${entry.concern}: needs a note`);
		}
	}

	for (const [concern, count] of seen) {
		if (count > 1) {
			errors.push(`${concern}: has ${count} owners; exactly one is allowed`);
		}
	}
	return errors;
}
