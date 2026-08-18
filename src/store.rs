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
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

/// Served-reference entries are retained at most this long before compaction.
const SERVED_LEDGER_MAX_AGE_SECS: u64 = 30 * 24 * 3600;
/// The ledger is compacted once it exceeds this many bytes. Compaction bounds
/// the file so `was_served_recent` scans stay small instead of growing
/// without bound over the lifetime of the store.
const SERVED_LEDGER_COMPACT_THRESHOLD_BYTES: u64 = 256 * 1024;

/// Store directory name under the resolved home, and the CWD-relative
/// last-resort fallback when no home can be resolved.
const STORE_DIR_NAME: &str = ".bbg-store";

/// Resolve the store directory used by every `bbg`/`bbg-proxy` entry point
/// (blob store, cost ledger, transcript ledger, skill manifest) when
/// `BBG_STORE_DIR` is not set.
///
/// Defaults to `~/.bbg-store` (home-anchored) rather than `./.bbg-store`
/// (CWD-relative): a CWD-relative default splits state across every directory
/// a command happens to be launched from — `bbg install` in one directory and
/// `bbg doctor` in another see different, empty stores. Falls back to the
/// CWD-relative path only if no home directory can be resolved at all, which
/// is the pre-existing behavior and strictly no worse than today.
pub fn default_store_dir() -> PathBuf {
    home_dir()
        .map(|home| home.join(STORE_DIR_NAME))
        .unwrap_or_else(|| PathBuf::from(STORE_DIR_NAME))
}

/// `$HOME` on Unix, `%USERPROFILE%` on Windows (falling back to `$HOME` there
/// too, e.g. under Git Bash). No `dirs`-style crate: this project is
/// deliberately dependency-light, and two env vars cover every platform bbg
/// ships prebuilt binaries for.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
    /// Minimum served-ledger size that triggers the next compaction scan. Shared
    /// across clones so the backoff (see `compact_served_ledger`) survives the
    /// `Store::clone` calls the proxy makes.
    served_compact_floor: Arc<AtomicU64>,
}

impl Store {
    pub fn open(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        ensure_private_dir(&root)?;
        ensure_private_dir(&root.join("blobs"))?;
        ensure_private_dir(&root.join("sigil"))?;
        ensure_private_dir(&root.join("normalization"))?;
        ensure_private_dir(&root.join("ledger"))?;
        ensure_private_dir(&root.join("receipts"))?;
        Ok(Self {
            root,
            served_compact_floor: Arc::new(AtomicU64::new(SERVED_LEDGER_COMPACT_THRESHOLD_BYTES)),
        })
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

    /// Save pre-normalization bytes keyed by the model-visible normalized text.
    /// A key collision with different bytes fails closed so the caller can
    /// forward the original instead of creating an ambiguous recovery mapping.
    pub fn put_normalization_original(
        &self,
        normalized: &str,
        original: &[u8],
    ) -> io::Result<String> {
        let key = normalized_markdown_hash(normalized);
        let path = self.root.join("normalization").join(&key);
        match validate_regular_file(&path) {
            Ok(()) => {
                let mut existing = Vec::new();
                open_private_read(&path)?.read_to_end(&mut existing)?;
                if existing != original {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "normalization recovery key collision",
                    ));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => atomic_write(&path, original)?,
            Err(error) => return Err(error),
        }
        Ok(key)
    }

    pub fn get_normalization_original(&self, normalized: &str) -> io::Result<Option<Vec<u8>>> {
        let path = self
            .root
            .join("normalization")
            .join(normalized_markdown_hash(normalized));
        let mut file = match open_private_read(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(Some(bytes))
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
        )?;
        self.compact_served_ledger()
    }

    /// Bounded rewrite of the served ledger. Only runs when the file is large;
    /// entries older than the max age are dropped so later `was_served_recent`
    /// scans never read an unbounded ledger. Unparseable lines are retained
    /// defensively. Rewrites are atomic via the store's temp-and-rename path.
    ///
    /// When the recent (unprunable) working set alone exceeds the threshold,
    /// compaction cannot shrink the file, so a fixed threshold would re-scan and
    /// rewrite the whole ledger on every serve. To avoid that, the next-scan
    /// floor is raised one threshold above whatever compaction retained, so an
    /// all-recent ledger is only rewritten once per threshold of new appends.
    fn compact_served_ledger(&self) -> io::Result<()> {
        let path = self.root.join("ledger").join("served.jsonl");
        let size = match fs::metadata(&path) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        if size < self.served_compact_floor.load(Ordering::Relaxed) {
            return Ok(());
        }
        let mut file = open_private_read(&path)?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        let now = unix_now_secs();
        let mut retained = Vec::new();
        for line in contents.lines() {
            let keep = serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .and_then(|entry| {
                    entry
                        .get("served_at_secs")
                        .and_then(serde_json::Value::as_u64)
                })
                .map(|served| {
                    served <= now && now.saturating_sub(served) <= SERVED_LEDGER_MAX_AGE_SECS
                })
                .unwrap_or(true);
            if keep {
                retained.extend_from_slice(line.as_bytes());
                retained.push(b'\n');
            }
        }
        atomic_write(&path, &retained)?;
        // Recompute from what this compaction actually retained, so the floor
        // tracks the working set down when entries age out, not just up.
        let floor = (retained.len() as u64)
            .saturating_add(SERVED_LEDGER_COMPACT_THRESHOLD_BYTES)
            .max(SERVED_LEDGER_COMPACT_THRESHOLD_BYTES);
        self.served_compact_floor.store(floor, Ordering::Relaxed);
        Ok(())
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

    /// Digests protected by the served-reference recovery window. Collection
    /// callers must union these with live-session pins.
    pub fn recently_served_digests(
        &self,
        now_secs: u64,
        window_secs: u64,
    ) -> io::Result<HashSet<String>> {
        let path = self.root.join("ledger").join("served.jsonl");
        let mut file = match open_private_read(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashSet::new()),
            Err(error) => return Err(error),
        };
        let mut entries = String::new();
        file.read_to_string(&mut entries)?;
        Ok(entries
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter_map(|entry| {
                let digest = entry.get("digest")?.as_str()?;
                let served = entry.get("served_at_secs")?.as_u64()?;
                (is_digest(digest)
                    && served <= now_secs
                    && now_secs.saturating_sub(served) <= window_secs)
                    .then_some(digest.to_owned())
            })
            .collect())
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

fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
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

    #[test]
    fn default_store_dir_is_home_anchored_not_cwd_relative() {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
        let Some(home) = home.filter(|value| !value.is_empty()) else {
            // No resolvable home in this environment: falling back to the
            // CWD-relative path is the documented, acceptable last resort.
            assert_eq!(default_store_dir(), PathBuf::from(STORE_DIR_NAME));
            return;
        };
        // Home-anchored: independent of CWD, unlike the old `./.bbg-store`.
        assert_eq!(
            default_store_dir(),
            PathBuf::from(home).join(STORE_DIR_NAME)
        );
    }

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
    fn corrupt_or_truncated_blob_fails_closed_with_integrity_error() {
        let store = store();
        let key = store.put(b"recover me").unwrap();
        // Tamper with the stored blob so its bytes no longer hash to the key.
        std::fs::write(store.path(&key), b"tampered").unwrap();
        let error = store.get(&key).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("CCR integrity check failed"));

        // Truncation is also rejected, never silently served.
        let key2 = store.put(b"full content here").unwrap();
        std::fs::write(store.path(&key2), b"ful").unwrap();
        assert_eq!(
            store.get(&key2).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
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
    fn served_reference_pins_protect_only_the_recovery_window() {
        let store = store();
        let digest = store.put(b"recover me").unwrap();
        store.mark_served(&digest, 100).unwrap();
        assert!(
            store
                .recently_served_digests(105, 10)
                .unwrap()
                .contains(&digest)
        );
        assert!(store.recently_served_digests(111, 10).unwrap().is_empty());
    }

    #[test]
    fn served_ledger_compaction_fills_scans_back_to_exact_recent_entries() {
        let store = store();
        let recent = store.put(b"recent blob").unwrap();
        let still = store.put(b"still held").unwrap();
        let path = store.root.join("ledger").join("served.jsonl");
        let mut contents = String::new();
        // A large batch of ancient entries pushes the ledger past the threshold.
        for _ in 0..(SERVED_LEDGER_COMPACT_THRESHOLD_BYTES / 32 + 64) {
            contents.push_str(&format!(
                "{{\"digest\":\"{}\",\"served_at_secs\":1}}\n",
                0_u64
            ));
        }
        let now = unix_now_secs();
        contents
            .push_str(&serde_json::json!({"digest": recent, "served_at_secs": now}).to_string());
        contents.push('\n');
        contents.push_str(&serde_json::json!({"digest": still, "served_at_secs": now}).to_string());
        contents.push('\n');
        fs::write(&path, contents.as_bytes()).unwrap();
        store.compact_served_ledger().unwrap();
        // Old entries pruned; recent ones retained and recoverable.
        assert!(
            store
                .was_served_recent(&recent, now, 30 * 24 * 3600)
                .unwrap()
        );
        assert!(
            store
                .was_served_recent(&still, now, 30 * 24 * 3600)
                .unwrap()
        );
        let remaining = fs::read_to_string(&path).unwrap();
        assert!(
            remaining.lines().count() < 10,
            "ledger should shrink after compaction"
        );
    }

    #[test]
    fn served_ledger_backs_off_when_compaction_cannot_shrink() {
        let store = store();
        let path = store.root.join("ledger").join("served.jsonl");
        let now = unix_now_secs();
        let digest = store.put(b"blob").unwrap();

        // An all-recent ledger just past the threshold: compaction can prune
        // nothing.
        let line = serde_json::json!({"digest": digest, "served_at_secs": now}).to_string();
        let mut contents = String::new();
        while contents.len() < SERVED_LEDGER_COMPACT_THRESHOLD_BYTES as usize + 2048 {
            contents.push_str(&line);
            contents.push('\n');
        }
        fs::write(&path, &contents).unwrap();
        store.compact_served_ledger().unwrap();
        assert!(fs::metadata(&path).unwrap().len() >= SERVED_LEDGER_COMPACT_THRESHOLD_BYTES);

        // Append one ancient entry, then compact again. The floor was raised
        // above the current size, so this scan is skipped and the ancient entry
        // is NOT rewritten away — proving the ledger is not re-scanned per serve.
        let ancient = serde_json::json!({"digest": digest, "served_at_secs": 1}).to_string();
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "{ancient}").unwrap();
        drop(file);
        store.compact_served_ledger().unwrap();
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("\"served_at_secs\":1"),
            "backoff should skip the re-scan, leaving the ancient entry in place"
        );
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
