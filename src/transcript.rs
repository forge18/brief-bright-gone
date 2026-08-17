//! Schema-versioned JSONL transcript records with conservative redaction.
use crate::private_fs::{ensure_private_dir, open_private_append, open_private_read};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{io, io::Read, path::Path, sync::LazyLock};
pub const TRANSCRIPT_SCHEMA_VERSION: u32 = 1;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TranscriptRecord {
    pub schema_version: u32,
    pub timestamp: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_version: Option<String>,
    #[serde(default)]
    pub lint: Vec<crate::lint::Finding>,
}
static AUTH_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)((?:[\"']?(?:proxy-)?authorization[\"']?\s*[:=]\s*[\"']?)(?:bearer\s+)?)[^\s,;\"'}]+"#,
    )
    .expect("valid authorization redaction regex")
});
static DOUBLE_QUOTED_NAMED_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)((?:[\"]?(?:x-api-key|api[_-]?key|access[_-]?token|token|password|secret)[\"]?\s*[:=]\s*\")[^\"\r\n]*\")"#,
    )
    .expect("valid double-quoted named-secret regex")
});
static SINGLE_QUOTED_NAMED_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)((?:[']?(?:x-api-key|api[_-]?key|access[_-]?token|token|password|secret)[']?\s*[:=]\s*')[^'\r\n]*')"#,
    )
    .expect("valid single-quoted named-secret regex")
});
static NAMED_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)((?:[\"']?(?:x-api-key|api[_-]?key|access[_-]?token|token|password|secret)[\"']?\s*[:=]\s*[\"']?))[^\s,;&\"'}]+"#,
    )
    .expect("valid named-secret redaction regex")
});
static COOKIE_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)((?:[\"']?(?:set-cookie|cookie|session(?:[_-]?(?:id|token))?)[\"']?\s*[:=]\s*[\"']?))[^\"'\r\n]+"#,
    )
    .expect("valid cookie redaction regex")
});
static URI_USERINFO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(https?://)[^/\s@]+@"#).expect("valid URI userinfo redaction regex")
});
static PRIVATE_KEY_BEGIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*-----BEGIN (?:[A-Z0-9 ]+ )?PRIVATE KEY-----\s*$"#)
        .expect("valid private key begin regex")
});
static PRIVATE_KEY_END: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*-----END (?:[A-Z0-9 ]+ )?PRIVATE KEY-----\s*$"#)
        .expect("valid private key end regex")
});

/// Redact recognized credential forms before transcript serialization.
/// Record and line boundaries remain stable; secret length does not.
pub fn redact(s: &str) -> String {
    let mut in_private_key = false;
    s.lines()
        .map(|line| {
            if PRIVATE_KEY_BEGIN.is_match(line) {
                in_private_key = true;
                return line.to_owned();
            }
            if in_private_key {
                if PRIVATE_KEY_END.is_match(line) {
                    in_private_key = false;
                    return line.to_owned();
                }
                return "[REDACTED]".to_owned();
            }
            if let Ok(mut value) = serde_json::from_str::<Value>(line) {
                redact_json(&mut value);
                return serde_json::to_string(&value).expect("JSON value serializes");
            }
            redact_unstructured(line)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn redact_json(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                let normalized = key.to_ascii_lowercase().replace('-', "_");
                if matches!(
                    normalized.as_str(),
                    "authorization"
                        | "proxy_authorization"
                        | "x_api_key"
                        | "api_key"
                        | "access_token"
                        | "token"
                        | "password"
                        | "secret"
                        | "cookie"
                        | "set_cookie"
                        | "session"
                        | "session_id"
                        | "session_token"
                ) {
                    *value = Value::String("[REDACTED]".into());
                } else {
                    redact_json(value);
                }
            }
        }
        Value::Array(values) => values.iter_mut().for_each(redact_json),
        Value::String(text) => *text = redact_unstructured(text),
        _ => {}
    }
}

fn redact_unstructured(line: &str) -> String {
    let line = AUTH_SECRET.replace_all(line, "${1}[REDACTED]");
    let line = COOKIE_SECRET.replace_all(&line, "${1}[REDACTED]");
    let line = DOUBLE_QUOTED_NAMED_SECRET.replace_all(&line, |captures: &regex::Captures<'_>| {
        let matched = captures.get(1).expect("full quoted secret").as_str();
        let prefix_end = matched.rfind('"').expect("closing quote");
        let value_start = matched[..prefix_end].rfind('"').expect("opening quote");
        format!("{}[REDACTED]\"", &matched[..=value_start])
    });
    let line = SINGLE_QUOTED_NAMED_SECRET.replace_all(&line, |captures: &regex::Captures<'_>| {
        let matched = captures.get(1).expect("full quoted secret").as_str();
        let prefix_end = matched.rfind('\'').expect("closing quote");
        let value_start = matched[..prefix_end].rfind('\'').expect("opening quote");
        format!("{}[REDACTED]'", &matched[..=value_start])
    });
    let line = NAMED_SECRET.replace_all(&line, "${1}[REDACTED]");
    URI_USERINFO
        .replace_all(&line, "${1}[REDACTED]@")
        .into_owned()
}

impl TranscriptRecord {
    pub fn new(
        timestamp: String,
        session_id: String,
        role: String,
        content: String,
        skill_version: Option<String>,
    ) -> Self {
        Self {
            schema_version: TRANSCRIPT_SCHEMA_VERSION,
            timestamp,
            session_id,
            role,
            content: redact(&content),
            skill_version,
            lint: Vec::new(),
        }
    }
}
pub fn append(path: &Path, record: &TranscriptRecord) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        ensure_private_dir(parent)?;
    }
    let mut f = open_private_append(path)?;
    serde_json::to_writer(&mut f, record).map_err(io::Error::other)?;
    use std::io::Write;
    f.write_all(b"\n")?;
    f.sync_all()
}
pub fn read(path: &Path) -> io::Result<Vec<TranscriptRecord>> {
    let mut file = match open_private_read(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error),
    };
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    contents
        .lines()
        .map(|line| serde_json::from_str(line).map_err(io::Error::other))
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn redacts_headers_assignments_queries_and_json_without_changing_lines() {
        let content = concat!(
            "Authorization: Bearer auth-value\n",
            "x-api-key: header-value\n",
            "OPENAI_API_KEY=env-value\n",
            "https://example.test/?token=query-value&ok=1\n",
            r#"{"password":"correct horse battery staple","items":[{"token":"array secret"}],"safe":"kept"}"#,
            "\nhello"
        );
        let r = TranscriptRecord::new(
            "t".into(),
            "s".into(),
            "assistant".into(),
            content.into(),
            None,
        );
        for secret in [
            "auth-value",
            "header-value",
            "env-value",
            "query-value",
            "correct horse battery staple",
            "array secret",
        ] {
            assert!(!r.content.contains(secret));
        }
        assert_eq!(r.content.lines().count(), content.lines().count());
        assert!(r.content.contains("ok=1"));
        assert!(r.content.contains(r#""safe":"kept""#));
        assert_eq!(r.schema_version, 1);
    }

    #[test]
    fn redacts_quoted_nested_credentials_cookies_keys_and_uri_userinfo() {
        let content = concat!(
            r#"{"outer":{"Authorization":"Bearer nested-auth","cookie":"sid=cookie-secret","session_id":"session-secret"}}"#,
            "\nProxy-Authorization = 'Bearer proxy-secret'",
            "\npostgres mirror https://db-user:db-password@example.test/path",
            "\npassword='correct horse battery staple'",
            "\n{",
            "\n  \"secret\": \"pretty printed secret phrase\",",
            "\n  \"safe\": \"kept\"",
            "\n}",
            "\n-----BEGIN PRIVATE KEY-----",
            "\nprivate-key-material",
            "\nsecond-private-key-line",
            "\n-----END PRIVATE KEY-----",
            "\nsafe text"
        );
        let redacted = redact(content);
        for secret in [
            "nested-auth",
            "cookie-secret",
            "session-secret",
            "proxy-secret",
            "correct horse battery staple",
            "pretty printed secret phrase",
            "db-user",
            "db-password",
            "private-key-material",
            "second-private-key-line",
        ] {
            assert!(!redacted.contains(secret), "secret leaked: {secret}");
        }
        assert_eq!(redacted.lines().count(), content.lines().count());
        assert!(redacted.contains(r#""outer""#));
        assert!(redacted.contains("example.test/path"));
        assert!(redacted.contains("-----BEGIN PRIVATE KEY-----"));
        assert!(redacted.ends_with("safe text"));
    }

    #[cfg(unix)]
    #[test]
    fn transcript_storage_is_private_and_rejects_symlinks() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = std::env::temp_dir().join(format!(
            "bbg-transcript-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = root.join("records.jsonl");
        let record =
            TranscriptRecord::new("t".into(), "s".into(), "user".into(), "ok".into(), None);
        append(&path, &record).unwrap();
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let link = root.join("linked.jsonl");
        symlink(&path, &link).unwrap();
        assert!(append(&link, &record).is_err());
        assert!(read(&link).is_err());
        let _ = fs::remove_dir_all(root);
    }
}
