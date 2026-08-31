//! Command Governor command-line entry point.
//!
//! # Phase 1 role
//!
//! `command-governor` is the only binary in the workspace. It parses arguments
//! and delegates to `governor-daemon`, which owns all composition; the CLI holds
//! no domain logic and no second copy of any state.
//!
//! Phase 1 aims at just enough surface to exercise the kernel and store locally
//! — `daemon`, `status`, `doctor`, `obligations`. There is no GUI, no ChatGPT
//! browser control, no live MCP connector, and no worker adapter.

/// Scaffold entry point.
///
/// No subcommands are wired yet: this commit establishes the workspace, the
/// toolchain pin, and the CI gates. Argument parsing arrives with the first
/// command that has a daemon behind it to talk to.
fn main() {
    println!("command-governor: Phase 1 scaffold, no subcommands implemented yet");
}
