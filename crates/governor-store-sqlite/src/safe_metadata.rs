//! `events.safe_metadata_json`: typed, allowlisted, bounded — never a blob.
//!
//! `docs/data-model.md`: *`safe_metadata_json` is not a generic provider dump.
//! Each event kind has an explicit serializer and allowed bounded fields;
//! unknown fields are discarded.*
//!
//! Two things make that structural rather than aspirational.
//!
//! **There is no generic writer.** A metadata document is built only by
//! [`SafeMetadata`], whose four value shapes are a [`SafeToken`], an opaque
//! identity, a signed integer, and a label from a closed set. There is no
//! method that takes a `&str`, a `serde_json::Value`, or anything else
//! free-form, so a prompt, a tool argument, a shell command, a cwd or a
//! transcript path has no way in. `governor-core` already refuses those at the
//! [`SafeToken`] charset.
//!
//! **The reader is not a JSON parser.** [`SafeMetadata::parse`] accepts exactly
//! one shape: a flat object whose values are strings, integers or booleans. A
//! nested object, an array, a float, or a `null` is a malformed document, so a
//! provider payload cannot round-trip through this column even if some future
//! code path tried to put one there. Keys outside the event kind's allowlist are
//! discarded during parsing.

use std::fmt::Write as _;

use governor_core::fence::SafeToken;
use governor_core::id::{Id, IdKind};

use crate::error::{CorruptReason, CorruptValue, StoreResult};

/// A value that may appear in a safe-metadata document.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SafeValue {
    /// A bounded redaction-safe token, including canonical opaque identities.
    Token(String),
    /// A bounded signed integer: a generation, a revision, an attempt number.
    Int(i64),
    /// A label from one of the closed sets in [`crate::codec`].
    Label(&'static str),
}

/// A typed, allowlisted metadata document for one event kind.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SafeMetadata {
    fields: Vec<(&'static str, SafeValue)>,
}

impl SafeMetadata {
    /// An empty document.
    pub(crate) const fn new() -> Self {
        Self { fields: Vec::new() }
    }

    /// Records a provider-supplied opaque token.
    #[must_use]
    pub(crate) fn token(mut self, key: &'static str, value: &SafeToken) -> Self {
        self.fields
            .push((key, SafeValue::Token(value.as_str().to_owned())));
        self
    }

    /// Records an opaque domain identity.
    ///
    /// Canonical UUID text is inside the [`SafeToken`] charset, so this is the
    /// same value shape as [`Self::token`].
    #[must_use]
    pub(crate) fn id<K: IdKind>(mut self, key: &'static str, value: Id<K>) -> Self {
        self.fields.push((key, SafeValue::Token(value.to_string())));
        self
    }

    /// Records a bounded counter.
    #[must_use]
    pub(crate) fn int(mut self, key: &'static str, value: i64) -> Self {
        self.fields.push((key, SafeValue::Int(value)));
        self
    }

    /// Records a label from a closed set.
    #[must_use]
    pub(crate) fn label(mut self, key: &'static str, value: &'static str) -> Self {
        self.fields.push((key, SafeValue::Label(value)));
        self
    }

    /// Renders the document for the `safe_metadata_json` column.
    pub(crate) fn to_json(&self) -> String {
        let mut out = String::from("{");
        for (index, (key, value)) in self.fields.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            write_json_string(&mut out, key);
            out.push(':');
            match value {
                SafeValue::Token(text) => write_json_string(&mut out, text),
                SafeValue::Label(text) => write_json_string(&mut out, text),
                SafeValue::Int(number) => {
                    let _ = write!(out, "{number}");
                }
            }
        }
        out.push('}');
        out
    }

    /// Parses a stored document, keeping only fields in `allowed`.
    ///
    /// # Errors
    ///
    /// Returns [`CorruptReason::MalformedMetadata`] when the text is not a flat
    /// object of strings, integers and booleans.
    pub(crate) fn parse(text: &str, allowed: &[&str]) -> StoreResult<MetadataFields> {
        let parsed = parse_flat_object(text).ok_or_else(malformed)?;
        Ok(MetadataFields {
            fields: parsed
                .into_iter()
                .filter(|(key, _)| allowed.contains(&key.as_str()))
                .collect(),
        })
    }
}

fn malformed() -> crate::error::StoreError {
    CorruptValue::new(
        "events",
        "safe_metadata_json",
        CorruptReason::MalformedMetadata,
    )
    .into()
}

fn missing() -> crate::error::StoreError {
    CorruptValue::new(
        "events",
        "safe_metadata_json",
        CorruptReason::MissingMetadataField,
    )
    .into()
}

/// The allowlisted fields recovered from a stored metadata document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MetadataFields {
    fields: Vec<(String, ParsedValue)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ParsedValue {
    Text(String),
    Int(i64),
    Bool(bool),
}

impl MetadataFields {
    fn get(&self, key: &str) -> Option<&ParsedValue> {
        self.fields
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    fn text(&self, key: &str) -> StoreResult<&str> {
        match self.get(key) {
            Some(ParsedValue::Text(text)) => Ok(text),
            Some(_) => Err(malformed()),
            None => Err(missing()),
        }
    }

    /// Reads a required opaque token field.
    pub(crate) fn token(&self, key: &str) -> StoreResult<SafeToken> {
        SafeToken::new(self.text(key)?).map_err(|_| {
            CorruptValue::new("events", "safe_metadata_json", CorruptReason::UnsafeToken).into()
        })
    }

    /// Reads a required opaque identity field.
    pub(crate) fn id<K: IdKind>(&self, key: &str) -> StoreResult<Id<K>> {
        Id::parse(self.text(key)?).map_err(|_| {
            CorruptValue::new(
                "events",
                "safe_metadata_json",
                CorruptReason::MalformedIdentity,
            )
            .into()
        })
    }

    /// Reads a required label field, leaving validation to the caller's codec.
    pub(crate) fn label(&self, key: &str) -> StoreResult<&str> {
        self.text(key)
    }

    /// Reads a required unsigned counter field.
    pub(crate) fn u64(&self, key: &str) -> StoreResult<u64> {
        match self.get(key) {
            Some(ParsedValue::Int(value)) => u64::try_from(*value).map_err(|_| {
                CorruptValue::new(
                    "events",
                    "safe_metadata_json",
                    CorruptReason::IntegerOutOfRange,
                )
                .into()
            }),
            Some(_) => Err(malformed()),
            None => Err(missing()),
        }
    }

    /// Reads a required bounded counter field.
    pub(crate) fn u32(&self, key: &str) -> StoreResult<u32> {
        u32::try_from(self.u64(key)?).map_err(|_| {
            CorruptValue::new(
                "events",
                "safe_metadata_json",
                CorruptReason::IntegerOutOfRange,
            )
            .into()
        })
    }

    /// Number of allowlisted fields recovered.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.fields.len()
    }
}

fn write_json_string(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Parses exactly one shape: `{"k":"v","n":1,"b":true}`.
///
/// Returns `None` for anything else, nesting and floats included. That refusal
/// is the point: this column cannot hold a provider document.
fn parse_flat_object(text: &str) -> Option<Vec<(String, ParsedValue)>> {
    let bytes = text.as_bytes();
    let mut at = skip_ws(bytes, 0);
    if bytes.get(at)? != &b'{' {
        return None;
    }
    at = skip_ws(bytes, at + 1);
    let mut fields = Vec::new();
    if bytes.get(at) == Some(&b'}') {
        return (skip_ws(bytes, at + 1) == bytes.len()).then_some(fields);
    }
    loop {
        let (key, next) = parse_string(bytes, at)?;
        at = skip_ws(bytes, next);
        if bytes.get(at)? != &b':' {
            return None;
        }
        at = skip_ws(bytes, at + 1);
        let (value, next) = parse_value(bytes, at)?;
        // A duplicate key is a malformed document, not a last-one-wins merge.
        if fields.iter().any(|(name, _): &(String, _)| name == &key) {
            return None;
        }
        fields.push((key, value));
        at = skip_ws(bytes, next);
        match bytes.get(at)? {
            b',' => at = skip_ws(bytes, at + 1),
            b'}' => return (skip_ws(bytes, at + 1) == bytes.len()).then_some(fields),
            _ => return None,
        }
    }
}

fn skip_ws(bytes: &[u8], mut at: usize) -> usize {
    while matches!(bytes.get(at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        at += 1;
    }
    at
}

fn parse_value(bytes: &[u8], at: usize) -> Option<(ParsedValue, usize)> {
    match bytes.get(at)? {
        b'"' => parse_string(bytes, at).map(|(text, next)| (ParsedValue::Text(text), next)),
        b't' => bytes
            .get(at..at + 4)
            .filter(|slice| *slice == b"true")
            .map(|_| (ParsedValue::Bool(true), at + 4)),
        b'f' => bytes
            .get(at..at + 5)
            .filter(|slice| *slice == b"false")
            .map(|_| (ParsedValue::Bool(false), at + 5)),
        b'-' | b'0'..=b'9' => parse_int(bytes, at),
        _ => None,
    }
}

fn parse_int(bytes: &[u8], at: usize) -> Option<(ParsedValue, usize)> {
    let start = at;
    let mut end = at;
    if bytes.get(end) == Some(&b'-') {
        end += 1;
    }
    let digits_from = end;
    while matches!(bytes.get(end), Some(b'0'..=b'9')) {
        end += 1;
    }
    if end == digits_from {
        return None;
    }
    // A float or an exponent is not a value this column carries.
    if matches!(bytes.get(end), Some(b'.' | b'e' | b'E')) {
        return None;
    }
    let text = std::str::from_utf8(&bytes[start..end]).ok()?;
    text.parse::<i64>().ok().map(|n| (ParsedValue::Int(n), end))
}

fn parse_string(bytes: &[u8], at: usize) -> Option<(String, usize)> {
    if bytes.get(at)? != &b'"' {
        return None;
    }
    let mut out = String::new();
    let mut index = at + 1;
    loop {
        match bytes.get(index)? {
            b'"' => return Some((out, index + 1)),
            b'\\' => {
                // Only the escapes this module's writer emits are accepted.
                match bytes.get(index + 1)? {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'u' => {
                        let hex = std::str::from_utf8(bytes.get(index + 2..index + 6)?).ok()?;
                        let code = u32::from_str_radix(hex, 16).ok()?;
                        out.push(char::from_u32(code)?);
                        index += 4;
                    }
                    _ => return None,
                }
                index += 2;
            }
            _ => {
                // Step by whole characters so multi-byte UTF-8 survives.
                let rest = std::str::from_utf8(bytes.get(index..)?).ok()?;
                let ch = rest.chars().next()?;
                if (ch as u32) < 0x20 {
                    return None;
                }
                out.push(ch);
                index += ch.len_utf8();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use governor_core::id::ObligationId;
    use uuid::Uuid;

    fn token(text: &str) -> SafeToken {
        SafeToken::new(text).expect("test token is safe")
    }

    #[test]
    fn a_typed_document_round_trips() {
        let document = SafeMetadata::new()
            .token("run_ref", &token("run-7"))
            .id("artifact_id", ObligationId::from_uuid(Uuid::from_u128(9)))
            .int("incarnation", 3)
            .label("failure_class", "structured_error");
        let json = document.to_json();

        let fields = SafeMetadata::parse(
            &json,
            &["run_ref", "artifact_id", "incarnation", "failure_class"],
        )
        .expect("the writer's own output parses");
        assert_eq!(fields.token("run_ref").unwrap(), token("run-7"));
        assert_eq!(fields.u64("incarnation").unwrap(), 3);
        assert_eq!(fields.label("failure_class").unwrap(), "structured_error");
        assert_eq!(
            fields
                .id::<governor_core::id::kind::Obligation>("artifact_id")
                .expect("the identity round-trips"),
            ObligationId::from_uuid(Uuid::from_u128(9))
        );
    }

    #[test]
    fn fields_outside_the_allowlist_are_discarded() {
        let json = r#"{"run_ref":"run-1","smuggled":"anything"}"#;
        let fields = SafeMetadata::parse(json, &["run_ref"]).unwrap();
        assert_eq!(fields.len(), 1);
        assert!(fields.token("smuggled").is_err());
    }

    #[test]
    fn an_empty_document_is_valid() {
        let fields = SafeMetadata::parse("{}", &[]).unwrap();
        assert_eq!(fields.len(), 0);
        assert_eq!(SafeMetadata::new().to_json(), "{}");
    }

    #[test]
    fn a_provider_shaped_document_is_refused() {
        // Nesting, arrays, floats, nulls and trailing junk are exactly the
        // shapes a raw provider record arrives in.
        for document in [
            r#"{"tool_input":{"command":"rm -rf /"}}"#,
            r#"{"messages":["hello"]}"#,
            r#"{"cost":0.42}"#,
            r#"{"cwd":null}"#,
            r#"{"a":1} trailing"#,
            r#"{"a":1,"a":2}"#,
            "[1,2,3]",
            "not json at all",
        ] {
            assert!(
                SafeMetadata::parse(document, &["tool_input", "messages", "cost", "cwd", "a"])
                    .is_err(),
                "{document} must be refused"
            );
        }
    }

    #[test]
    fn a_missing_or_mistyped_field_fails_closed() {
        let json = SafeMetadata::new().int("incarnation", 1).to_json();
        let fields = SafeMetadata::parse(&json, &["incarnation", "run_ref"]).unwrap();
        assert!(fields.token("run_ref").is_err(), "missing field");
        assert!(fields.token("incarnation").is_err(), "wrong shape");
    }

    #[test]
    fn quotes_and_control_characters_cannot_break_out_of_a_value() {
        // `SafeToken` already refuses these, so this is defence in depth for
        // the label path; the assertion is that the writer's output re-parses.
        let mut document = SafeMetadata::new();
        document
            .fields
            .push(("run_ref", SafeValue::Token("a\"b\\c\u{1}d".to_owned())));
        let json = document.to_json();
        let fields = SafeMetadata::parse(&json, &["run_ref"]).unwrap();
        assert_eq!(fields.label("run_ref").unwrap(), "a\"b\\c\u{1}d");
    }
}
