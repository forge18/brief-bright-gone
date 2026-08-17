//! Conservative, recoverable tool-result compressors.
//!
//! This module deliberately refuses to infer provenance from payload bytes.
//! Callers must supply stable tool-result metadata; otherwise bytes pass through.

use crate::{
    detect::{self, ContentType},
    session::{Match, Registry},
    store::Store,
};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Json,
    Tabular,
    Log,
    FileRead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    ToolResult,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metadata {
    pub source: Source,
    pub kind: ToolKind,
    /// Capture time supplied by the tool adapter. `None` is ineligible.
    pub captured_at_secs: Option<u64>,
    /// Explicitly marks bytes that must remain verbatim (I4).
    pub protected: bool,
    /// File reads in the active recent window are never transformed.
    pub in_recent_window: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transform {
    Toon,
    RepeatedLog,
    FileReference,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub digest: String,
    pub transform: Transform,
    pub original_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub bytes: Vec<u8>,
    pub receipt: Option<Receipt>,
}

/// Select one transform only from trusted adapter metadata plus a strict type
/// check. Anything ambiguous, stale, protected, or unsupported passes through.
pub fn classify(
    bytes: &[u8],
    metadata: Metadata,
    now_secs: u64,
    max_age_secs: u64,
) -> Option<Transform> {
    if metadata.source != Source::ToolResult || metadata.protected {
        return None;
    }
    let captured = metadata.captured_at_secs?;
    if captured > now_secs || now_secs.saturating_sub(captured) > max_age_secs {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    match metadata.kind {
        ToolKind::Json if detect::detect(text) == ContentType::Json => Some(Transform::Toon),
        ToolKind::Tabular if detect::detect(text) == ContentType::Tabular => Some(Transform::Toon),
        ToolKind::Log if detect::detect(text) == ContentType::Log => Some(Transform::RepeatedLog),
        // File reads are only considered after session selection; the recent
        // window is an unconditional I4 exclusion.
        ToolKind::FileRead if !metadata.in_recent_window => Some(Transform::FileReference),
        _ => None,
    }
}

/// Persist originals before constructing a reference. The receipt is appended
/// only after the blob has been verified, making the order auditable.
pub fn transform(
    store: &Store,
    sessions: &mut Registry,
    session: &Match,
    bytes: &[u8],
    metadata: Metadata,
    now_secs: u64,
    max_age_secs: u64,
) -> Output {
    let Some(transform) = classify(bytes, metadata, now_secs, max_age_secs) else {
        return Output {
            bytes: bytes.to_vec(),
            receipt: None,
        };
    };

    // Dedup is stateful and is deliberately disabled for new/collision sessions.
    if transform == Transform::FileReference && !sessions.seen_in_existing_session(session, bytes) {
        sessions.remember(session, bytes);
        return Output {
            bytes: bytes.to_vec(),
            receipt: None,
        };
    }
    if transform == Transform::FileReference
        && store
            .was_served_recent_for(bytes, now_secs, max_age_secs)
            .unwrap_or(true)
    {
        return Output {
            bytes: bytes.to_vec(),
            receipt: None,
        };
    }

    let Ok(digest) = store.put(bytes) else {
        return Output {
            bytes: bytes.to_vec(),
            receipt: None,
        };
    };
    // `put` verifies the content address; do not forward an unverified reference.
    if store.get(&digest).ok().flatten().as_deref() != Some(bytes) {
        return Output {
            bytes: bytes.to_vec(),
            receipt: None,
        };
    }
    let rendered = match transform {
        Transform::Toon => toon(bytes, &digest),
        Transform::RepeatedLog => collapse_log(bytes, &digest),
        Transform::FileReference => format!("[bbg:file-ref:{digest}]\n").into_bytes(),
    };
    let receipt = Receipt {
        digest,
        transform,
        original_bytes: bytes.len(),
    };
    if store.record_receipt(&receipt, now_secs).is_err() {
        return Output {
            bytes: bytes.to_vec(),
            receipt: None,
        };
    }
    Output {
        bytes: rendered,
        receipt: Some(receipt),
    }
}

fn toon(bytes: &[u8], digest: &str) -> Vec<u8> {
    // JSON is emitted as a compact scalar/value stream; delimited tool output
    // uses the same deterministic envelope. Original formatting is recovered
    // only from the CCR reference, never by attempting an inverse transform.
    let body = match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(value) => serde_json::to_string(&value).expect("JSON value serializes"),
        Err(_) => std::str::from_utf8(bytes)
            .expect("classifier checked UTF-8")
            .lines()
            .map(str::trim)
            .collect::<Vec<_>>()
            .join("\n"),
    };
    format!("[bbg:toon:{digest}]\n{body}\n").into_bytes()
}

fn collapse_log(bytes: &[u8], digest: &str) -> Vec<u8> {
    let text = std::str::from_utf8(bytes).expect("classifier checked UTF-8");
    let mut out = format!("[bbg:log:{digest}]\n");
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut i = 0;
    while i < lines.len() {
        let mut count = 1;
        while i + count < lines.len() && lines[i + count] == lines[i] {
            count += 1;
        }
        out.push_str(lines[i]);
        if count > 1 {
            out.push_str(&format!("[bbg:repeat:{count}]\n"));
        }
        i += count;
    }
    out.into_bytes()
}

pub fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    fn store() -> Store {
        Store::open(std::env::temp_dir().join(format!(
                "bbg-compress-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )))
        .unwrap()
    }
    fn meta(kind: ToolKind) -> Metadata {
        Metadata {
            source: Source::ToolResult,
            kind,
            captured_at_secs: Some(10),
            protected: false,
            in_recent_window: false,
        }
    }

    #[test]
    fn classifier_fails_open_without_complete_trusted_metadata() {
        assert_eq!(
            classify(
                br#"{"a":1}"#,
                Metadata {
                    source: Source::Other,
                    ..meta(ToolKind::Json)
                },
                11,
                10
            ),
            None
        );
        assert_eq!(
            classify(
                br#"{"a":1}"#,
                Metadata {
                    protected: true,
                    ..meta(ToolKind::Json)
                },
                11,
                10
            ),
            None
        );
        assert_eq!(classify(b"not json", meta(ToolKind::Json), 11, 10), None);
    }

    #[test]
    fn toon_is_recoverable_byte_exactly_and_receipted_after_storage() {
        let store = store();
        let mut sessions = Registry::default();
        let session = Match::New {
            id: "s".into(),
            collision: false,
        };
        let original = b"{\n  \"a\": [1, 2]\n}\n";
        let output = transform(
            &store,
            &mut sessions,
            &session,
            original,
            meta(ToolKind::Json),
            11,
            10,
        );
        let receipt = output.receipt.unwrap();
        assert!(
            String::from_utf8(output.bytes)
                .unwrap()
                .starts_with("[bbg:toon:")
        );
        assert_eq!(store.get(&receipt.digest).unwrap(), Some(original.to_vec()));
        assert_eq!(store.receipts().unwrap().len(), 1);
    }

    #[test]
    fn tabular_toon_is_recoverable_without_assuming_json() {
        let store = store();
        let mut sessions = Registry::default();
        let session = Match::New {
            id: "s".into(),
            collision: false,
        };
        let original = b" name | count \n one  |  1 \n";
        let output = transform(
            &store,
            &mut sessions,
            &session,
            original,
            meta(ToolKind::Tabular),
            11,
            10,
        );
        let receipt = output.receipt.unwrap();
        assert!(
            String::from_utf8(output.bytes)
                .unwrap()
                .starts_with("[bbg:toon:")
        );
        assert_eq!(store.get(&receipt.digest).unwrap(), Some(original.to_vec()));
    }

    #[test]
    fn repeated_logs_are_collapsed_and_recoverable_byte_exactly() {
        let store = store();
        let mut sessions = Registry::default();
        let session = Match::New {
            id: "s".into(),
            collision: false,
        };
        let original = b"INFO tick\nINFO tick\nINFO done\n";
        let output = transform(
            &store,
            &mut sessions,
            &session,
            original,
            meta(ToolKind::Log),
            11,
            10,
        );
        let receipt = output.receipt.unwrap();
        assert!(
            String::from_utf8(output.bytes)
                .unwrap()
                .contains("[bbg:repeat:2]")
        );
        assert_eq!(store.get(&receipt.digest).unwrap(), Some(original.to_vec()));
    }

    #[test]
    fn file_dedup_happens_only_after_existing_session_and_exempts_served_content() {
        let store = store();
        let mut sessions = Registry::default();
        let bytes = b"file bytes";
        let new = Match::New {
            id: "s".into(),
            collision: false,
        };
        assert!(
            transform(
                &store,
                &mut sessions,
                &new,
                bytes,
                meta(ToolKind::FileRead),
                11,
                10
            )
            .receipt
            .is_none()
        );
        let existing = Match::Existing("s".into());
        let output = transform(
            &store,
            &mut sessions,
            &existing,
            bytes,
            meta(ToolKind::FileRead),
            11,
            10,
        );
        assert!(
            String::from_utf8(output.bytes)
                .unwrap()
                .starts_with("[bbg:file-ref:")
        );
        let digest = store.put(bytes).unwrap();
        store.mark_served(&digest, 12).unwrap();
        assert!(
            transform(
                &store,
                &mut sessions,
                &existing,
                bytes,
                meta(ToolKind::FileRead),
                13,
                10
            )
            .receipt
            .is_none()
        );
    }
}
