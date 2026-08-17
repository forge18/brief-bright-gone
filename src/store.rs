//! Local content-addressed storage for recoverable proxy transforms.

use crate::private_fs::{
    ensure_private_dir, open_private_append, open_private_read, set_file_permissions,
    validate_regular_file,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn open(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        ensure_private_dir(&root)?;
        ensure_private_dir(&root.join("blobs"))?;
        ensure_private_dir(&root.join("sigil"))?;
        ensure_private_dir(&root.join("ledger"))?;
        ensure_private_dir(&root.join("receipts"))?;
        Ok(Self { root })
    }

    /// Store bytes under their SHA-256 digest. Existing blobs are verified,
    /// preventing a corrupt file from being silently accepted as a cache hit.
    pub fn put(&self, bytes: &[u8]) -> io::Result<String> {
        let digest = digest(bytes);
        let path = self.path(&digest);
        if path.exists() {
            self.verify(&digest)?;
            return Ok(digest);
        }
        atomic_write(&path, bytes)?;
        self.verify(&digest)?;
        Ok(digest)
    }

    pub fn get(&self, expected_digest: &str) -> io::Result<Option<Vec<u8>>> {
        if !is_digest(expected_digest) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid CCR digest",
            ));
        }
        let path = self.path(expected_digest);
        let mut file = match open_private_read(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        if digest(&bytes) != expected_digest {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "CCR integrity check failed",
            ));
        }
        Ok(Some(bytes))
    }

    /// Save original sigil bytes keyed by the normalized decoded Markdown.
    /// First writer wins so concurrent requests cannot overwrite an existing
    /// reversible mapping with unrelated bytes.
    pub fn put_sigil_original(&self, markdown: &str, original: &[u8]) -> io::Result<String> {
        let key = normalized_markdown_hash(markdown);
        let path = self.root.join("sigil").join(&key);
        match validate_regular_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => atomic_write(&path, original)?,
            Err(error) => return Err(error),
        }
        Ok(key)
    }

    /// Record that `bbg get` has served a reference. The ledger is separate
    /// from payload bytes so recovery remains byte-exact.
    pub fn mark_served(&self, digest: &str, served_at_secs: u64) -> io::Result<()> {
        if !is_digest(digest) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid CCR digest",
            ));
        }
        append_line(
            &self.root.join("ledger").join("served.jsonl"),
            &serde_json::json!({"digest": digest, "served_at_secs": served_at_secs}).to_string(),
        )
    }

    pub fn was_served_recent(
        &self,
        digest: &str,
        now_secs: u64,
        window_secs: u64,
    ) -> io::Result<bool> {
        if !is_digest(digest) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid CCR digest",
            ));
        }
        let path = self.root.join("ledger").join("served.jsonl");
        let mut file = match open_private_read(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        let mut entries = String::new();
        file.read_to_string(&mut entries)?;
        Ok(entries
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .any(|entry| {
                entry.get("digest").and_then(serde_json::Value::as_str) == Some(digest)
                    && entry
                        .get("served_at_secs")
                        .and_then(serde_json::Value::as_u64)
                        .is_some_and(|served| {
                            served <= now_secs && now_secs.saturating_sub(served) <= window_secs
                        })
            }))
    }

    pub fn was_served_recent_for(
        &self,
        bytes: &[u8],
        now_secs: u64,
        window_secs: u64,
    ) -> io::Result<bool> {
        self.was_served_recent(&digest(bytes), now_secs, window_secs)
    }

    /// Append an auditable receipt only after `put` has completed and verified
    /// the original blob. A receipt is proof that a forwarded reference had a
    /// durable original at transform time.
    pub fn record_receipt(
        &self,
        receipt: &crate::compress::Receipt,
        at_secs: u64,
    ) -> io::Result<()> {
        let transform = match receipt.transform {
            crate::compress::Transform::Toon => "toon",
            crate::compress::Transform::RepeatedLog => "repeated_log",
            crate::compress::Transform::FileReference => "file_reference",
        };
        append_line(
            &self.root.join("receipts").join("transforms.jsonl"),
            &serde_json::json!({"digest": receipt.digest, "transform": transform, "original_bytes": receipt.original_bytes, "at_secs": at_secs}).to_string(),
        )
    }

    pub fn receipts(&self) -> io::Result<Vec<serde_json::Value>> {
        let path = self.root.join("receipts").join("transforms.jsonl");
        let mut file = match open_private_read(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        Ok(contents
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect())
    }

    /// Remove only blobs proven unpinned by the caller's liveness registry.
    pub fn collect_unpinned(&self, pinned: &HashSet<String>) -> io::Result<usize> {
        let mut removed = 0;
        for entry in fs::read_dir(self.root.join("blobs"))? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_digest(&name) && !pinned.contains(&name) {
                fs::remove_file(entry.path())?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    pub fn get_sigil_original(&self, markdown: &str) -> io::Result<Option<Vec<u8>>> {
        let path = self
            .root
            .join("sigil")
            .join(normalized_markdown_hash(markdown));
        let mut file = match open_private_read(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(Some(bytes))
    }

    fn path(&self, digest: &str) -> PathBuf {
        self.root.join("blobs").join(digest)
    }

    fn verify(&self, digest: &str) -> io::Result<()> {
        self.get(digest).map(|_| ())
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("store path has no parent"))?;
    ensure_private_dir(parent)?;
    let temp = parent.join(format!(".tmp-{}-{}", std::process::id(), unique_suffix()));
    let write_result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        set_file_permissions(path)
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    write_result
}

fn append_line(path: &Path, line: &str) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("ledger path has no parent"))?;
    ensure_private_dir(parent)?;
    let mut file = open_private_append(path)?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()
}

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

pub fn normalized_markdown_hash(markdown: &str) -> String {
    let normalized = markdown
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    digest(normalized.as_bytes())
}

fn is_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn store() -> Store {
        Store::open(std::env::temp_dir().join(format!(
            "bbg-store-{}",
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        )))
        .unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn store_is_owner_only_and_rejects_blob_symlinks() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let store = store();
        assert_eq!(
            fs::metadata(&store.root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(store.root.join("blobs"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );

        let digest = digest(b"linked");
        let outside = store.root.join("outside");
        fs::write(&outside, b"linked").unwrap();
        symlink(&outside, store.path(&digest)).unwrap();
        assert!(store.get(&digest).is_err());
        assert!(store.put(b"linked").is_err());

        let key = store.put(b"private").unwrap();
        assert_eq!(
            fs::metadata(store.path(&key)).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn content_addressing_and_integrity() {
        let store = store();
        let key = store.put(b"exact").unwrap();
        assert_eq!(store.get(&key).unwrap(), Some(b"exact".to_vec()));
    }

    #[test]
    fn rejects_invalid_digest_before_path_lookup() {
        assert!(store().get("../not-a-digest").is_err());
    }

    #[test]
    fn collection_preserves_pinned_blobs() {
        let store = store();
        let keep = store.put(b"keep").unwrap();
        let remove = store.put(b"remove").unwrap();
        let pinned = HashSet::from([keep.clone()]);
        assert_eq!(store.collect_unpinned(&pinned).unwrap(), 1);
        assert_eq!(store.get(&keep).unwrap(), Some(b"keep".to_vec()));
        assert_eq!(store.get(&remove).unwrap(), None);
    }

    #[test]
    fn served_references_are_durable_and_recent_only_within_window() {
        let store = store();
        let digest = store.put(b"recover me").unwrap();
        store.mark_served(&digest, 100).unwrap();
        assert!(store.was_served_recent(&digest, 105, 10).unwrap());
        assert!(!store.was_served_recent(&digest, 111, 10).unwrap());
    }

    #[test]
    fn sigil_originals_use_normalized_markdown_keys() {
        let store = store();
        store
            .put_sigil_original("text  \n", "§ text".as_bytes())
            .unwrap();
        assert_eq!(
            store.get_sigil_original("text\n").unwrap(),
            Some("§ text".as_bytes().to_vec())
        );
    }
}
