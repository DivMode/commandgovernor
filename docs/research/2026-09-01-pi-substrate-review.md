# Pi core research — substrate for a Command Governor Pi-native distribution

**Date of research:** 2026-09-01
**Pinned target:** Pi `v0.84.4`
**Researcher note:** Everything below is from live primary sources fetched on 2026-09-01: the GitHub REST API for `earendil-works/pi`, a local `git clone --depth 1 --branch v0.84.4`, the npm registry, and a local install of the published tarball. Anything I could not verify is marked **UNVERIFIED** explicitly. Nothing here is recalled from training data.

**Local working copies used (both in a scratchpad outside the repository; nothing in the repo was touched):**
- a `git clone --depth 1 --branch v0.84.4` of `earendil-works/pi`
- an `npm install --ignore-scripts @earendil-works/pi-coding-agent@0.84.4`

---

## 1. What Pi concretely is

### Identity and language

`earendil-works/pi` — "AI agent toolkit: unified LLM API, agent loop, TUI, coding agent CLI". Language: TypeScript. Created 2025-08-09. 100,298 stars at time of fetch. License MIT.

> Source: `GET https://api.github.com/repos/earendil-works/pi` (fetched 2026-09-01). Repo `pushed_at: 2026-09-01T10:36:00Z`.

Note on repo naming: docs inside the repo still link to `github.com/earendil-works/pi-mono` for source-file references (e.g. `packages/coding-agent/docs/session-format.md` lines 31-35). `pi-mono` appears to be the former repository name; the canonical repo today is `earendil-works/pi`. **Both spellings appear in upstream docs** — do not treat a `pi-mono` link as a different project.

### Monorepo layout (at tag v0.84.4)

Verified with `ls packages` and per-package `package.json` reads in the clone:

| Workspace path | npm name | version | bin | engines |
|---|---|---|---|---|
| `packages/coding-agent` | `@earendil-works/pi-coding-agent` | 0.84.4 | `pi` → `dist/bundle/cli.js` | node `>=22.19.0` |
| `packages/agent` | `@earendil-works/pi-agent-core` | 0.84.4 | — | node `>=22.19.0` |
| `packages/ai` | `@earendil-works/pi-ai` | 0.84.4 | `pi-ai` → `dist/cli.js` | node `>=22.19.0` |
| `packages/tui` | `@earendil-works/pi-tui` | 0.84.4 | — | node `>=22.19.0` |
| `packages/client` | `@earendil-works/pi-client` | 0.84.4 | — | node `>=22.19.0` |
| `packages/protocol` | `@earendil-works/pi-protocol` | 0.84.4 | — | node `>=22.19.0` |
| `packages/server` | `@earendil-works/pi-server` | 0.84.4 | — | node `>=22.19.0` |
| `packages/telemetry` | `@earendil-works/pi-telemetry` | 0.84.4 | — | node `>=22.19.0` |
| `packages/evals` | `@earendil-works/pi-evals` | 0.84.4 | — | private, not published |

Root `package.json` is `pi-monorepo` (private), `"engines": {"node": ">=22.19.0"}`, npm workspaces.

**Runtime:** Node.js ≥ 22.19.0 is the declared engine for every published package. Bun is supported as a *build* target — the official standalone binaries are Bun-compiled executables (`scripts/build-binaries.sh`, referenced in `README.md`), and `packages/coding-agent/src/config.ts` has explicit `isBunBinary` / `isBunRuntime` detection and a `"bun-binary"` install method. So: **Node is the supported runtime for npm installs; Bun appears only as the compiled-binary flavour and as one of several supported global package managers.**

### Install methods (three, all upstream-documented)

1. **npm global (documented default):**
   ```bash
   npm install -g --ignore-scripts @earendil-works/pi-coding-agent
   ```
   > `packages/coding-agent/docs/quickstart.md` lines 9-13; `docs/index.md` lines 7-13.

2. **curl installer:** `curl -fsSL https://pi.dev/install.sh | sh` — documented as installing via npm globally, so it is removed with `npm uninstall -g`.
   > `docs/index.md` lines 15-21; `docs/quickstart.md` lines 17-21.

3. **Standalone Bun binaries / source archive from GitHub releases.** The v0.84.4 release carries 10 assets, all covered by a `SHA256SUMS` file (see §2).

pnpm, Yarn and Bun global installs are also documented as supported uninstall/update paths (`docs/quickstart.md` lines 23-31; `src/config.ts` `detectInstallMethod()` returns `"npm" | "pnpm" | "yarn" | "bun" | "bun-binary" | "unknown"`).

### How a user runs it

```bash
cd /path/to/project
pi                          # interactive TUI
pi -p "prompt"              # print / one-shot
pi --mode json "prompt"     # JSONL event stream
pi --mode rpc               # stdin/stdout JSONL RPC
```
> `docs/quickstart.md` lines 35-40, 147-157; `docs/usage.md` "Modes" table.

Auth is `/login` (subscription OAuth: Claude Pro/Max, ChatGPT Plus/Pro Codex, GitHub Copilot) or API-key env vars / `~/.pi/agent/auth.json`.
> `docs/quickstart.md` lines 42-67.

**Verified locally:**
```
$ npm install --ignore-scripts @earendil-works/pi-coding-agent@0.84.4   # 136 packages, 2s
$ node -e "…require('.../package.json')"
@earendil-works/pi-coding-agent 0.84.4 {"pi":"dist/bundle/cli.js"} {"node":">=22.19.0"} piConfig= {"configDir":".pi"}
$ ./node_modules/.bin/pi --version
0.84.4
$ ./node_modules/.bin/pi --version >/dev/null 2>&1; echo $?      →  0
$ PI_OFFLINE=1 pi -p --no-session "hello" >/dev/null 2>&1; echo $?  →  1   (no credentials configured)
```
Local Node was v24.19.0, npm 11.17.0 — above the declared minimum, so the `>=22.19.0` floor itself was **not** exercised.

### Design principles — what Pi deliberately does NOT have

> "It intentionally does not include built-in MCP, sub-agents, permission popups, plan mode, to-dos, or background bash. You can build or install those workflows as extensions or packages, or use external tools such as containers and tmux."
> — `packages/coding-agent/docs/usage.md`, "Design Principles"

> "Pi does not include a built-in permission system for restricting filesystem, process, network, or credential access. By default, it runs with the permissions of the user and process that launched it."
> — `README.md`, "Permissions & Containerization"; expanded in `docs/security.md` ("No Built-in Sandbox").

This is load-bearing for Command Governor: **subagents, MCP, permission gating, and plan mode are all things CG must compose or build**, not adopt from core.

---

## 2. Release v0.84.4 — confirmed

### Git

```
GET https://api.github.com/repos/earendil-works/pi/git/ref/tags/v0.84.4
→ {"ref":"refs/tags/v0.84.4", "object":{"sha":"b79e4cc834970cca69daebffab7df1da7d1e52c4","type":"commit"}}
```
Local clone `git log -1`:
```
b79e4cc834970cca69daebffab7df1da7d1e52c4 2026-08-28 23:56:03 +0200 Release v0.84.4
```

The tag is a **lightweight tag pointing directly at the commit** (`object.type == "commit"`, not `"tag"`), so `v0.84.4 == b79e4cc834970cca69daebffab7df1da7d1e52c4` exactly.

### GitHub release

- `tag_name: v0.84.4`, `draft: false`, `prerelease: false`, `target_commitish: main`
- `created_at: 2026-08-28T21:56:03Z`, `published_at: 2026-08-28T22:08:23Z`
- URL: https://github.com/earendil-works/pi/releases/tag/v0.84.4
- `GET /releases/latest` returns **v0.84.4** — it is the current latest.

Release-note highlights relevant to CG:
- **`ui_prompt_start` / `ui_prompt_end` extension events** — "so host integrations can distinguish active agent work from waiting on user-facing `ctx.ui` prompts" (PR #8355).
- **RPC `clear_queue`** — "retrieve and remove queued steering and follow-up messages" (issue #8432).
- Fixed: "resumed sessions corrupting the next appended entry when their JSONL file lacks a trailing newline" (#8…, truncated in my fetch) — directly relevant to durable-session handling.

### Release assets (all 10, with SHA256SUMS)

```
pi-0.84.4-source.tar.gz                        ca3958559b60f87ee44c84d94df8c3ee0b7eda575370402abb2d0ad9155cde4a
pi-darwin-arm64.tar.gz                         c68e3ac4d05b4e282aaab2e6c76f161d3e9e68f19a22e38913cbfaadb6c800f0
pi-darwin-x64.tar.gz                           7a042d6413065421387001a4986190a1a03186c95a695f4dee0bdc76e60de8f7
pi-linux-x64.tar.gz                            c2f3c3e6a1850bd87654cc3ca8811013272397c3d042a4e2a64c43ee1b423972
pi-linux-arm64.tar.gz                          135580f6b942151646e67b8b866d987d28ce3cff5a497030775ddd29659f943d
pi-windows-x64.zip                             03b2318774f18721e959d9f8f3340a9f942e7aa516fb7030d3007a12a40a4a97
pi-windows-arm64.zip                           6b2726efc34a9158ab06bf7b981f7bcccf15de9ea236a3f4ef7a894a78aa386e
pi-coding-agent-install-package.json           053e1c9f456f4863098baa02379c83fa92ddd70c1c61616df48548924d0caf19
pi-coding-agent-install-package-lock.json      a9f805a677f0860328059390b0f62adcc655299952f293127a8db8939818dff4
SHA256SUMS                                     (the file itself, 823 bytes)
```
> `GET https://api.github.com/repos/earendil-works/pi/releases/tags/v0.84.4` + fetch of the `SHA256SUMS` asset.

**The two `pi-coding-agent-install-*` assets are the reproducible-install mechanism** and matter enormously for a distribution. Contents of `pi-coding-agent-install-package.json`:

```json
{
  "name": "@earendil-works/pi-coding-agent-install",
  "version": "0.84.4",
  "private": true,
  "description": "Lockfile root used by the Pi installer and updater.",
  "dependencies": { "@earendil-works/pi-coding-agent": "0.84.4" },
  "overrides": { "protobufjs": "7.6.5", "rimraf": "6.1.2", "gaxios": { "rimraf": "6.1.2" } },
  "engines": { "node": ">=22.19.0" }
}
```
The companion `pi-coding-agent-install-package-lock.json` is `lockfileVersion: 3` with **137 package entries**, each with `resolved` + `integrity`. Generated by `scripts/generate-coding-agent-install-lock.mjs` in the repo (verified by reading the script header; it also carries an explicit allowlist of packages permitted to run lifecycle scripts: `@google/genai@1.52.0`, `protobufjs@7.6.5`).

### npm

`GET https://registry.npmjs.org/@earendil-works/pi-coding-agent`:
- `dist-tags`: `{"latest": "0.84.4", "legacy-node20": "0.74.2"}`
- 43 published versions total.
- **0.84.4 published 2026-08-28T22:07:57.753Z**
- `dist.shasum: 3a2f04bfc5e463b4cfa36b174a586d11a0bdf9ad`
- `dist.integrity: sha512-jmOlrqUmvhh/siNWFRXjYLJzhKFIHNsAQaysRwzQPQFnPAaV/vhqHsLH/MBsIISA1Rjj7WTUFR3nJrpXoLx39w==`
- `fileCount: 1044`, `unpackedSize: 21,497,182`
- npm **provenance attestation present** (`predicateType: https://slsa.dev/provenance/v1`) plus two registry signatures.
- Runtime deps are all exact-pinned except the internal `@earendil-works/*` ones, which are `^0.84.4`.
- The published tarball **ships `npm-shrinkwrap.json`** (`lockfileVersion 3`, 136 entries) — verified by `ls` inside the installed package. This means `npm install -g @earendil-works/pi-coding-agent@0.84.4` already pins the whole transitive tree.

Sibling packages, all at `latest = 0.84.4` and all having a `0.84.4` version (verified individually against the registry):

| package | 0.84.4 tarball shasum |
|---|---|
| `@earendil-works/pi-agent-core` | `451e9e76b6c7fecd2a49ed5bf905f8dd6c7ce876` |
| `@earendil-works/pi-ai` | `348f1be5c2a0f4d17cc167fe1e5a7cabb191d079` |
| `@earendil-works/pi-client` | `88523ba121aea1f57bae5d67656f09af7c6fcf02` |
| `@earendil-works/pi-protocol` | `aee0630eb3ce6844d3f68323a4a1fffc988fb0c0` |
| `@earendil-works/pi-server` | `bf1e5c43310671e21d4b06ace0db3da6d91501d9` |
| `@earendil-works/pi-telemetry` | `0eefd361fab1db773496384c87bfd91be2772790` |
| `@earendil-works/pi-tui` | `1b5bee5f22ba90539beaddac4e4ee7ad81c8a279` |

### Is there a newer stable release? **No.**

- `GET /releases?per_page=25` — newest is v0.84.4 (2026-08-28). Previous: v0.84.3 (2026-08-24), v0.84.2 (2026-08-14), v0.84.1 (2026-08-07), v0.84.0 (2026-08-06), v0.83.0 (2026-07-29).
- No draft or prerelease entries in the listing (`draft=False, prerelease=False` for every row).
- `GET /releases/latest` → v0.84.4.
- npm `dist-tags.latest` → 0.84.4.
- `GET /tags?per_page=30` — newest tag is v0.84.4.

**However, `main` has moved.** `GET /compare/v0.84.4...main` → `status: ahead, ahead_by: 8, behind_by: 0`. Those 8 commits:

```
853a80d26c  Add [Unreleased] section for next cycle
56c6fb33c4  fix(coding-agent): settle active turn before in-memory fork (#8937)
afda4d6202  fix(agent): stop prepared tools after preflight abort (#8936)
f2a6227899  feat(coding-agent): adjust TUI selections in thinking-mode, models and scoped models (#890x)
6492144773  fix(tui): detect Zed terminal capabilities (#8828)
62835ea81b  fix(coding-agent): use ctx.cwd for cwd-sensitive tools when available (#8627)
a63fb12c13  fix(ai): match subdomains and root domains in NO_PROXY (#8737)
3fc3ef532b  fix(coding-agent): keep theme markers visible (#8950)
```
`main`'s `packages/coding-agent/package.json` still says `version: 0.84.4`, and there is an `[Unreleased]` changelog section — i.e. **v0.84.5 is in flight but not cut.** Two of those commits (`56c6fb33c4` fork settling, `afda4d6202` preflight abort tool teardown) touch exactly the durability/lifecycle surface CG cares about; worth tracking, not worth unpinning for.

---

## 3. Idiomatic layout of a Pi "distribution" / config

### Config file locations and precedence

| Location | Scope |
|---|---|
| `~/.pi/agent/settings.json` | global (all projects) |
| `.pi/settings.json` | project (current directory) |

> `docs/settings.md` lines 5-8.

**Precedence:** project overrides global; **nested objects are merged, not replaced**. Worked example from `docs/settings.md` lines 348-369: global `{theme:"dark", compaction:{enabled:true, reserveTokens:16384}}` + project `{compaction:{reserveTokens:8192}}` → `{theme:"dark", compaction:{enabled:true, reserveTokens:8192}}`. One documented exception: "A project `defaultTools` array **replaces** the global array" (`docs/settings.md` line 244).

**Path resolution:** "Paths in `~/.pi/agent/settings.json` resolve relative to `~/.pi/agent`. Paths in `.pi/settings.json` resolve relative to `.pi`. Absolute paths and `~` are supported." (`docs/settings.md` line 281).

**Config directory name is not hardcoded.** From `packages/coding-agent/src/config.ts`:
```ts
export const PACKAGE_NAME: string = pkg.name || "@earendil-works/pi-coding-agent";
export const APP_NAME:  string = piConfigName || "pi";
export const APP_TITLE: string = piConfigName ? APP_NAME : "π";
export const CONFIG_DIR_NAME: string = pkg.piConfig?.configDir || ".pi";
export const VERSION: string = pkg.version || "0.0.0";
export const ENV_AGENT_DIR   = `${APP_NAME.toUpperCase()}_CODING_AGENT_DIR`;
export const ENV_SESSION_DIR = `${APP_NAME.toUpperCase()}_CODING_AGENT_SESSION_DIR`;
```
A `piConfig: { name, configDir }` block in the CLI's own `package.json` rebrands the whole harness (app name, TUI title, config dir, and even the env-var prefix). The published 0.84.4 package sets only `piConfig: {"configDir": ".pi"}` (verified locally). Extensions are told to use the exported `CONFIG_DIR_NAME` rather than hardcoding `.pi`, because "Rebranded distributions can use a different config directory name" (`docs/extensions.md` line 980).

> **This is the supported rebranding seam** if Command Governor ever wants to ship as `commandgovernor` with a `.commandgovernor` config dir — but it requires republishing a CLI package with a modified `piConfig`, i.e. a fork of `packages/coding-agent`. That is ADR-0008 tier-5 ("FORK Pi core"). Not recommended for v1.

### Resource discovery — full matrix

Every resource type is discovered from six kinds of source. Consolidated from `docs/skills.md` §Locations, `docs/prompt-templates.md` §Locations, `docs/themes.md` §Locations, `docs/extensions.md` §Extension Locations, `docs/settings.md` §Resources:

| Resource | Global | Project (trust-gated) | Package | Settings key | CLI flag | Disable |
|---|---|---|---|---|---|---|
| Extensions | `~/.pi/agent/extensions/*.ts`, `*/index.ts` | `.pi/extensions/*.ts`, `*/index.ts` | `pi.extensions` or `extensions/` | `extensions[]` | `-e/--extension` (repeatable) | `--no-extensions` |
| Skills | `~/.pi/agent/skills/`, `~/.agents/skills/` | `.pi/skills/`, `.agents/skills/` in cwd **and ancestors up to git root** | `pi.skills` or `skills/` | `skills[]` | `--skill` (repeatable, additive even with `--no-skills`) | `--no-skills` |
| Prompt templates | `~/.pi/agent/prompts/*.md` | `.pi/prompts/*.md` | `pi.prompts` or `prompts/` | `prompts[]` | `--prompt-template` | `--no-prompt-templates` |
| Themes | built-in `dark`/`light`, `~/.pi/agent/themes/*.json` | `.pi/themes/*.json` | `pi.themes` or `themes/` | `themes[]` | `--theme`, `--use-theme` | `--no-themes` |
| Context files | `~/.pi/agent/AGENTS.md` | `AGENTS.md`/`CLAUDE.md` walking up + cwd; `AGENTS.override.md` replaces both in its directory | — | — | — | `--no-context-files` / `-nc` |
| System prompt | `~/.pi/agent/SYSTEM.md`, `~/.pi/agent/APPEND_SYSTEM.md` | `.pi/SYSTEM.md`, `.pi/APPEND_SYSTEM.md` | — | — | `--system-prompt`, `--append-system-prompt` | — |

Prompt-template discovery in `prompts/` is **non-recursive** (`docs/prompt-templates.md` line 95). Skills discovery **is** recursive for directories containing `SKILL.md`.

Settings arrays support glob patterns, `!exclude`, `+force-include-exact-path`, `-force-exclude-exact-path` (`docs/settings.md` line 292).

Pi can load skills straight from other harnesses:
```json
{ "skills": ["~/.claude/skills", "~/.codex/skills"] }
```
> `docs/skills.md` lines 46-55.

### Project trust — the gate on everything project-local

From `docs/security.md` (authoritative) and `docs/settings.md` §Project Trust:

- A project "requires trust" if any of these exist: `.pi/settings.json`; `.pi/extensions|skills|prompts|themes`; `.pi/SYSTEM.md` or `.pi/APPEND_SYSTEM.md`; project `.agents/skills` in cwd or an ancestor. **A bare `.pi` directory does not count.**
- Decisions are stored by canonical directory in `~/.pi/agent/trust.json`; the closest saved decision on the current-or-parent path wins over the global default.
- **Non-interactive modes (`-p`, `--mode json`, `--mode rpc`) never prompt.** Without a saved decision they follow global `defaultProjectTrust`: `"ask"` (default) and `"never"` **ignore** project resources; `"always"` trusts them. `--approve`/`-a` and `--no-approve`/`-na` override for one run.
- Before trust resolves, Pi loads only context files, user/global extensions, and CLI `-e` extensions. Those can handle the `project_trust` event; **the first extension returning yes/no owns the decision** and suppresses the built-in prompt.
- Context files (`AGENTS.md`, `CLAUDE.md`, `AGENTS.override.md`) load **regardless** of trust.
- "Project trust is only an input-loading guard… It does not make untrusted code, untrusted prompts, or untrusted model output safe."

> **CG consequence:** any headless/orchestrated CG worker started with `--mode rpc` will silently ignore the project's `.pi/settings.json` unless CG explicitly passes `--approve` or the machine has `defaultProjectTrust: "always"` or a saved `trust.json` entry. This is a real footgun for a distribution whose whole point is a curated project-level loadout — a CG launch wrapper must handle it deterministically, not rely on the default.

### Package manifest format

Two forms, both in `packages/coding-agent/docs/packages.md`:

**Explicit manifest** — a `pi` key in `package.json`:
```json
{
  "name": "my-package",
  "keywords": ["pi-package"],
  "pi": {
    "extensions": ["./extensions"],
    "skills": ["./skills"],
    "prompts": ["./prompts"],
    "themes": ["./themes"],
    "video": "https://example.com/demo.mp4",
    "image": "https://example.com/screenshot.png"
  }
}
```
Paths are package-root-relative; arrays support globs and `!exclusions`. Positive globs discover **visible** paths in lexical order — dot-prefixed paths and symlinked resource roots must be listed directly.

The parser is `packages/coding-agent/src/core/pi-manifest.ts` (35 lines, read in full). It recognises **exactly four resource fields**:
```ts
const RESOURCE_FIELDS = ["extensions", "skills", "prompts", "themes"] as const;
```
Any entry that is not an array of strings is dropped silently; a malformed `package.json` yields `null`. **There is no `agents` field and no `tools` field.** (`video`/`image` are gallery metadata read elsewhere, not by this parser.)

**Convention directories** — used when no `pi` manifest exists:
- `extensions/` → `.ts` and `.js` files
- `skills/` → recursive `SKILL.md` folders + top-level `.md` files
- `prompts/` → `.md` files
- `themes/` → `.json` files

**Dependency rules** (`docs/packages.md` §Dependencies): runtime deps go in `dependencies` (Pi runs `npm install --omit=dev` on install, so `devDependencies` are unavailable at runtime). Pi-bundled packages must be `peerDependencies` with `"*"` and must not be bundled: `@earendil-works/pi-ai`, `@earendil-works/pi-agent-core`, `@earendil-works/pi-coding-agent`, `@earendil-works/pi-tui`, `typebox`. Other pi packages must go in both `dependencies` and `bundledDependencies` and be referenced through `node_modules/` paths — "Pi loads packages with separate module roots, so separate installs do not collide or share modules."

### Package manager: install, sources, scope, and the pin/lock story

```bash
pi install npm:@foo/bar@1.0.0
pi install git:github.com/user/repo@v1
pi install https://github.com/user/repo
pi install /absolute/path        # or ./relative/path
pi remove npm:@foo/bar
pi list
pi update --all | --extensions | --models | --self
pi config                        # TUI to enable/disable individual resources
pi -e npm:@foo/bar               # try without installing (temp dir, this run only)
```

**Scope:** `install`/`remove` write to `~/.pi/agent/settings.json` by default; `-l` writes to `.pi/settings.json`. Install roots: user → `~/.pi/agent/npm/`, `~/.pi/agent/git/<host>/<path>`; project → `.pi/npm/`, `.pi/git/<host>/<path>`.

> **The key line for a distribution:** "Project settings can be shared with your team, and **pi installs any missing packages automatically on startup after the project is trusted**." (`docs/packages.md` line 43.)

**Scope dedup** (`docs/packages.md` §Scope and Deduplication): if the same package appears in global and project settings, the project entry wins, unless it has `autoload: false`, in which case it is applied as a *delta* over the global entry. Identity = npm package name / git repo URL without ref / resolved absolute path.

**Package filtering** — object form in `packages[]` narrows what a package contributes:
```json
{ "source": "npm:my-package",
  "extensions": ["extensions/*.ts", "!extensions/legacy.ts"],
  "skills": [], "prompts": ["prompts/review.md"], "themes": ["+themes/legacy.json"] }
```
Omit a key → load all of that type; `[]` → load none. "Filters layer on top of the manifest. They narrow down what is already allowed."

**Is there a lockfile for pi packages? No — and this matters.**

Verified by reading `packages/coding-agent/src/core/package-manager.ts` (2699 lines) and `src/utils/git.ts`:

```ts
// package-manager.ts, parseSource() — npm branch
return { type: "npm", spec, name, version,
         range: getNpmVersionRange(version),
         pinned: isExactNpmVersion(version) };      // semver `valid()`, not `validRange()`
```
```ts
// utils/git.ts, GitSource
/** Git ref (branch, tag, commit) if specified */
ref?: string;
/** True if ref was specified (package won't be auto-updated) */
pinned: boolean;
```

So the pin **is the source string in `settings.json`**. There is no resolved-SHA lockfile, no `pi.lock`, no integrity record for installed pi packages.

- `npm:pkg@1.2.3` (exact semver) → `pinned: true`; `pi update --extensions` / `--all` skip it (`package-manager.ts:1104`, `:1210`).
- `npm:pkg` or `npm:pkg@^1` → not pinned; `pi update` resolves and moves it.
- `git:host/user/repo@<ref>` → `pinned: true` if any ref given. Updates "do not move them to newer refs, but they **do** reconcile an existing clone to the configured ref" — and "when reconciliation changes the checkout, pi resets and cleans the clone, then runs `npm install` if `package.json` exists" (`docs/packages.md` lines 90-93).
- `git:host/user/repo` with **no** ref → not pinned, tracks the default branch.

**The trap:** a git *tag* satisfies `pinned` but is mutable upstream. Only a **40-character commit SHA** is an immutable pin. Since Pi records nothing else, a distribution that wants reproducibility must write commit SHAs into `settings.json` itself.

Related: `npmCommand` in settings pins which npm binary/wrapper is used for all package operations, e.g. `["mise","exec","node@20","--","npm"]` (`docs/settings.md` lines 198-220).

---

## 4. Extension API surface

### Registration model

An extension is a TypeScript module with a **default-exported factory** receiving `ExtensionAPI`:
```ts
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
export default function (pi: ExtensionAPI) { /* pi.on / pi.registerTool / … */ }
```
Loaded via **jiti**, so TypeScript needs no compilation (`docs/extensions.md` line 179).

The factory may be **async**; Pi awaits it before continuing startup, and specifically before `session_start`, before `resources_discover`, and before flushing queued `pi.registerProvider()` calls (line 181).

**Critical lifecycle rule** (`docs/extensions.md` lines 220-224):
> "Extension factories may run in invocations that never start a session. **Do not start background resources such as processes, sockets, file watchers, or timers from the factory.** Defer background resource startup until `session_start` or the command/tool/event that needs the resource. Register an idempotent `session_shutdown` handler to close any session-scoped resources you start."

Available imports for extensions: `@earendil-works/pi-coding-agent`, `typebox`, `@earendil-works/pi-ai` (for `StringEnum`), `@earendil-works/pi-tui`, plus Node built-ins and any npm deps installed next to the extension.

### Full lifecycle event list (v0.84.4)

Reproducing the authoritative diagram from `docs/extensions.md` lines 277-349:

```
pi starts
  ├─► project_trust            (user/global + CLI -e extensions only, before project resources load)
  ├─► session_start            { reason: "startup" }
  └─► resources_discover       { reason: "startup" }

user sends prompt
  ├─► (extension commands checked first, bypass if found)
  ├─► input                    (intercept / transform / handle)
  ├─► (skill / template expansion if not handled)
  ├─► before_agent_start       (inject message, modify system prompt)
  ├─► agent_start
  ├─► message_start / message_update / message_end
  │   ┌─── turn (repeats while LLM calls tools) ───┐
  │   ├─► turn_start
  │   ├─► context                      (modify messages)
  │   ├─► before_provider_headers      (mutate headers in place)
  │   ├─► before_provider_request      (inspect or replace payload)
  │   ├─► after_provider_response      (status + headers, before stream consume)
  │   │     ├─► tool_execution_start
  │   │     ├─► tool_call              (CAN BLOCK; input is mutable)
  │   │     ├─► tool_execution_update
  │   │     ├─► tool_result            (CAN MODIFY; middleware-chained)
  │   │     └─► tool_execution_end
  │   └─► turn_end
  ├─► agent_end
  └─► agent_settled            (no retry / compaction / follow-up left)

/new or /resume
  ├─► session_before_switch    (can cancel)
  ├─► session_shutdown
  ├─► session_start            { reason: "new" | "resume", previousSessionFile? }
  └─► resources_discover       { reason: "startup" }

/fork or /clone
  ├─► session_before_fork      (can cancel)
  ├─► session_shutdown
  ├─► session_start            { reason: "fork", previousSessionFile }
  └─► resources_discover       { reason: "startup" }

/name or pi.setSessionName()  └─► session_info_changed
/compact or auto-compaction   ├─► session_before_compact (cancel or supply summary)
                              ├─► session_compact
                              └─► session_compact_failed
/tree navigation              ├─► session_before_tree (cancel or supply summary)
                              └─► session_tree
/model or Ctrl+P              ├─► thinking_level_select (if clamped)
                              └─► model_select
thinking level change         └─► thinking_level_select
exit (Ctrl+C/D, SIGHUP, SIGTERM) └─► session_shutdown

(plus, new in 0.84.4) ui_prompt_start / ui_prompt_end around blocking ctx.ui prompts
user `!` / `!!` commands       └─► user_bash (can intercept or fully replace)
```

Notes that matter for CG's reliability contract:

- **`agent_settled` is the right completion signal, not `agent_end`.** "`agent_end` fires when that run ends, but Pi may still auto-retry, auto-compact and retry, or continue with queued follow-up messages. Use `agent_settled` for status integrations that need to know Pi will not continue running automatically." (`docs/extensions.md` line 569.) `ctx.isIdle()` is true inside `agent_settled` "unless another extension started a new run."
- **`ui_prompt_start`/`ui_prompt_end` are notification-only, best-effort, and NOT awaited** before showing/closing the prompt; nested prompts coalesce into one outer span (lines 583-599). Do not build a correctness invariant on them.
- `tool_call` can `{ block: true, reason?, terminate? }`. `terminate` only applies to a blocked call, and "the agent stops early only when **every** finalized result in the batch is terminating" (line 793).
- `tool_call`'s `event.input` is **mutable in place**, mutations affect real execution, later handlers see earlier mutations, and **no re-validation is performed after mutation** (lines 788-792). A CG policy gate that rewrites arguments is therefore responsible for its own validation.
- Parallel tool mode is the **default**. `tool_call` is "not guaranteed to see sibling tool results from that same assistant message in `ctx.sessionManager`" (line 784). `tool_execution_end` and `tool_result` interleave in completion order while final `toolResult` message events are emitted in assistant source order (lines 655-659, 846).
- `session_before_compact` can `{ cancel: true }` or supply `{ compaction: { summary, firstKeptEntryId, tokensBefore, usage? } }` — i.e. an extension can fully own compaction content. `reason` ∈ `"manual" | "threshold" | "overflow"`, plus `willRetry`.
- `before_agent_start` returns `{ message?, systemPrompt? }` and **chains across extensions**; `event.systemPromptOptions` exposes the structured inputs (customPrompt, selectedTools, toolSnippets, promptGuidelines, appendSystemPrompt, cwd, contextFiles, skills).
- **Error handling:** "Extension errors are logged, agent continues. `tool_call` errors block the tool (fail-safe). Tool `execute` errors must be signaled by throwing" (lines 2919-2923).

### Custom tools

`pi.registerTool({ name, label, description, promptSnippet?, promptGuidelines?, parameters, prepareArguments?, execute, renderCall?, renderResult? })`, parameters as a **typebox** schema.

- Works **during load and after startup** — callable from `session_start`, command handlers, other event handlers. "New tools are refreshed immediately in the same session… callable by the LLM without `/reload`" (line 1369).
- `promptSnippet` opts the tool into the system prompt's `Available tools` list; without it a custom tool is omitted from that section.
- `promptGuidelines` bullets are appended **flat** to `Guidelines` with no tool-name prefix — the doc explicitly warns each bullet must name its own tool.
- Registering a tool with a built-in's name **overrides the built-in** (`docs/extensions.md` §Overriding Built-in Tools; example `tool-override.ts`).
- File-mutating custom tools **must** use `withFileMutationQueue(absolutePath, fn)` to share the per-file queue with built-in `edit`/`write`, or parallel execution silently loses writes (lines 1923-1950). Queue the whole read-modify-write window, and pass a resolved absolute path (the helper `realpath()`s existing files so symlink aliases share a queue).
- Built-in tools: `read`, `bash`, `powershell` (Windows), `edit`, `write`, `grep`, `find`, `ls`. `defaultTools` in settings selects which are on at startup; `--tools`/`--no-tools`/`--no-builtin-tools`/`--exclude-tools` override.

### Commands, shortcuts, flags, renderers

`pi.registerCommand(name, { description, handler, getArgumentCompletions? })`, `pi.registerShortcut`, `pi.registerFlag`, `pi.registerMessageRenderer`, `pi.registerEntryRenderer`, `pi.registerMarkdownTransformer`, `pi.registerProvider` / `unregisterProvider`, `pi.exec`, `pi.getActiveTools()/getAllTools()/setActiveTools()`, `pi.setModel`, `pi.get/setThinkingLevel`, `pi.events` (inter-extension event bus), `pi.getCommands()`.

Command-name collisions across extensions are **kept, not dropped** — Pi assigns numeric suffixes in load order (`/review:1`, `/review:2`) (line 1529).

### State persistence — three distinct mechanisms

1. **`pi.appendEntry(customType, data?)`** → writes a `CustomEntry` to the session JSONL. **Does NOT participate in LLM context.** Restored by scanning `ctx.sessionManager.getEntries()` on `session_start`. Optionally rendered in the TUI via `pi.registerEntryRenderer(customType, renderer)`.
2. **`pi.sendMessage({customType, content, display, details}, {deliverAs, triggerTurn})`** → a `CustomMessageEntry`, which **DOES** participate in LLM context. `deliverAs` ∈ `"steer"` (default; delivered after the current assistant turn's tool calls, before the next LLM call) | `"followUp"` (after the agent has no more tool calls) | `"nextTurn"` (queued for the next user prompt, interrupts nothing).
3. **Tool-result `details`** — the documented pattern for branch-correct state: "Extensions with state should store it in tool result `details` for proper branching support" (`docs/extensions.md` §State Management, lines 1877-1908), reconstructing by walking `ctx.sessionManager.getBranch()` on `session_start`.

Plus `pi.setLabel(entryId, label)` — labels "persist in the session and survive restarts", shown in `/tree`, readable via `ctx.sessionManager.getLabel(id)`. And `pi.setSessionName()` / `getSessionName()`.

> **CG note:** #3 (tool-result details) is the only one of the three that is automatically *branch-correct* — a `CustomEntry` written on an abandoned branch is still in the file but not on the active path. For CG's obligation/lineage records, "which branch was this written on" is exactly the question, so choose deliberately per record type.

### Session-control methods — commands only

`ExtensionCommandContext` extends `ExtensionContext` with `waitForIdle()`, `newSession(options?)`, `fork(entryId, options?)`, `navigateTree(targetId, options?)`, `switchSession(path, options?)`, `reload()`. These "are only available in commands because they can deadlock if called from event handlers" (line 1111).

The `withSession` callback pattern and its documented footguns (lines 1260-1301) are important for CG:
- `withSession` runs only after the old session emitted `session_shutdown`, the old runtime was torn down, the replacement was rebound, and the **new** extension instance already received `session_start`.
- But the callback still runs in the **original closure** — your old instance may already have cleaned up.
- Captured old `pi` / old command `ctx` session-bound objects are stale and **will throw**. A captured `ctx.sessionManager` is still the old object.
- "Only capture plain data that survives shutdown cleanly, such as strings, ids, and serialized config."

`ctx.reload()` is similarly documented as effectively terminal for the handler: "For predictable behavior, treat reload as terminal for that handler (`await ctx.reload(); return;`)."

### Other `ExtensionContext` surface

`ctx.ui` (select/confirm/input/editor/custom/notify/setStatus/setWidget/setFooter/setHeader/setTheme/...), `ctx.mode`, `ctx.hasUI`, `ctx.cwd`, `ctx.isProjectTrusted()`, `ctx.sessionManager` (read-only), `ctx.modelRegistry` / `ctx.model` / `ctx.thinkingLevel` / `ctx.scopedModels`, `ctx.signal` (agent abort signal; `undefined` when idle), `ctx.isIdle()` / `ctx.abort()` / `ctx.hasPendingMessages()`, `ctx.shutdown()`, `ctx.getContextUsage()`, `ctx.compact({customInstructions, onComplete, onError})`, `ctx.getSystemPrompt()`.

`ctx.getSystemPrompt()` caveat: it reports Pi's system-prompt *string*, and does **not** reflect `context` message mutations or `before_provider_request` payload rewrites.

### Mode behaviour table (`docs/extensions.md` lines 2925-2934)

| Mode | `ctx.mode` | `ctx.hasUI` | Notes |
|---|---|---|---|
| Interactive | `"tui"` | `true` | full TUI |
| RPC (`--mode rpc`) | `"rpc"` | `true` | dialogs/notifications over the JSON protocol; `custom()` returns `undefined` |
| JSON (`--mode json`) | `"json"` | `false` | event stream to stdout; UI methods are no-ops |
| Print (`-p`) | `"print"` | `false` | extensions run but cannot prompt |

`ctx.shutdown()` is deferred to idle in interactive and RPC modes, and is a **no-op in print mode**.

### RPC mode (`pi --mode rpc`)

Framing is strict JSONL, **LF only**:
> "Split records on `\n` only. Accept optional `\r\n` input by stripping a trailing `\r`. Do not use generic line readers that treat Unicode separators as newlines. In particular, **Node `readline` is not protocol-compliant for RPC mode** because it also splits on `U+2028` and `U+2029`, which are valid inside JSON strings."
> — `docs/rpc.md` lines 28-38.

Commands carry an optional `id` echoed in the response. Command families: Prompting (`prompt` with `streamingBehavior: "steer"|"followUp"`), State, Model, Thinking, Queue Modes (incl. **`clear_queue`**, new in 0.84.4), Compaction, Retry, Bash, **Session** (`get_session_stats`, `export_html`, `switch_session`, `fork`, `clone`, `get_fork_messages`, …), Commands (`get_commands`).

Session commands return `{cancelled: true|false}` because an extension's `session_before_switch` / `session_before_fork` handler can veto them — the RPC response is `success: true` with `data.cancelled: true`, **not** an error. A client that only checks `success` will silently mis-report a vetoed fork.

`get_session_stats` returns `sessionFile`, `sessionId`, message/tool counts, cumulative tokens and cost, and `contextUsage {tokens, contextWindow, percent}`. Documented caveat: "`contextUsage.tokens` and `contextUsage.percent` are `null` immediately after compaction until a fresh post-compaction assistant response provides valid usage data."

RPC event types (21): `agent_start`, `agent_end` (with `willRetry`), `agent_settled`, `turn_start`, `turn_end`, `message_start`, `message_update`, `message_end`, `bash_execution_update`, `tool_execution_start/update/end`, `queue_update`, `compaction_start/end`, `auto_retry_start/end`, `summarization_retry_scheduled` / `_attempt_start` / `_finished`, `extension_error`.

There is also an **Extension UI Protocol** over RPC (`docs/rpc.md` lines 1184-1375) so `ctx.ui.*` prompts round-trip to the RPC client as requests on stdout and responses on stdin.

Upstream's own guidance: "If you're building a Node.js application, consider using `AgentSession` directly from `@earendil-works/pi-coding-agent` instead of spawning a subprocess." RPC is preferred for other languages, process isolation, or language-agnostic clients (`docs/rpc.md` line 5; `docs/sdk.md` §RPC Mode Alternative).

### SDK (in-process)

```ts
import { createAgentSession, ModelRuntime, SessionManager } from "@earendil-works/pi-coding-agent";
const modelRuntime = await ModelRuntime.create();
const { session } = await createAgentSession({ sessionManager: SessionManager.inMemory(), modelRuntime });
session.subscribe(ev => { /* … */ });
await session.prompt("…");
```

`createAgentSession({cwd, agentDir, model, tools, sessionManager, resourceLoader, …})`. A custom `ResourceLoader` fully replaces discovery: `DefaultResourceLoader({ additionalExtensionPaths, extensionFactories, skillsOverride, promptsOverride, agentsFilesOverride, eventBus })`. Inline extensions can be named via `InlineExtension { name, factory }` so the startup list shows `<inline:my-provider>` instead of `<inline:1>`.

Run-mode helpers are exported: `InteractiveMode`, `runPrintMode(runtime, {mode:"text", initialMessage, initialImages, messages})`, `runRpcMode(runtime)`, built on `createAgentSessionRuntime` / `createAgentSessionServices` / `createAgentSessionFromServices`.

Documented exports include `SessionManager`, `SettingsManager`, `ModelRuntime`, `ModelRegistry`, `DefaultResourceLoader`, `createEventBus`, `CONFIG_DIR_NAME`, `defineTool`, `getAgentDir`, `getPackageDir`, `createCodingTools`, `createReadOnlyTools`, and the individual `create*Tool` factories.

### Print / JSON mode for smoke tests

- `pi -p "prompt"` — prints the response and exits; reads piped stdin and merges it into the initial prompt.
- `pi --mode json "prompt"` — every session event as JSON lines on stdout. First line is the session header `{"type":"session","version":3,"id":…,"cwd":…}`. `message_update` records are **delta-only** (no cumulative `message`, no `assistantMessageEvent.partial`) to keep stream size linear; `message_end` carries the final authoritative message.
- Documented smoke-test idiom: `pi --mode json "List files" 2>/dev/null | jq -c 'select(.type == "message_end")'`.

**Verified locally:** `pi --version` exits 0; `PI_OFFLINE=1 pi -p --no-session "hello"` with no credentials prints "No API key found for the selected model." and **exits 1**. I did **not** run a credentialed end-to-end print/RPC round-trip — no provider key was available in this environment. **UNVERIFIED: real agent turn behaviour, RPC command/response round-trip, and `agent_settled` timing under load.**

---

## 5. Agent roles and skills

### Agent roles: not a Pi core concept

This is a firm negative finding, supported three ways:

1. `packages/coding-agent/src/core/pi-manifest.ts` recognises exactly `["extensions","skills","prompts","themes"]`. No `agents`.
2. `docs/usage.md` §Design Principles: Pi "intentionally does not include built-in MCP, **sub-agents**, permission popups, plan mode, to-dos, or background bash."
3. Grepping all of `packages/coding-agent/docs/` for "subagent"/"sub-agent" yields exactly three hits: an examples-table row (`subagent/` — "Spawn sub-agents", using `registerTool` + `exec`), the design-principles disclaimer, and an SDK bullet ("Build custom tools that spawn sub-agents"). No role/frontmatter format is defined anywhere in core.

`~/.agents/skills/` and project `.agents/skills/` are **skill** directories under the Agent Skills convention — they are not agent-role definitions. Do not confuse the two.

**The community convention** (which CG would be adopting, not inheriting) is a markdown file with YAML frontmatter, loaded by a subagent *extension*. Concrete example from `amosblomqvist/pi-config`, `deprecated/extensions/subagents/agents/worker.md`:

```markdown
---
name: worker
description: General-purpose worker — reads, writes, and edits code
tools: read, write, edit, safe_bash
model: anthropic/claude-sonnet-4-6
---
You are a worker agent. You operate in an isolated context — you have no knowledge of any prior conversation.
…
```
Sibling files: `agents/researcher.md`, `agents/scout.md`. The loader is `deprecated/extensions/subagents/index.ts`; a `tools/safe-bash.ts` supplies the restricted bash tool named in frontmatter.

> **CG consequence:** the `tools:` and `model:` frontmatter fields are enforced by *whatever extension reads them*, not by Pi. ADR-0008 invariant 6 ("worker/subagent roles and resumed loadouts are explicit and least-authority; resume cannot silently broaden an old worker under new defaults") therefore has **no core enforcement point** — CG owns it end to end, in its own subagent extension, or it does not hold.

### Skills

Pi implements the **Agent Skills standard** (https://agentskills.io/specification), "warning about most violations but remaining lenient." One deliberate divergence: "Pi allows skill names to differ from their parent directory even though the standard disallows it; that rule is suboptimal for shared skill directories used across multiple agent harnesses." (`docs/skills.md` line 7.)

**Structure:** a directory containing `SKILL.md`; everything else freeform (`scripts/`, `references/`, `assets/` by convention). Relative paths from the skill directory.

**Frontmatter** (`docs/skills.md` §Frontmatter):

| Field | Required | Notes |
|---|---|---|
| `name` | yes | ≤64 chars, `[a-z0-9-]`, no leading/trailing/consecutive hyphens |
| `description` | yes | ≤1024 chars; this is what drives model-side selection |
| `license` | no | |
| `compatibility` | no | ≤500 chars, environment requirements |
| `metadata` | no | arbitrary key-value map |
| `allowed-tools` | no | space-delimited pre-approved tools (**experimental**) |
| `disable-model-invocation` | no | when `true`, hidden from the system prompt; only reachable via `/skill:name` |

Unknown frontmatter fields are ignored.

**Loading — progressive disclosure** (`docs/skills.md` §How Skills Work):
1. At startup Pi scans skill locations and extracts names + descriptions.
2. The system prompt includes available skills in XML per the spec.
3. When a task matches, the agent uses `read` to load the full `SKILL.md` — *"models don't always do this; use prompting or `/skill:name` to force it."*
4. The agent follows the instructions using relative paths.

**Discovery precision** (lines 37-41):
- In `~/.pi/agent/skills/` and `.pi/skills/`: root `.md` files count as individual skills when they have valid frontmatter with a non-empty `description`.
- In **all** locations: directories containing `SKILL.md` are found recursively.
- In `~/.agents/skills/` and project `.agents/skills/`: root `.md` files are **ignored**; nested `.md` files in grouping folders are found when they declare skill frontmatter.
- Malformed `SKILL.md`, or one without a description, warns and does **not** load. Name collisions warn and **keep the first found**.

**Skill commands:** skills register as `/skill:name`; arguments are appended to the skill content as `User: <args>`. Toggled by `enableSkillCommands` (default `true`).

### Prompt templates

Markdown with optional frontmatter `description` and `argument-hint`; filename minus `.md` becomes the slash command. Substitutions: `$1`, `$2`, `$@` / `$ARGUMENTS`, `${1:-default}`, `${@:-default}`, `${@:N}`, `${@:N:L}`. Discovery in `prompts/` is **non-recursive**.

**Input processing order** (`docs/extensions.md` lines 915-920) — worth memorising, because it determines what a CG policy extension can intercept:
1. Extension commands (`/cmd`) — if matched, the handler runs and the `input` event is **skipped**.
2. `input` event — can `continue` / `transform` / `handled`.
3. Skill commands (`/skill:name`) expanded, if not handled.
4. Prompt templates expanded, if not handled.
5. Agent processing (`before_agent_start`, …).

---

## 6. Version and compatibility checking

### Programmatic access — yes, two ways

1. **CLI:** `pi --version` / `pi -v`. Implemented at `packages/coding-agent/src/main.ts:615` (`console.log(VERSION)`). Verified locally: prints `0.84.4`, exit code 0.
2. **SDK constant:** `VERSION` is a public export.
   ```ts
   // packages/coding-agent/src/index.ts, lines 5-15
   export { CONFIG_DIR_NAME, getAgentDir, getDocsPath, getExamplesPath,
            getPackageDir, getReadmePath, VERSION } from "./config.ts";
   ```
   ```ts
   // packages/coding-agent/src/config.ts
   export const VERSION: string = pkg.version || "0.0.0";
   ```
   `pkg` is the CLI package's own `package.json`, read at module load. Note the fallback: if the package.json cannot be read, `VERSION` is `"0.0.0"` — a guard must treat `"0.0.0"` as "unknown", not "ancient".

Also exported and useful: `PACKAGE_NAME`, `APP_NAME`, `CONFIG_DIR_NAME`, `getAgentDir()`, `getPackageDir()`, `getDocsPath()`.

Shell tools additionally receive `PI_SESSION_ID`, `PI_SESSION_FILE`, `PI_PROVIDER`, `PI_MODEL`, `PI_REASONING_LEVEL`, and every Pi child process inherits `AI_AGENT=pi` and `PI_CODING_AGENT=true` (`docs/environment-variables.md`). Those markers are **not** set when Pi is embedded through the SDK.

### "This config requires pi >= X" — **there is no built-in mechanism**

Verified negatively: grepping `packages/coding-agent/src/` for `minPiVersion`, `requiresPi`, `piVersion`, `satisfies(VERSION`, and `engines` returned **zero** hits. `VERSION` is used only for `--version`, the self-update check (`package-manager-cli.ts`), the User-Agent string, the changelog gate, and the interactive header.

The `pi` manifest parser accepts only the four resource arrays, so a `"pi": { "minVersion": … }` field would be silently ignored. npm `engines` constrains Node, not Pi.

**Recommended CG approach (must be built, not adopted):** a first-loading CG extension that reads `VERSION` and fails loud.

```ts
import { VERSION, type ExtensionAPI } from "@earendil-works/pi-coding-agent";

const REQUIRED = "0.84.4";   // exact pin for the CG distribution

export default function (pi: ExtensionAPI) {
  pi.on("session_start", async (_e, ctx) => {
    if (VERSION !== REQUIRED) {
      const msg = `Command Governor requires pi ${REQUIRED}; found ${VERSION}.`;
      if (ctx.hasUI) ctx.ui.notify(msg, "error");
      console.error(msg);
      ctx.shutdown();            // no-op in print mode — see caveat below
    }
  });
}
```

Three caveats the implementation must respect:
- Do the check in `session_start`, **not** in the factory — factories run in invocations that never start a session (`pi list`, `pi config`, `pi update`), and failing there would break package management.
- `ctx.shutdown()` is a **no-op in print mode**, and is deferred to idle in interactive/RPC. It is not a hard abort. For a genuinely blocking gate, combine the notify with a `tool_call` handler that returns `{ block: true }`, or check the version in the CG launcher *before* spawning `pi`.
- For range comparison rather than equality, add `semver` to the CG package's own `dependencies` — do **not** reach into pi's bundled `semver`, because only `pi-ai`, `pi-agent-core`, `pi-coding-agent`, `pi-tui`, and `typebox` are contractually available to extensions.

A belt-and-braces version is a launcher-side preflight: `test "$(pi --version)" = "0.84.4"` before starting any CG worker.

---

## 7. Session persistence

### Location and naming

```
~/.pi/agent/sessions/--<path>--/<timestamp>_<uuid>.jsonl
```
where `<path>` is the working directory with `/` replaced by `-` (`docs/session-format.md` lines 5-11).

Overridable, in precedence order: `--session-dir` > `PI_CODING_AGENT_SESSION_DIR` > `sessionDir` in settings.json (`docs/settings.md` line 256). Whole agent dir overridable via `PI_CODING_AGENT_DIR`.

### Format

JSONL, one JSON object per line, each with a `type`. **Current version 3.** v1 = linear (legacy), v2 = tree with `id`/`parentId`, v3 = renamed `hookMessage` role to `custom`. Older sessions auto-migrate on load.

`SessionEntryBase`: `{ type, id (8-char hex), parentId (string|null), timestamp (ISO) }`. The header line is `{"type":"session","version":3,"id":"<uuid>","timestamp":…,"cwd":…}` and is **not** part of the tree; forked/cloned sessions add `"parentSession": "/path/to/original/session.jsonl"`.

Entry types: `session` (header), `message`, `model_change`, `thinking_level_change`, `compaction`, `branch_summary`, `custom`, `custom_message`, `label`, `session_info`.

`AgentMessage` union: `UserMessage | AssistantMessage | ToolResultMessage | BashExecutionMessage | CustomMessage | BranchSummaryMessage | CompactionSummaryMessage`.

Two facts with direct CG relevance:
- `CustomEntry` (`type:"custom"`) — extension state persistence, **does not participate in LLM context**, keyed by `customType`.
- `CustomMessageEntry` (`type:"custom_message"`) — extension-injected content that **does** participate in LLM context, with `display: true|false`.

### Tree, resume, fork, clone

Sessions are **trees**, not logs. Every entry has `id`/`parentId`; the current position is the active leaf. Branching happens in place, in the same file.

| | `/tree` | `/fork` | `/clone` |
|---|---|---|---|
| Output | same session file | new session file | new session file |
| View | full tree | user-message selector | current active branch |
| Typical use | explore alternatives in place | new session from an earlier prompt | duplicate current work before continuing |
| Summary | optional branch summary | none | none |

> `docs/sessions.md` lines 118-127.

CLI: `pi -c` (continue most recent), `pi -r` (browse), `pi --session <path|id>` (partial UUID accepted), `pi --fork <path|id>`, `pi --no-session` (ephemeral), `pi --name/-n`.

**Undocumented-in-usage.md but present in `--help` and in `src/cli/args.ts:125-126`:**
```
--session-id <id>    Use exact project session ID, creating it if missing
```
This is a **deterministic, idempotent session address** — start-or-attach by a caller-chosen id. For durable orchestration that is materially better than "continue most recent", and it is the single most useful undocumented flag I found. It appears in `pi --help` output at `args.ts:288` but not in `docs/usage.md`'s Session Options table, so treat it as supported-but-lightly-documented.

### Context reconstruction

`buildContextEntries()` walks leaf → root, honouring compaction:
1. collect all entries on the path;
2. if a `CompactionEntry` is on the path, include it first; if it has `retainedTail`, it is a **self-contained checkpoint** and entries after the compaction are included; otherwise include entries from `firstKeptEntryId` to the compaction;
3. then entries after the compaction;
4. preserve non-message entries in range so interactive mode can render them.

`buildSessionContext()` then maps entries → messages: `message` → stored `AgentMessage`; `compaction` → `compactionSummary` + `retainedTail`; `branch_summary` → `branchSummary`; `custom_message` → `CustomMessage`; **`custom` → no context message**.

> "Newer harness-generated compactions include `retainedTail` so we can rebuild context from this checkpoint without walking older entries before the compaction entry."

### `SessionManager` API (public, exported from the SDK)

Statics: `create(cwd, sessionDir?)`, `open(path, sessionDir?)`, `continueRecent(cwd, sessionDir?)`, `inMemory(cwd?)`, `forkFrom(sourcePath, targetCwd, sessionDir?)`, `list(cwd, sessionDir?, onProgress?)`, `listAll(onProgress?)`.

Instance — session: `newSession({parentSession?})`, `setSessionFile(path)`, `createBranchedSession(leafId)`.
Instance — append (all return an entry id): `appendMessage`, `appendThinkingLevelChange`, `appendModelChange`, `appendCompaction`, `appendCustomEntry`, `appendSessionInfo`, `appendCustomMessageEntry`, `appendLabelChange`.
Instance — tree: `getLeafId`, `getLeafEntry`, `getEntry`, `getBranch(fromId?)`, `getTree`, `getChildren`, `getLabel`, `branch(entryId)`, `resetLeaf()`, `branchWithSummary(entryId, summary, details?, fromHook?)`.
Instance — context/info: `buildContextEntries`, `buildSessionContext`, `getEntries`, `getHeader`, `getSessionName`, `getCwd`, `getSessionDir`, `getSessionId`, `getSessionFile` (undefined for in-memory), `isPersisted`.

### Durability notes for CG

- Sessions are **append-only JSONL on the local filesystem**. There is no transaction, no fsync guarantee documented, and no server-side copy. `proper-lockfile@4.1.2` is a direct dependency of the CLI, so *some* locking exists, but I did not trace where it is applied. **UNVERIFIED: concurrent-writer safety of a single session file.**
- v0.84.4 explicitly fixed "resumed sessions corrupting the next appended entry when their JSONL file lacks a trailing newline" — evidence that JSONL append durability has had real bugs recently, and a reason to pin rather than track a moving target.
- `~/.pi/agent/` holds sessions, settings, credentials (`auth.json`), models, trust decisions, installed packages, and the debug log. `docs/security.md` advises: "avoid mounting host `~/.pi/agent` unless the container should access host sessions, settings, and credentials."
- `/export` writes HTML or JSONL; `/import <file>` imports and resumes a JSONL session; `/share` uploads a private GitHub gist. Deleting a session = deleting its `.jsonl` (Pi prefers the `trash` CLI when available).

---

## 8. How other serious Pi distributions structure themselves

Read-only reference. Both fetched via the GitHub API on 2026-09-01; neither is being copied.

### `HazAT/pi-config` — "clone the repo *as* `~/.pi/agent`"

46 files. `pushed_at: 2026-08-25`. Description: "My personal pi coding agent configuration - skills and extensions."

Layout:
```
package.json          # {"name":"pi-config","keywords":["pi-package"], no `pi` manifest → convention dirs}
settings.json         # ships as the global settings file itself
models.json  mcp.json
AGENTS.md             # global context file
setup.sh  link-claude.sh
npm/.gitignore        # placeholder for pi-managed package installs
extensions/execute-command/index.ts
prompts/*.md          # plan, todos, execute, review, visualize-plan, superconductor-setup
skills/<name>/SKILL.md (+ references/, scripts/)
```

Install model (from its README):
```bash
mkdir -p ~/.pi
git clone git@github.com:HazAT/pi-config ~/.pi/agent
cd ~/.pi/agent && ./setup.sh
```
`setup.sh` **refuses to run unless `$SCRIPT_DIR == $HOME/.pi/agent`**, creates `settings.json` only if absent (preserving an existing one), then runs `pi install …` for each git package, then installs a macOS runtime with `uv`, then links skills/prompts into Claude Code.

Its `settings.json`:
```json
{ "lastChangelogVersion": "0.84.2",
  "defaultProvider": "openai-codex", "defaultModel": "gpt-5.6-sol", "defaultThinkingLevel": "high",
  "packages": [ "git:github.com/pasky/chrome-cdp-skill",
                "git:github.com/HazAT/pi-parallel",
                "git:github.com/HazAT/pi-macos-harness",
                "git:github.com/nicobailon/visual-explainer" ],
  "hideThinkingBlock": false, "enabledModels": [], "theme": "dark" }
```
**Note: every package source is unpinned** (no `@ref`). Reproducibility is explicitly not a goal there. CG must not copy that.

Two structural ideas worth stealing:
- **Verification block in the README** — `jq empty settings.json models.json mcp.json package.json` plus a `pi --no-extensions --no-context-files --skill … --prompt-template … --list-models …` smoke invocation. That is exactly the shape of a conformance check that needs no credentials.
- **State kept as plain Markdown keyed off `PI_SESSION_FILE`**: `${PI_SESSION_FILE%.jsonl}.plan.md`, `.todos.md`, `.review.md`. Explicitly: "There is no sidecar directory, repository run store, membership system, or custom JSONL state. The current coordinator owns all writes." A deliberately minimal durability model that CG should evaluate against — and probably reject, since CG's invariants demand correlated identities and reconciliation that flat Markdown cannot carry.

### `amosblomqvist/pi-config` — "browse and copy pieces"

71 files. `pushed_at: 2026-08-25`. `DivMode/pi-config` is a **fork of this repo** (confirmed via the API: `fork: true, parent: amosblomqvist/pi-config`, same `pushed_at`), which matches ADR-0008's account.

> "This is **not** meant to be installed as one big package. Browse the repo and copy the pieces you want into your own Pi config." … "Avoid cloning this repo directly into `~/.pi/agent` unless it is a fresh setup."

Layout:
```
extensions/<name>.ts               # single-file
extensions/<name>/index.ts + package.json + package-lock.json   # directory + own deps
extensions/prompt-snippets/snippets/*.md
skills/<name>/SKILL.md + scripts/ + references/
deprecated/…                       # kept for reference, explicitly not in active use
```
Big extensions are promoted to their own repositories: `pi-interactive-subagents`, `pi-observational-memory`, `pi-dictate` — with **stub directories left behind** in the config repo pointing at them. That is a clean pattern for CG: keep the distribution repo as the curated index, let heavyweight components live and version independently.

Per-extension npm deps are kept **with the extension** (`extensions/browser/package.json` + `package-lock.json`), matching `docs/extensions.md` §Extension Styles.

The `deprecated/extensions/subagents/` tree is the concrete agent-role reference (see §5): `index.ts` loader + `agents/{worker,researcher,scout}.md` + `tools/safe-bash.ts`.

### What the two have in common, and what neither does

Common: convention directories mirroring `~/.pi/agent`; `settings.json` as the top-level composition point; `packages[]` as the third-party mechanism; skills as `SKILL.md` directories; prompt templates as flat `.md`.

**Neither pins anything.** Neither records the pi version they target beyond a `lastChangelogVersion` string. Neither has a conformance suite. Neither uses project-scoped `.pi/settings.json`. For a *personal* config that is fine; for a distribution with a reliability contract it is precisely the gap CG exists to fill.

---

## 9. Recommendation for the Command Governor distribution

### Repo layout

Keep `DivMode/commandgovernor` as the product repo and add a Pi-package root to it. Because the repo also contains the frozen Rust tree, use an **explicit `pi` manifest** pointing into a subdirectory rather than top-level convention dirs:

```
commandgovernor/
├─ package.json                    # the Pi package manifest (see below)
├─ harness/
│  ├─ extensions/                  # CG extensions (TS, jiti-loaded)
│  │  ├─ cg-version-guard/         # §6 — asserts pi == pinned version
│  │  ├─ cg-obligations/           # durable foreman events, appendEntry/tool-details
│  │  ├─ cg-subagent/              # roles + least-authority loadouts (§5 gap)
│  │  ├─ cg-policy-gate/           # tool_call blocking, protected paths
│  │  └─ cg-foreman-transport/     # ChatGPT bridge, capability-gated per ADR-0008 §8
│  ├─ skills/<name>/SKILL.md
│  ├─ prompts/*.md
│  ├─ themes/*.json
│  ├─ agents/*.md                  # CG-owned role frontmatter; read by cg-subagent
│  └─ profiles/
│     ├─ foreman/.pi/settings.json # project-settings templates per role
│     └─ worker/.pi/settings.json
├─ pins/
│  ├─ pi-0.84.4/package.json           # verbatim release asset
│  ├─ pi-0.84.4/package-lock.json      # verbatim release asset
│  ├─ SHA256SUMS                       # verbatim release asset
│  └─ pins.json                        # CG-owned: pi tag+sha, npm integrity, 3rd-party SHAs
├─ conformance/                     # credential-free + credentialed suites
└─ .pi/settings.json                # dogfooding: CG's own project loadout
```

`package.json`:
```json
{
  "name": "@divmode/command-governor",
  "version": "0.1.0",
  "keywords": ["pi-package"],
  "pi": {
    "extensions": ["./harness/extensions"],
    "skills":     ["./harness/skills"],
    "prompts":    ["./harness/prompts"],
    "themes":     ["./harness/themes"]
  },
  "peerDependencies": {
    "@earendil-works/pi-coding-agent": "*",
    "@earendil-works/pi-agent-core": "*",
    "@earendil-works/pi-ai": "*",
    "@earendil-works/pi-tui": "*",
    "typebox": "*"
  },
  "dependencies": { "semver": "<exact>" }
}
```
`harness/agents/` is deliberately **outside** the `pi` manifest — Pi would not know what to do with it, and `cg-subagent` reads it directly.

Consumers install with a **commit-pinned** git source:
```bash
pi install -l git:github.com/DivMode/commandgovernor@<40-char-sha>
```
`-l` writes it into the consuming project's `.pi/settings.json`, which Pi then auto-installs on trusted startup.

### Pin / lock mechanism — three layers

1. **Pi runtime.** Vendor the release's own lock artifacts into `pins/pi-0.84.4/`:
   - `pi-coding-agent-install-package.json` (sha256 `053e1c9f456f4863098baa02379c83fa92ddd70c1c61616df48548924d0caf19`)
   - `pi-coding-agent-install-package-lock.json` (sha256 `a9f805a677f0860328059390b0f62adcc655299952f293127a8db8939818dff4`)

   Install with `npm ci --ignore-scripts` against that lock root. This reproduces all 137 packages by `resolved` + `integrity`, and it is upstream's *own* installer input, so it will not drift from what `pi update` produces. Belt and braces: assert `npm view` integrity equals `sha512-jmOlrqUmvhh/siNWFRXjYLJzhKFIHNsAQaysRwzQPQFnPAaV/vhqHsLH/MBsIISA1Rjj7WTUFR3nJrpXoLx39w==`, and record git tag `v0.84.4` = `b79e4cc834970cca69daebffab7df1da7d1e52c4`.

   For air-gapped or Nix-style builds, prefer the standalone binary + its `SHA256SUMS` entry (e.g. `pi-darwin-arm64.tar.gz` = `c68e3ac4d05b4e282aaab2e6c76f161d3e9e68f19a22e38913cbfaadb6c800f0`), or build from `pi-0.84.4-source.tar.gz` (`ca3958…`) with `./scripts/build-binaries.sh --offline-model-data`.

2. **Third-party Pi packages.** In `settings.json`, use **exact npm versions** (`npm:pkg@1.2.3`) or **40-char git commit SHAs** (`git:github.com/o/r@<sha>`). Never a bare name, never a branch, and *never a tag* — Pi's `pinned` flag is satisfied by any ref, but only a SHA is immutable. Mirror every entry into `pins/pins.json` with the resolved SHA and the date it was reviewed, because Pi itself keeps no lockfile for these.

3. **CG's own package.** Consumers pin `git:github.com/DivMode/commandgovernor@<sha>`; CG's own `dependencies` use exact versions with a committed `package-lock.json`.

### Version assertion

Ship `harness/extensions/cg-version-guard/` implementing §6 (read the exported `VERSION`, compare in `session_start`, notify + `ctx.shutdown()`), **and** a launcher preflight `test "$(pi --version)" = "0.84.4"`, because `ctx.shutdown()` is a no-op in print mode.

### Trust handling — do not leave to default

Every CG worker launch must be explicit about project trust. Headless modes never prompt and, under the default `defaultProjectTrust: "ask"`, will **silently ignore** the project's `.pi/settings.json` — meaning a CG worker would run with none of its curated loadout and no error. Pass `--approve` deliberately, or have `cg-version-guard` (a *user/global* extension, which loads pre-trust) own the `project_trust` decision by returning `{trusted:"yes", remember:true}` for known-good roots.

### Conformance suite

Two tiers:
- **Credential-free** (runs in CI): `jq empty` over every JSON file; `pi --version` equals the pin; `pi --no-extensions --no-context-files --skill … --prompt-template …` loads each resource without error; JSON-schema validation of `harness/agents/*.md` frontmatter; assert `pi -p` exits **1** with no credentials (verified: it does).
- **Credentialed** (gated): `pi --mode json` smoke per role, asserting the `agent_settled` event arrives; RPC fork/switch round-trips asserting `data.cancelled` is handled; a compaction test asserting CG's `session_before_compact` handler is what produced the entry (`fromHook: true`).

### Tradeoffs

- **Pinning to 0.84.4 while `main` is 8 commits ahead** means CG will not get `56c6fb33c4` (settle active turn before in-memory fork) or `afda4d6202` (stop prepared tools after preflight abort) until it re-pins. Both are in CG's blast radius. Accept the pin; open a tracking issue for v0.84.5.
- **Pi's release cadence is fast** — six releases in the ~5 weeks from v0.83.0 (2026-07-29) to v0.84.4 (2026-08-28). A pinned distribution will need a scheduled re-pin ritual with the conformance suite as the gate, not an ad-hoc upgrade.
- **No lockfile for pi packages** means CG's `pins.json` is a CG-maintained artifact with no upstream enforcement. If someone runs `pi install` by hand in a CG project, `settings.json` and `pins.json` will disagree silently. A conformance check should diff them.
- **Extensions run with full user permissions and no sandbox.** Composing third-party pi packages is composing arbitrary code. ADR-0008's "COMPOSE a reviewed Pi package" step must mean *reviewed at a specific SHA*, and the pin is what makes the review meaningful.
- **The rebranding seam (`piConfig`) is tempting and should be resisted for v1.** It gives `commandgovernor` as the binary name and `.commandgovernor` as the config dir, but it requires forking and republishing `packages/coding-agent` — ADR-0008 tier 5, with a permanent merge burden against a fast-moving upstream.

---

## 10. Source index

**Live endpoints fetched 2026-09-01**
- `https://api.github.com/repos/earendil-works/pi`
- `https://api.github.com/repos/earendil-works/pi/git/ref/tags/v0.84.4`
- `https://api.github.com/repos/earendil-works/pi/releases/tags/v0.84.4`
- `https://api.github.com/repos/earendil-works/pi/releases/latest`
- `https://api.github.com/repos/earendil-works/pi/releases?per_page=25`
- `https://api.github.com/repos/earendil-works/pi/tags?per_page=30`
- `https://api.github.com/repos/earendil-works/pi/compare/v0.84.4...main`
- `https://registry.npmjs.org/@earendil-works/pi-coding-agent` (+ `pi-ai`, `pi-agent-core`, `pi-tui`, `pi-client`, `pi-protocol`, `pi-server`, `pi-telemetry`)
- `https://github.com/earendil-works/pi/releases/download/v0.84.4/{SHA256SUMS, pi-coding-agent-install-package.json, pi-coding-agent-install-package-lock.json}`
- `https://raw.githubusercontent.com/earendil-works/pi/v0.84.4/packages/coding-agent/docs/extensions.md` (HTTP 200 spot-check)
- `https://pi.dev/docs/latest` (HTTP 200)
- `https://api.github.com/repos/{amosblomqvist,HazAT,DivMode}/pi-config` + their `git/trees/main?recursive=1`
- `https://raw.githubusercontent.com/HazAT/pi-config/main/{package.json,settings.json,README.md,setup.sh,AGENTS.md}`
- `https://raw.githubusercontent.com/amosblomqvist/pi-config/main/{README.md,deprecated/extensions/subagents/agents/worker.md}`

**Files read in the v0.84.4 clone** (paths relative to the repo root; permanent URLs are `https://github.com/earendil-works/pi/blob/v0.84.4/<path>`)
- `README.md`, `package.json`, `packages/*/package.json`
- `packages/coding-agent/src/config.ts`
- `packages/coding-agent/src/index.ts`
- `packages/coding-agent/src/core/pi-manifest.ts`
- `packages/coding-agent/src/core/package-manager.ts` (targeted reads)
- `packages/coding-agent/src/utils/git.ts`
- `packages/coding-agent/src/cli/args.ts` (targeted)
- `scripts/generate-coding-agent-install-lock.mjs`
- `packages/coding-agent/docs/`: `index.md`, `quickstart.md`, `usage.md`, `settings.md`, `packages.md`, `skills.md`, `prompt-templates.md`, `themes.md`, `sessions.md`, `session-format.md`, `extensions.md`, `rpc.md`, `sdk.md`, `json.md`, `environment-variables.md`, `security.md`, `compaction.md`

**Explicitly UNVERIFIED**
- Credentialed end-to-end print/JSON/RPC round-trip (no provider key in this environment).
- `agent_settled` timing under retry/compaction load.
- Concurrent-writer safety of a single session JSONL file (`proper-lockfile` is a dependency; I did not trace its call sites).
- The `node >= 22.19.0` floor itself (local Node was v24.19.0).
- Whether `pi update --self` on a managed install preserves the vendored lock (docs describe a staged, lockfile-backed release; not exercised).
