use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
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

fn target() -> String {
    let os = if cfg!(target_os = "macos") {
        "apple-darwin"
    } else if cfg!(target_os = "linux") {
        "unknown-linux-gnu"
    } else {
        "pc-windows-msvc"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    format!("{arch}-{os}")
}

fn checksum(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

fn create_archive(root: &Path, archive: &Path, entries: &[&str]) {
    let source = root.join("source");
    fs::create_dir_all(&source).unwrap();
    for entry in entries {
        fs::write(source.join(entry), entry.as_bytes()).unwrap();
    }
    let status = Command::new("tar")
        .args([
            "-czf",
            archive.to_str().unwrap(),
            "-C",
            source.to_str().unwrap(),
        ])
        .args(entries)
        .status()
        .unwrap();
    assert!(status.success());
}

fn create_traversal_archive(archive: &Path) {
    let script = r#"
import io, sys, tarfile
with tarfile.open(sys.argv[1], 'w:gz') as archive:
    info = tarfile.TarInfo('../bbg')
    data = b'hostile'
    info.size = len(data)
    archive.addfile(info, io.BytesIO(data))
"#;
    let status = Command::new("python3")
        .args(["-c", script, archive.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());
}

fn mock_curl(root: &Path) -> PathBuf {
    let bin = root.join("mock-bin");
    fs::create_dir_all(&bin).unwrap();
    let script = bin.join("curl");
    fs::write(
        &script,
        "#!/bin/sh\nset -eu\nout=''\nurl=''\nprev=''\nfor arg in \"$@\"; do\n  if [ \"$prev\" = '-o' ]; then out=\"$arg\"; fi\n  case \"$arg\" in https://*) url=\"$arg\";; esac\n  prev=\"$arg\"\ndone\ncp \"$BBG_FIXTURE_DIR/${url##*/}\" \"$out\"\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    }
    bin
}

fn run_install(root: &Path, fixtures: &Path) -> std::process::Output {
    let mock_bin = mock_curl(root);
    let mut paths = vec![mock_bin];
    if let Some(path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path));
    }
    Command::new("bash")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/install.sh"))
        .env("BBG_VERSION", "1.0.0")
        .env("BBG_INSTALL_DIR", root.join("install"))
        .env("BBG_FIXTURE_DIR", fixtures)
        .env("PATH", std::env::join_paths(paths).unwrap())
        .output()
        .unwrap()
}

fn write_checksums(fixtures: &Path, archive: &Path, checksum_value: &str) {
    fs::write(
        fixtures.join("SHA256SUMS"),
        format!(
            "{checksum_value}  {}\n",
            archive.file_name().unwrap().to_string_lossy()
        ),
    )
    .unwrap();
}

#[test]
fn installer_accepts_local_dual_binary_archive_with_matching_checksum() {
    let root = temporary_root("installer-valid");
    let fixtures = root.join("fixtures");
    fs::create_dir_all(&fixtures).unwrap();
    let archive = fixtures.join(format!("bbg-{}.tar.gz", target()));
    create_archive(&root, &archive, &["bbg", "bbg-proxy"]);
    write_checksums(&fixtures, &archive, &checksum(&archive));

    let output = run_install(&root, &fixtures);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(root.join("install/bbg")).unwrap(), b"bbg");
    assert_eq!(
        fs::read(root.join("install/bbg-proxy")).unwrap(),
        b"bbg-proxy"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn installer_rejects_bad_checksums_and_hostile_archive_entries() {
    for (label, entries, traversal, checksum_value) in [
        ("checksum", vec!["bbg", "bbg-proxy"], false, "00".repeat(32)),
        (
            "multi",
            vec!["bbg", "bbg-proxy", "extra"],
            false,
            String::new(),
        ),
        ("traversal", vec![], true, String::new()),
    ] {
        let root = temporary_root(label);
        let fixtures = root.join("fixtures");
        fs::create_dir_all(&fixtures).unwrap();
        let archive = fixtures.join(format!("bbg-{}.tar.gz", target()));
        if traversal {
            create_traversal_archive(&archive);
        } else {
            create_archive(&root, &archive, &entries);
        }
        let value = if checksum_value.is_empty() {
            checksum(&archive)
        } else {
            checksum_value
        };
        write_checksums(&fixtures, &archive, &value);
        let output = run_install(&root, &fixtures);
        assert!(!output.status.success(), "{label} archive must be rejected");
        assert!(!root.join("install/bbg").exists());
        let _ = fs::remove_dir_all(root);
    }
}
