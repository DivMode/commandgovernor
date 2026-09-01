//! Argument parsing.
//!
//! # Why this is hand-written
//!
//! Four subcommands, one global option, no positional arguments, no
//! subcommand-specific flags. `clap` would bring a handful of crates into a
//! dependency graph this project keeps deliberately small and audits with
//! `cargo deny` and `cargo audit` on every commit, and it would buy help
//! formatting for a surface that fits on one screen. When the command set grows
//! past what a total match over a `&str` can carry legibly, that trade changes;
//! today it does not.
//!
//! The parser is total and refuses anything it does not recognise. An unknown
//! flag is a usage error, never a silently ignored argument.

use std::path::PathBuf;

use governor_daemon::StateRoot;

/// What the user asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    /// Run the authoritative daemon in the foreground.
    Daemon,
    /// Aggregate obligation, attention and health state.
    Status,
    /// Diagnose the state root.
    Doctor,
    /// List the open obligations.
    Obligations,
    /// Print usage.
    Help,
    /// Print the version.
    Version,
}

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Invocation {
    /// The subcommand.
    pub(crate) command: Command,
    /// The state root, when one was named or could be defaulted.
    pub(crate) state_root: Option<StateRoot>,
}

/// Why a command line was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum UsageError {
    /// No subcommand was given.
    #[error("no command given")]
    NoCommand,
    /// The subcommand is not one this binary implements.
    #[error("unknown command `{0}`")]
    UnknownCommand(String),
    /// An option this binary does not implement.
    #[error("unknown option `{0}`")]
    UnknownOption(String),
    /// `--state-root` was given without a value.
    #[error("--state-root needs a path")]
    MissingStateRoot,
    /// Nothing named a state root and no default could be derived.
    #[error(
        "no state root: pass --state-root <path>, or set CG_STATE_ROOT, \
         XDG_STATE_HOME or HOME"
    )]
    NoDefaultStateRoot,
}

/// The usage text.
pub(crate) const USAGE: &str = "\
command-governor — local-first durable control plane for delegated work

USAGE:
    command-governor <COMMAND> [OPTIONS]

COMMANDS:
    daemon         Run the authoritative daemon in the foreground
    status         Aggregate obligation, attention and health state
    obligations    One line per open obligation
    doctor         Diagnose the state root without taking authority
    help           Print this text
    version        Print the version

OPTIONS:
    --state-root <PATH>    Where durable state lives.
                           Defaults to $CG_STATE_ROOT, else
                           $XDG_STATE_HOME/command-governor, else the
                           per-user data directory for this platform.

EXIT CODES:
    0   healthy
    1   usage error
    2   the daemon refused to serve, or refused to start
    3   the state root is unhealthy (doctor)
    4   no daemon is running on this state root

Output is plain `key=value` lines: opaque identities, classes and counters
only. Phase 1 has no browser, MCP, or worker adapter.
";

/// Parses a command line, excluding the program name.
///
/// # Errors
///
/// Returns [`UsageError`] for anything unrecognised; nothing is guessed at.
pub(crate) fn parse<I, S>(arguments: I) -> Result<Invocation, UsageError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut command = None;
    let mut state_root: Option<PathBuf> = None;
    let mut arguments = arguments.into_iter();

    while let Some(argument) = arguments.next() {
        let argument = argument.as_ref();
        match argument {
            "--state-root" => {
                let value = arguments.next().ok_or(UsageError::MissingStateRoot)?;
                let value = value.as_ref();
                if value.is_empty() {
                    return Err(UsageError::MissingStateRoot);
                }
                state_root = Some(PathBuf::from(value));
            }
            "-h" | "--help" => command = Some(Command::Help),
            "-V" | "--version" => command = Some(Command::Version),
            other if other.starts_with('-') => {
                if let Some(value) = other.strip_prefix("--state-root=") {
                    if value.is_empty() {
                        return Err(UsageError::MissingStateRoot);
                    }
                    state_root = Some(PathBuf::from(value));
                } else {
                    return Err(UsageError::UnknownOption(other.to_owned()));
                }
            }
            other if command.is_none() => command = Some(parse_command(other)?),
            other => return Err(UsageError::UnknownCommand(other.to_owned())),
        }
    }

    let command = command.ok_or(UsageError::NoCommand)?;
    let state_root = match state_root {
        Some(path) => Some(StateRoot::new(path)),
        // `help` and `version` answer without touching a state root, so they
        // must not fail for want of one.
        None if matches!(command, Command::Help | Command::Version) => None,
        None => Some(StateRoot::default_location().ok_or(UsageError::NoDefaultStateRoot)?),
    };

    Ok(Invocation {
        command,
        state_root,
    })
}

fn parse_command(value: &str) -> Result<Command, UsageError> {
    match value {
        "daemon" => Ok(Command::Daemon),
        "status" => Ok(Command::Status),
        "doctor" => Ok(Command::Doctor),
        "obligations" => Ok(Command::Obligations),
        "help" => Ok(Command::Help),
        "version" => Ok(Command::Version),
        other => Err(UsageError::UnknownCommand(other.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(arguments: &[&str]) -> Invocation {
        parse(arguments.iter().copied()).expect("a valid command line")
    }

    #[test]
    fn every_subcommand_parses() {
        for (text, expected) in [
            ("daemon", Command::Daemon),
            ("status", Command::Status),
            ("doctor", Command::Doctor),
            ("obligations", Command::Obligations),
            ("help", Command::Help),
            ("version", Command::Version),
        ] {
            assert_eq!(
                parsed(&[text, "--state-root", "/tmp/cg"]).command,
                expected,
                "{text}"
            );
        }
    }

    #[test]
    fn the_state_root_may_be_given_either_way_and_in_either_order() {
        let expected = StateRoot::new("/tmp/cg");
        assert_eq!(
            parsed(&["status", "--state-root", "/tmp/cg"]).state_root,
            Some(expected.clone())
        );
        assert_eq!(
            parsed(&["--state-root=/tmp/cg", "status"]).state_root,
            Some(expected)
        );
    }

    #[test]
    fn an_unknown_option_or_command_is_refused_rather_than_ignored() {
        assert_eq!(
            parse(["status", "--force"]),
            Err(UsageError::UnknownOption("--force".to_owned()))
        );
        assert_eq!(
            parse(["acknowledge"]),
            Err(UsageError::UnknownCommand("acknowledge".to_owned()))
        );
        assert_eq!(
            parse(["status", "extra"]),
            Err(UsageError::UnknownCommand("extra".to_owned()))
        );
        assert_eq!(parse(Vec::<&str>::new()), Err(UsageError::NoCommand));
    }

    #[test]
    fn a_state_root_flag_without_a_value_is_refused() {
        assert_eq!(
            parse(["status", "--state-root"]),
            Err(UsageError::MissingStateRoot)
        );
        assert_eq!(
            parse(["status", "--state-root="]),
            Err(UsageError::MissingStateRoot)
        );
    }

    #[test]
    fn help_and_version_need_no_state_root() {
        assert_eq!(parse(["help"]).expect("help").state_root, None);
        assert_eq!(parse(["--help"]).expect("help").state_root, None);
        assert_eq!(parse(["-V"]).expect("version").command, Command::Version);
    }

    #[test]
    fn the_usage_text_names_every_command_and_exit_code() {
        for needle in [
            "daemon",
            "status",
            "obligations",
            "doctor",
            "--state-root",
            "CG_STATE_ROOT",
        ] {
            assert!(USAGE.contains(needle), "usage omits {needle}");
        }
        for code in ["0 ", "1 ", "2 ", "3 ", "4 "] {
            assert!(USAGE.contains(code), "usage omits exit code {code}");
        }
    }
}
