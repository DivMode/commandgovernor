# @commandgovernor/harness

The Command Governor package: skills, prompts, role files and the project
settings that install the pinned packages. No extensions, no runtime code.
What each piece is and why it exists is in `package.json`'s `commandgovernor`
block and in `docs/research/2026-09-04-zero-custom-code-proof.md`.

## Roles (`agents/`)

Agent-type definitions in the
[`@gotgenes/pi-subagents`](https://github.com/gotgenes/pi-packages) agent-file
format (YAML frontmatter plus a system-prompt body). They are configuration:
Command Governor ships no code that reads them.

Install into a project:

```sh
mkdir -p .pi/agents && cp harness/agents/{implementer,reviewer,scout,researcher}.md .pi/agents/
```

Then delegate with the `subagent` tool, e.g.
`subagent({ subagent_type: "reviewer", prompt: "...", description: "..." })`.

The bodies encode the independence rule (ADR 0008 invariant 8): the
implementer never approves its own work, the reviewer never reviews work it
implemented and never approves, and the acceptance record is written on
GitHub by the reviewer of record. On Prime Agent the only built-in tool is
the Python kernel, so a role cannot be made read-only by tool allowlist; the
frontmatter comments say so rather than implying a restriction that does not
exist.

Copy only the four role files: `@gotgenes/pi-subagents` offers every `.md`
under `.pi/agents/` as an agent type, whatever its shape.

## Project settings (`settings.project.json`)

Copy to `<project>/.prime/agent/settings.json` (or merge). Prime installs the
listed packages on startup; versions must equal `pins/pins.json`.
