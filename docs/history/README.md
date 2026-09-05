# Historical design documents

These documents describe the pre-ADR-0008 standalone Rust daemon / SQLite /
browser / MCP control plane. That implementation was frozen by ADR 0008,
superseded by ADRs 0009 and 0010, and removed from the active tree together
with the `crates/` workspace.

They are kept as provenance for the reliability invariants that ADRs 0008–0010
retain as product requirements. Nothing in this directory describes the
current product topology, and nothing here is an instruction to rebuild a
second runtime beside Prime Agent. The current direction is always
`docs/adr/` plus `README.md`.

| Document | What it was |
| --- | --- |
| `architecture.md` | V1 Rust daemon + CLI control-plane architecture |
| `data-model.md` | SQLite authority schema and artifact boundary |
| `state-machines.md` | worker / obligation / delivery / turn state machines |
| `worker-lifecycle.md` | Claude-first worker host, input and watchdog contract |
| `browser-transport.md` | ChatGPT Web browser wake transport (CDP) |
| `mcp-contract.md` | foreman MCP ABI |
| `2026-08-31-architecture-review.md` | independent review of the V1 design |
| `pi-native-migration-notes.md` | notes for migrating off the Rust crates onto the Pi-family substrate (superseded by their deletion) |
| `2026-09-01-pi-package-selection-matrix.md` | package matrix at 2026-09-01 revisions (superseded by the 2026-09-04 proof) |
