# Security Policy

## Supported versions

Command Governor is pre-release and does not yet publish supported versions.
Security reports about the design, repository, pinned substrate, package
selection or configuration are welcome.

The current security model is in [`docs/threat-model.md`](docs/threat-model.md).
Pre-ADR-0008 Rust/browser/MCP documents under `docs/history/` are provenance,
not the current product topology.

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability or include
sensitive logs, credentials, prompts, conversation contents, repository
source, browser profile data or session data in a public report.

Use GitHub private vulnerability reporting for this repository:

<https://github.com/DivMode/commandgovernor/security/advisories/new>

Include the affected component/version/commit, impact, a minimal safe
reproduction when possible, and any suggested mitigation.

If the defect is in Prime Agent or in a package Command Governor pins, say
so; Command Governor ships no runtime code of its own, so most fixes land
upstream and Command Governor re-pins.

## Trust model in one paragraph

Command Governor is a local-first distribution of Prime Agent plus selected
packages, configuration, skills and conformance tests. The local OS user is
the trust root. Prime, every package and every tool run with that user's
authority. The pinned Prime has no permission or approval system and its
Python kernel executes shell commands below every extension hook, so
destructive work cannot be gated by configuration on the current substrate;
OS containment of the kernel process is the user's control until Prime
exposes a kernel-boundary hook. This is documented as an open limitation in
the threat model, not claimed as mitigated.

## What the distribution does guarantee

- The Prime release it installs is exactly the one it pinned, verified
  against two independent checksum authorities before npm runs, with
  install scripts ignored.
- Every third-party package is pinned to an exact version or commit, with
  license and review date, owns exactly one concern, and is admitted only
  after being observed to load on the pinned Prime.
- After a worker, supervisor or client dies, no stock Prime surface repeats
  an external effect, and the same session is recovered on the same
  transcript. These are Prime's guarantees, asserted by Command Governor's
  black-box conformance suite on every change.
- No undocumented ChatGPT backend transport, borrowed OAuth token, or
  security-control emulation is part of the product.

## Handling

Reports are acknowledged as quickly as possible. Confirmed issues that
belong upstream are forwarded there with a reproducer and tracked under
`docs/upstream/`; issues in Command Governor's own manifest, configuration
or conformance are fixed here.
