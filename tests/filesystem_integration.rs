use brief_bright_gone::{compress::unix_now_secs, store::Store};
use std::{
    collections::HashSet,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temporary_root(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "bbg-{label}-{}-{nonce}-{sequence}",
        std::process::id()
    ))
}

#[test]
fn get_is_byte_exact_and_marks_reference_served_only_after_success() {
    let root = temporary_root("get");
    let store = Store::open(&root).unwrap();
    let original = b"first line\n\0binary tail\n";
    let digest = store.put(original).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_bbg"))
        .env_clear()
        .env("BBG_STORE_DIR", &root)
        .args(["get", &digest])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, original);
    assert!(
        store
            .was_served_recent(&digest, unix_now_secs(), 10)
            .unwrap()
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn collection_preserves_served_pins_and_removes_unpinned_blobs() {
    let root = temporary_root("gc");
    let store = Store::open(&root).unwrap();
    let pinned = store.put(b"pinned").unwrap();
    let stale = store.put(b"stale").unwrap();
    let now = unix_now_secs();
    store.mark_served(&pinned, now).unwrap();
    let pins = store.recently_served_digests(now, 60).unwrap();
    assert_eq!(store.collect_unpinned(&pins).unwrap(), 1);
    assert_eq!(store.get(&pinned).unwrap(), Some(b"pinned".to_vec()));
    assert_eq!(store.get(&stale).unwrap(), None);

    let _ = std::fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn store_paths_are_private_and_final_component_symlinks_are_rejected() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = temporary_root("modes");
    let store = Store::open(&root).unwrap();
    assert_eq!(
        std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o700
    );

    let digest = store.put(b"target").unwrap();
    let blob = root.join("blobs").join(&digest);
    assert_eq!(
        std::fs::metadata(&blob).unwrap().permissions().mode() & 0o777,
        0o600
    );
    std::fs::remove_file(&blob).unwrap();
    let outside = root.join("outside");
    std::fs::write(&outside, b"target").unwrap();
    symlink(&outside, &blob).unwrap();
    assert!(store.get(&digest).is_err());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn collection_with_no_pins_removes_every_blob() {
    let root = temporary_root("no-pins");
    let store = Store::open(&root).unwrap();
    let digest = store.put(b"orphan").unwrap();
    assert_eq!(store.collect_unpinned(&HashSet::new()).unwrap(), 1);
    assert_eq!(store.get(&digest).unwrap(), None);
    let _ = std::fs::remove_dir_all(root);
}
