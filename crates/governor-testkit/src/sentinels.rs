//! The forbidden-persistence fixture, and the sweep that checks it.
//!
//! `docs/data-model.md` "Forbidden-persistence fixture" and `docs/testing.md`
//! SEC-001 name the same corpus: a distinct sentinel injected into the cwd, the
//! prompt, raw tool arguments and results, the shell command, the transcript
//! path, the terminal transcript, intermediate provider records, browser
//! cookies/tokens/headers/bodies, GitHub auth, and environment secrets. After a
//! lifecycle, every durable surface is byte-scanned and the expected result is
//! **zero matches** — except the one designated final-result artifact, which
//! deliberately carries [`FINAL_RESULT_SENTINEL`].
//!
//! # Two different reasons a value stays out
//!
//! Most of the corpus contains a space, a quote, a newline or a `/`, so
//! [`SafeToken`](governor_core::fence::SafeToken) refuses it and the value can
//! never reach a store API at all. A cookie value and an opaque provider
//! identity are, as strings, indistinguishable — no charset can separate them,
//! and what keeps those out is the schema having nowhere to put them.
//! [`Sentinel::token_shaped`] records which rule each one rests on, so the
//! suite states the honest reason rather than claiming the stronger guarantee
//! everywhere.

/// One piece of forbidden content, and why it must not be persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sentinel {
    /// What kind of forbidden content this stands for.
    pub label: &'static str,
    /// The exact bytes to scan for.
    pub value: &'static str,
    /// Whether the value would pass the redaction-safe charset.
    ///
    /// `false` means the domain refuses it before any I/O. `true` means it is
    /// kept out by there being no column for it, which is a weaker and more
    /// honest claim.
    pub token_shaped: bool,
}

/// The full corpus.
pub const FORBIDDEN: &[Sentinel] = &[
    Sentinel {
        label: "cwd",
        value: "/Volumes/Data/Developer/CGSENTINELCWD",
        token_shaped: false,
    },
    Sentinel {
        label: "prompt",
        value: "please review the diff and ACK CGSENTINELPROMPT",
        token_shaped: false,
    },
    Sentinel {
        label: "raw tool arguments",
        value: r#"{"command":"ls -la","cwd":"/tmp/CGSENTINELTOOLARGS"}"#,
        token_shaped: false,
    },
    Sentinel {
        label: "raw tool result",
        value: "total 48\ndrwxr-xr-x CGSENTINELTOOLRESULT",
        token_shaped: false,
    },
    Sentinel {
        label: "shell command",
        value: "rm -rf /tmp/CGSENTINELCOMMAND",
        token_shaped: false,
    },
    Sentinel {
        label: "transcript path",
        value: "/Users/peter/.claude/CGSENTINELTRANSCRIPTPATH.jsonl",
        token_shaped: false,
    },
    Sentinel {
        label: "terminal transcript",
        value: "$ cargo test\n   Compiling CGSENTINELTERMINAL",
        token_shaped: false,
    },
    Sentinel {
        label: "provider intermediate record",
        value: r#"{"type":"tool_use","id":"CGSENTINELSTREAM"}"#,
        token_shaped: false,
    },
    Sentinel {
        label: "browser authorization header",
        value: "Bearer CGSENTINELHEADER",
        token_shaped: false,
    },
    Sentinel {
        label: "browser response body",
        value: r#"{"conversation":{"id":"CGSENTINELBODY"}}"#,
        token_shaped: false,
    },
    Sentinel {
        label: "browser cookie",
        value: "__Secure-next-auth.session-token=CGSENTINELCOOKIE",
        token_shaped: true,
    },
    Sentinel {
        label: "provider api token",
        value: "sk-proj-CGSENTINELAPITOKEN",
        token_shaped: true,
    },
    Sentinel {
        label: "github credential",
        value: "ghp_CGSENTINELGITHUBCREDENTIAL",
        token_shaped: true,
    },
    Sentinel {
        label: "environment secret",
        value: "CGSENTINELENVSECRET",
        token_shaped: true,
    },
];

/// The one sentinel that is *allowed* to be durable.
///
/// It is placed in the bounded final assistant result, which the artifact store
/// exists to hold. `docs/testing.md` SEC-001: "only the explicit final-result
/// candidate/result artifact may contain a sentinel deliberately placed in the
/// final assistant result".
pub const FINAL_RESULT_SENTINEL: &str = "CGSENTINELFINALRESULT";

/// One place a sentinel was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Path of the file, relative to the state root.
    pub file: String,
    /// Which sentinel was found.
    pub label: &'static str,
}

/// Scans every supplied file for every supplied sentinel.
#[must_use]
pub fn sweep(files: &[(String, Vec<u8>)], sentinels: &[Sentinel]) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (name, bytes) in files {
        for sentinel in sentinels {
            if contains(bytes, sentinel.value.as_bytes()) {
                findings.push(Finding {
                    file: name.clone(),
                    label: sentinel.label,
                });
            }
        }
    }
    findings
}

/// Asserts no forbidden content reached any durable surface.
///
/// # Panics
///
/// Panics naming every file and sentinel that matched.
pub fn assert_no_forbidden_bytes(files: &[(String, Vec<u8>)], context: &str) {
    assert!(
        !files.is_empty(),
        "{context}: the sweep must have something to scan"
    );
    let findings = sweep(files, FORBIDDEN);
    assert!(
        findings.is_empty(),
        "{context}: forbidden content reached durable state: {findings:#?}"
    );
}

/// Asserts the final-result sentinel appears only under `allowed_prefix`.
///
/// # Panics
///
/// Panics naming every file outside the allowed prefix that carries it.
pub fn assert_result_sentinel_confined(
    files: &[(String, Vec<u8>)],
    allowed_prefix: &str,
    context: &str,
) {
    let needle = FINAL_RESULT_SENTINEL.as_bytes();
    let mut leaked = Vec::new();
    let mut found_where_allowed = false;
    for (name, bytes) in files {
        if !contains(bytes, needle) {
            continue;
        }
        if name.starts_with(allowed_prefix) {
            found_where_allowed = true;
        } else {
            leaked.push(name.clone());
        }
    }
    assert!(
        leaked.is_empty(),
        "{context}: the final assistant result leaked outside {allowed_prefix}: {leaked:#?}"
    );
    assert!(
        found_where_allowed,
        "{context}: the designated artifact does not carry the result sentinel, \
         so the sweep proved nothing"
    );
}

/// Asserts a value minted at run time reached none of the supplied surfaces.
///
/// The static corpus cannot cover a secret the run itself generates. The wake
/// correlation ID is the case that matters: `DeliveryId` is a possession fence
/// `foreman_resume` accepts, the store must persist its hex in
/// `browser_deliveries`, and it must appear on no *output* surface — no CLI
/// stdout or stderr, no log line, no rendered error. Pass only those surfaces;
/// scanning the database file would fail for the one reason that is correct.
///
/// # Panics
///
/// Panics naming every surface that carried the value.
pub fn assert_absent(surfaces: &[(String, Vec<u8>)], label: &str, value: &str, context: &str) {
    assert!(
        !surfaces.is_empty(),
        "{context}: the sweep must have something to scan"
    );
    assert!(
        !value.is_empty(),
        "{context}: an empty needle would prove nothing about {label}"
    );
    let leaked: Vec<&String> = surfaces
        .iter()
        .filter(|(_, bytes)| contains(bytes, value.as_bytes()))
        .map(|(name, _)| name)
        .collect();
    assert!(
        leaked.is_empty(),
        "{context}: the {label} reached an output surface: {leaked:#?}"
    );
}

/// Reports whether `haystack` contains `needle`.
#[must_use]
pub fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use governor_core::fence::SafeToken;

    #[test]
    fn every_sentinel_is_distinct() {
        let mut values: Vec<&str> = FORBIDDEN.iter().map(|s| s.value).collect();
        values.push(FINAL_RESULT_SENTINEL);
        let count = values.len();
        values.sort_unstable();
        values.dedup();
        assert_eq!(values.len(), count, "two sentinels would confuse a finding");
    }

    #[test]
    fn the_charset_claim_matches_reality() {
        for sentinel in FORBIDDEN {
            assert_eq!(
                SafeToken::new(sentinel.value).is_ok(),
                sentinel.token_shaped,
                "{}: token_shaped disagrees with the charset",
                sentinel.label
            );
        }
    }

    #[test]
    fn the_designated_result_sentinel_is_in_the_fixture_result() {
        assert!(contains(
            crate::scenario::FINAL_RESULT,
            FINAL_RESULT_SENTINEL.as_bytes()
        ));
    }

    #[test]
    fn a_sweep_finds_what_is_there_and_nothing_else() {
        let files = vec![
            ("db".to_owned(), b"harmless".to_vec()),
            ("wal".to_owned(), FORBIDDEN[0].value.as_bytes().to_vec()),
        ];
        let findings = sweep(&files, FORBIDDEN);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file, "wal");
    }
}
