# Command Governor

Command Governor is a curated, pinned distribution of [Prime Agent](https://github.com/PrimeIntellect-ai/prime-agent)
for durable, foreman-led AI/software-engineering work.

[commandgovernor.com](https://commandgovernor.com)

## What it is

```text
Prime Agent v0.9.1 (release assets verified against two checksum authorities)
  + pi-tasks                 durable task/evidence contract
  + @gotgenes/pi-subagents   delegation runtime; Command Governor's roles are its agent files
  + pi-pr-review             GitHub review lane with reviewed-head binding
  + @commandgovernor/harness skills, prompts, role files, project settings
  + pins/ and scripts/       manifest, checksums, install root, bootstrap
  + conformance/             black-box tests through stock Prime clients
```

**Command Governor contains no custom production code.** Everything that
executes is Prime Agent or a package Prime loads. The repository ships a
manifest, two shell scripts, configuration, prose, and tests. That is not a
minimalism preference; it is the result of a proof
([`docs/research/2026-09-04-zero-custom-code-proof.md`](docs/research/2026-09-04-zero-custom-code-proof.md))
that ran every product requirement through the surfaces a user actually
runs and found that the substrate and existing packages already satisfy
them, or that the gap is one no local code could close.

## Current architecture

Read in order:

1. [ADR 0008](docs/adr/0008-adopt-pi-native-command-governor-harness.md) — Command Governor is a Pi-family distribution, not a runtime.
2. [ADR 0009](docs/adr/0009-prime-agent-substrate-and-acp-boundary.md) — Prime Agent is the substrate (Accepted 2026-09-04).
3. [ADR 0010](docs/adr/0010-keep-command-governor-composition-first.md) — every capability is `USE EXISTING`, `PLUGIN`, `TEMP WORKAROUND` or `DELETE`, and existence is proven before correctness.
4. [The proof](docs/research/2026-09-04-zero-custom-code-proof.md) — the dispositions, with evidence.

`main` plus the accepted ADRs are the architecture authority. Documents under
`docs/history/` describe the retired standalone Rust design and are provenance
only.

## What the product guarantees, and who guarantees it

| Requirement (ADR 0008 §4) | Owner | How it is checked |
| --- | --- | --- |
| delegated work survives worker, supervisor and client restarts | Prime: per-path session leases, `prime-agent -r <sessionFile>`, supervisor replacement | `conformance/runtime/d1-*` |
| an ambiguous external effect is never blindly replayed | Prime: worker recovery marker, no stock client re-issues a mutation | `conformance/runtime/d2-*` |
| every session has an explicit durable transcript | Prime | `conformance/runtime/d8-*` |
| worker finished ≠ work accepted; stale revisions cannot be accepted | the foreman's correlated ChatGPT reply (delivery id echoed, head SHA named) is the acceptance record; GitHub merge follows it; Prime `--autonomous-gate` | `conformance/runtime/foreman-transport` (TRN); `pins.json` concerns; gate test |
| the pin is exactly what the release published | `pins/`, `scripts/bootstrap.sh` | `conformance/tier1/pin.test.ts` |
| one owner per concern; every package pinned, licensed, and observed to load | `pins/pins.json` | `conformance/tier1`, `conformance/runtime/package-load` |

One requirement has **no enforcement point on the current substrate** and
is documented as open rather than claimed: user-owned approval of high-risk
actions (Prime has no permission system and its Python kernel runs shell
commands below every extension hook; see
[`docs/threat-model.md`](docs/threat-model.md)).

Models run on the user's subscriptions only, never on API keys: Claude
through the vendored `pi-claude-agent-sdk`, which starts the real Claude Code
binary on its own Max-plan login (Prime holds no Anthropic token), and GPT
through Prime's Codex login, which OpenAI endorses for third-party harnesses.

The ChatGPT Web foreman transport is the pinned `pi-gpt` package, driving
ChatGPT's undocumented backend with the user's Codex login. It was adopted
on the user's explicit acceptance of the account risk (ADR 0008 §8
amendment) and measured live on the exact foreman thread.

## Use it

```sh
scripts/bootstrap.sh        # verify and install the pinned Prime Agent
scripts/conformance.sh      # prove the distribution on this machine
```

For a project: copy `harness/settings.project.json` to
`.prime/agent/settings.json` (Prime installs the pinned packages on startup)
and `harness/agents/*.md` to `.pi/agents/`. Run `pins/prime-0.9.1/node_modules/.bin/prime-agent`
from the project, or install the same release yourself and keep the
`pins.json` version.

The skill `cg-conformance` explains bootstrap, the suite, and how to read a
failure; the prompt `cg-review` is the independent-review procedure.

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md) and
[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md). The first review question for any
production change is still **"should this code exist in Command Governor at
all?"**, and as of this release the answer for every capability was no.

Global Claude Code and Codex instructions are managed declaratively by
`DivMode/nix-config`; this repository does not carry repo-local `CLAUDE.md` or
`AGENTS.md` copies.

## Research

Source-grounded research lives under [`docs/research/`](docs/research/);
upstream defect records and ready-to-file drafts under
[`docs/upstream/`](docs/upstream/).

## License

MIT, see [LICENSE](LICENSE).
