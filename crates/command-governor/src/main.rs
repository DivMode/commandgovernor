//! Command Governor command-line entry point.
//!
//! # Phase 1 role
//!
//! `command-governor` is the only binary in the workspace. It parses arguments,
//! delegates to `governor-daemon`, prints what came back, and turns a typed
//! refusal into an exit code. It holds no domain logic and no second copy of
//! any state.
//!
//! Four commands: `daemon`, `status`, `obligations`, `doctor`. There is no GUI,
//! no ChatGPT browser control, no live MCP connector and no worker adapter.
//!
//! # Where the two error styles meet
//!
//! `docs/adr/0002-rust-daemon-and-sqlite.md`: *domain/storage crates expose
//! typed errors with `thiserror`; `anyhow` may be used at binary/composition
//! boundaries to add operational context, but a domain conflict such as stale
//! generation or ambiguous delivery must remain machine-classifiable.* This
//! file is that boundary and the only place in the workspace that depends on
//! `anyhow`. Every refusal arrives already typed and already carrying a stable
//! `code()`; the exit code is chosen from the type, and `anyhow` only adds the
//! sentence around it.
//!
//! # Output
//!
//! Plain `key=value` lines on standard output, produced by `governor-daemon`
//! and printed unchanged. That is deliberate: a presentation layer here would
//! be a second place a field could be added that the safe-diagnostics rule
//! forbids (`docs/threat-model.md`, "Threat: diagnostics become
//! exfiltration"). Diagnostics and refusals go to standard error in the same
//! shape.

mod cli;

use std::process::ExitCode;

use anyhow::Context as _;
use governor_daemon::ipc::{self, IpcError, Request};
use governor_daemon::{Daemon, DaemonConfig, DaemonError, StateRoot, diagnose};

use crate::cli::{Command, USAGE, UsageError};

/// Everything the state root can prove is in order.
const EXIT_OK: u8 = 0;
/// The command line itself was wrong.
const EXIT_USAGE: u8 = 1;
/// The daemon refused to start, or refused a request.
const EXIT_REFUSED: u8 = 2;
/// `doctor` found something wrong with the state root.
const EXIT_UNHEALTHY: u8 = 3;
/// A command that needs the daemon found none running.
const EXIT_NOT_RUNNING: u8 = 4;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let invocation = match cli::parse(&arguments) {
        Ok(invocation) => invocation,
        Err(error) => return usage_error(&error),
    };

    let Some(state_root) = invocation.state_root else {
        return match invocation.command {
            Command::Version => {
                println!("command-governor {}", env!("CARGO_PKG_VERSION"));
                ExitCode::from(EXIT_OK)
            }
            _ => {
                print!("{USAGE}");
                ExitCode::from(EXIT_OK)
            }
        };
    };

    match invocation.command {
        Command::Help => {
            print!("{USAGE}");
            ExitCode::from(EXIT_OK)
        }
        Command::Version => {
            println!("command-governor {}", env!("CARGO_PKG_VERSION"));
            ExitCode::from(EXIT_OK)
        }
        Command::Daemon => run_daemon(state_root),
        Command::Status => query(&state_root, Request::Status),
        Command::Obligations => query(&state_root, Request::Obligations),
        Command::Doctor => run_doctor(&state_root),
    }
}

fn usage_error(error: &UsageError) -> ExitCode {
    eprintln!("error class=usage detail={error}");
    eprint!("{USAGE}");
    ExitCode::from(EXIT_USAGE)
}

/// Starts the authoritative daemon and serves until a signal arrives.
fn run_daemon(state_root: StateRoot) -> ExitCode {
    let started = Daemon::start(DaemonConfig::new(state_root.clone())).with_context(|| {
        format!(
            "starting the Command Governor daemon against {}",
            state_root.path().display()
        )
    });

    let daemon = match started {
        Ok(daemon) => daemon,
        Err(error) => return refusal(&error),
    };

    let ready = daemon.ready();
    println!("daemon.state=ready");
    println!("daemon.slot={}", ready.incarnation.slot());
    println!("daemon.epoch={}", ready.daemon_epoch.get());
    println!("store.schema_epoch={}", ready.schema_epoch);
    println!("artifacts.verified={}", ready.artifacts_verified);
    println!("artifacts.quarantined={}", ready.artifacts_quarantined);
    println!("ipc.socket=bound");

    match daemon
        .run()
        .context("serving the Command Governor control socket")
    {
        Ok(()) => {
            println!("daemon.state=stopped");
            ExitCode::from(EXIT_OK)
        }
        Err(error) => refusal(&error),
    }
}

/// Asks a running daemon one question and prints its answer.
fn query(state_root: &StateRoot, request: Request) -> ExitCode {
    match ipc::request(&state_root.socket_path(), request) {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
            ExitCode::from(EXIT_OK)
        }
        Err(IpcError::NotRunning) => {
            eprintln!("error class=daemon_not_running");
            eprintln!(
                "hint: start one with `command-governor daemon --state-root {}`, \
                 or run `command-governor doctor` to inspect the state root offline",
                state_root.path().display()
            );
            ExitCode::from(EXIT_NOT_RUNNING)
        }
        Err(error) => {
            eprintln!("error class=ipc detail={error}");
            ExitCode::from(EXIT_REFUSED)
        }
    }
}

/// Diagnoses the state root, with or without a daemon running.
fn run_doctor(state_root: &StateRoot) -> ExitCode {
    let diagnosis = diagnose(state_root);
    for line in diagnosis.lines() {
        println!("{line}");
    }
    if diagnosis.healthy() {
        ExitCode::from(EXIT_OK)
    } else {
        ExitCode::from(EXIT_UNHEALTHY)
    }
}

/// Reports a typed refusal with its stable class, then its context.
///
/// The class comes from the typed error and is what a script should branch on;
/// the sentences are `anyhow`'s context, for a person.
fn refusal(error: &anyhow::Error) -> ExitCode {
    let class = error
        .downcast_ref::<DaemonError>()
        .map_or("unclassified", DaemonError::code);
    eprintln!("error class={class}");
    for cause in error.chain() {
        eprintln!("error detail={cause}");
    }
    ExitCode::from(EXIT_REFUSED)
}
