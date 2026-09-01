//! Deriving a [`ProcessIncarnation`] the OS can be asked about again.
//!
//! # Why a process number is not enough
//!
//! `governor-core`'s [`ProcessIncarnation`] is a process number *plus* an
//! opaque start identity, and equality requires both, so a recycled number is a
//! different incarnation. That is the impersonation the type exists to stop,
//! and it only works if the adapter supplies a start identity that can be
//! **re-derived for somebody else's process number**. A random per-process
//! nonce would distinguish two of our own runs but could never answer "is the
//! process that wrote this lock record still the one running under that
//! number?", which is the question stale-lock reclaim turns on.
//!
//! # The mechanisms, per platform
//!
//! - **Linux**: `/proc/<pid>/stat` field 22, the process start time in clock
//!   ticks since boot. Read directly; no subprocess.
//! - **macOS**: there is no `/proc`, and every in-process route to a
//!   process's start time (`sysctl KERN_PROC_PID`, `proc_pidinfo`) is a raw C
//!   call this workspace's `unsafe_code = "deny"` forbids. So the start time
//!   comes from `ps -o lstart= -p <pid>`, the documented BSD interface to the
//!   same kernel field, and the text is hashed into an opaque token rather than
//!   stored. `lstart` has one-second resolution, which is why it is never the
//!   sole authority: the instance lock's kernel-held advisory lock is
//!   (see [`crate::lock`]), and this value is the corroborating check and the
//!   diagnosis of *how* a holder differs.
//! - **Anything else**: no start identity is derivable, and
//!   [`start_ref`] returns `None`. Reclaim then rests on the kernel lock alone,
//!   which is documented in [`crate::lock`] and reported by `doctor`.

use std::path::Path;
use std::process::Command;

use governor_core::fence::SafeToken;
use governor_core::lease::{ProcessIncarnation, ProcessSlot, ProcessStartRef};
use sha2::{Digest as _, Sha256};

/// How many hex characters of the start-time hash are kept.
///
/// Sixty-four bits of a SHA-256 over a value that is not secret and is only
/// ever compared for equality. Collisions would have to coincide with a
/// recycled process number to matter at all.
const START_HASH_HEX_LEN: usize = 16;

/// Candidate locations of `ps`, tried in order.
///
/// Absolute paths on purpose: resolving `ps` through `PATH` would let whatever
/// environment the daemon was started from decide what answers this question.
const PS_PATHS: &[&str] = &["/bin/ps", "/usr/bin/ps"];

/// This process's own incarnation.
///
/// The start identity is absent when the platform offers no derivable one; the
/// incarnation is still well-formed, and equality then degrades to the process
/// number, which [`crate::lock`] never relies on alone.
#[must_use]
pub fn current() -> ProcessIncarnation {
    let slot = ProcessSlot::new(std::process::id());
    ProcessIncarnation::new(slot, start_ref(slot).unwrap_or_else(unknown_start))
}

/// The incarnation of another process, if it is running.
///
/// Returns `None` when the process number resolves to nothing, which is the
/// proof-of-absence stale-lock reclaim looks for.
#[must_use]
pub fn for_slot(slot: ProcessSlot) -> Option<ProcessIncarnation> {
    start_ref(slot).map(|start| ProcessIncarnation::new(slot, start))
}

/// The opaque start identity of a running process.
///
/// `None` means either "no such process" or "this platform cannot tell us".
/// The two are deliberately not distinguished here: both leave the kernel lock
/// as the only authority, and a caller that treated "cannot tell" as "gone"
/// would be reclaiming on ignorance.
#[must_use]
pub fn start_ref(slot: ProcessSlot) -> Option<ProcessStartRef> {
    let raw = if cfg!(target_os = "linux") {
        linux_start_ticks(slot)
    } else if cfg!(target_os = "macos") {
        bsd_start_text(slot)
    } else {
        None
    }?;
    token(&format!("{}.{}", platform_tag(), digest_hex(&raw))).map(ProcessStartRef::new)
}

/// The start identity recorded when the platform derives none.
///
/// A named placeholder rather than an `Option` because
/// [`ProcessIncarnation`] requires a start identity. It compares equal to
/// itself, so on such a platform two incarnations with the same process number
/// look alike — which is why [`crate::lock`] never treats the incarnation as
/// the authority.
pub const START_UNAVAILABLE: &str = "start-unavailable";

/// The placeholder start identity for a platform that offers none.
fn unknown_start() -> ProcessStartRef {
    ProcessStartRef::new(
        SafeToken::new(START_UNAVAILABLE).expect("the placeholder is a safe token"),
    )
}

const fn platform_tag() -> &'static str {
    if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "unknown"
    }
}

fn token(value: &str) -> Option<SafeToken> {
    SafeToken::new(value).ok()
}

/// Field 22 of `/proc/<pid>/stat`: start time in clock ticks since boot.
fn linux_start_ticks(slot: ProcessSlot) -> Option<String> {
    let text =
        std::fs::read_to_string(Path::new("/proc").join(slot.to_string()).join("stat")).ok()?;
    // The second field is the executable name in parentheses and may itself
    // contain spaces and parentheses, so the split point is the *last* `)`.
    let (_, rest) = text.rsplit_once(')')?;
    // `rest` starts at field 3 (state), so field 22 is offset 19.
    rest.split_whitespace().nth(19).map(str::to_owned)
}

/// `ps -o lstart= -p <pid>`: the process's start time as BSD reports it.
fn bsd_start_text(slot: ProcessSlot) -> Option<String> {
    for candidate in PS_PATHS {
        if !Path::new(candidate).exists() {
            continue;
        }
        let output = Command::new(candidate)
            .arg("-o")
            .arg("lstart=")
            .arg("-p")
            .arg(slot.to_string())
            .output()
            .ok()?;
        if !output.status.success() {
            // A non-zero exit is `ps` saying there is no such process.
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if text.is_empty() {
            return None;
        }
        return Some(text);
    }
    None
}

fn digest_hex(value: &str) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let digest: [u8; 32] = Sha256::digest(value.as_bytes()).into();
    let mut out = String::with_capacity(START_HASH_HEX_LEN);
    for byte in digest.iter().take(START_HASH_HEX_LEN / 2) {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_own_incarnation_names_our_own_process_number() {
        let me = current();
        assert_eq!(me.slot().get(), std::process::id());
    }

    #[test]
    fn our_own_incarnation_is_rederivable_from_the_process_number() {
        // The whole point: another process, holding only our number, must be
        // able to compute the same start identity we did.
        let me = current();
        let rederived = for_slot(me.slot()).expect("our own process is running");
        assert_eq!(rederived, me);
    }

    #[test]
    fn a_process_number_that_resolves_to_nothing_has_no_incarnation() {
        // A number the OS will not hand out. Not merely "probably free": the
        // maximum `pid_t` value is reserved on every platform this builds for.
        let impossible = ProcessSlot::new(u32::from(u16::MAX) * 4 + 3);
        assert_eq!(for_slot(impossible), None);
    }

    #[test]
    fn the_start_identity_is_a_bounded_opaque_token() {
        let me = current();
        let text = me.start().as_token().as_str();
        assert!(!text.is_empty());
        assert!(text.len() <= 32, "start identity should stay short: {text}");
        assert!(
            SafeToken::new(text).is_ok(),
            "the start identity must survive the redaction-safe charset"
        );
    }

    #[test]
    fn a_recycled_process_number_is_a_different_incarnation() {
        let slot = ProcessSlot::new(4242);
        let first = ProcessIncarnation::new(
            slot,
            ProcessStartRef::new(SafeToken::new("macos.aaaaaaaaaaaaaaaa").expect("token")),
        );
        let second = ProcessIncarnation::new(
            slot,
            ProcessStartRef::new(SafeToken::new("macos.bbbbbbbbbbbbbbbb").expect("token")),
        );
        assert_ne!(first, second);
        assert_eq!(
            first.classify(&second),
            Some(governor_core::lease::IncarnationMismatch::SlotReused)
        );
    }
}
