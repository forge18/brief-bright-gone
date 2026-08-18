//! Versioned, byte-stable skill and bounded installation support.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

pub const SKILL_VERSION: &str = "1.2.0";
pub const SKILL_FILENAME: &str = "BBG_SKILL.md";
pub const SKILL: &str = r#"# bbg communication skill v1.2.0
Emit compact sigil lines; never delete words from a retained sentence.

Blocks: `§ heading`, `- bullet`, `> consequence`, `! blocking`, `~ note`.
Nesting repeats `-`; `-#` is an ordered item. A sigil is recognized only when followed by whitespace. Use backticks for paths, flags, identifiers, commands and exact errors; fenced code is byte-exact. `*word` emphasizes one non-space span. Tables are `|cell|cell` runs with a header and data rows.

End every response with exactly one top-level terminal: `. result` (done), `? decision; options: ...` (decision needed), or `x cause; options: ...` (blocked). Put the answer first, label actionable severity, state options and uncertainty once, and do not add preambles, recaps, acknowledgments, hedges, or closers. Prefer `-` for parallel facts and `-#` for ordered procedures; otherwise use prose. Keep retained sentences grammatical and preserve identifiers, paths, versions, commands, errors, and line numbers.

Choose one shape; omit empty lines. Task: `§ goal / - done / ? decision; options: ...`; status: `§ state / - proof / > risk / . result`; decision: `§ decision / - recommendation / > tradeoff / ? decision; options: ...`; plan: `§ goal / -# step / -# verify / . result`; pair: `§ observe / - next / - check`; teach: `§ concept / - model / - example / - use / . result`; review: `§ verdict / ! finding — why / - fix / . result`; blocker: `! impact / - root cause / - option / x cause; options: ...`; handoff: `§ scope / - done / - evidence / > risk / . result`; retro: `§ retro / - keep / - change / - try / . result`. Pair is mid-loop; add its terminal only when returning control.

Before editing content last received as `[bbg:file-ref:<ref>]`, run `bbg get <ref>` to recover its exact bytes.
"#;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManifestEntry {
    pub path: PathBuf,
    pub digest: String,
    pub skill_version: String,
    pub installer_version: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    pub schema_version: u32,
    pub entries: Vec<ManifestEntry>,
}

fn manifest_path() -> PathBuf {
    env::var_os("BBG_STORE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(crate::store::default_store_dir)
        .join("skill-manifest.json")
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn load_manifest() -> io::Result<Manifest> {
    match fs::read(manifest_path()) {
        Ok(b) => serde_json::from_slice(&b).map_err(io::Error::other),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Manifest {
            schema_version: 1,
            entries: vec![],
        }),
        Err(e) => Err(e),
    }
}
fn save_manifest(m: &Manifest) -> io::Result<()> {
    let p = manifest_path();
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    atomic_write(&p, &serde_json::to_vec_pretty(m).map_err(io::Error::other)?)
}
fn target(dir: &Path) -> PathBuf {
    dir.join(SKILL_FILENAME)
}
/// Atomic write with a process-unique temporary name and an fsync before the
/// rename, so concurrent installers never collide on a shared `.tmp` and a
/// crash cannot leave a visible-but-unflushed partial file at the target.
fn unique_temp(path: &Path) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.with_file_name(format!(
        ".{}.{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("bbg"),
        std::process::id(),
        nonce
    ))
}

fn atomic_write(path: &Path, data: &[u8]) -> io::Result<()> {
    fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    let tmp = unique_temp(path);
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        let mut file = options.open(&tmp)?;
        file.write_all(data)?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

use std::io::Write;

/// Bounded, non-recursive probe. Missing locations are intentionally harmless.
pub fn probe_dirs() -> Vec<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from);
    let mut out = Vec::new();
    if let Some(h) = home {
        for rel in [
            ".config/bbg",
            ".config/claude",
            ".config/codex",
            ".config/opencode",
        ] {
            let p = h.join(rel);
            if p.is_dir() {
                out.push(p)
            }
        }
    }
    out
}
pub fn install(paths: &[PathBuf]) -> io::Result<Vec<PathBuf>> {
    let mut m = load_manifest()?;
    let mut written = Vec::new();
    let dirs = if paths.is_empty() {
        probe_dirs()
    } else {
        paths.to_vec()
    };
    for dir in dirs {
        let p = target(&dir);
        if let Some(old) = m.entries.iter().find(|e| e.path == p)
            && p.exists()
            && digest(&fs::read(&p)?) != old.digest
            && !paths.iter().any(|x| target(x) == p)
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("refusing modified unowned skill {}", p.display()),
            ));
        }
        atomic_write(&p, SKILL.as_bytes())?;
        let e = ManifestEntry {
            path: p.clone(),
            digest: digest(SKILL.as_bytes()),
            skill_version: SKILL_VERSION.into(),
            installer_version: env!("CARGO_PKG_VERSION").into(),
        };
        m.entries.retain(|x| x.path != p);
        m.entries.push(e);
        written.push(p);
    }
    save_manifest(&m)?;
    Ok(written)
}
pub fn uninstall() -> io::Result<Vec<PathBuf>> {
    let mut m = load_manifest()?;
    let mut removed = Vec::new();
    m.entries.retain(|e| match fs::read(&e.path) {
        Ok(b) if digest(&b) == e.digest => {
            let _ = fs::remove_file(&e.path);
            removed.push(e.path.clone());
            false
        }
        _ => true,
    });
    save_manifest(&m)?;
    Ok(removed)
}
pub fn upgrade() -> io::Result<Vec<PathBuf>> {
    let entries = load_manifest()?.entries;
    for entry in &entries {
        if let Ok(bytes) = fs::read(&entry.path)
            && digest(&bytes) != entry.digest
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("refusing modified managed skill {}", entry.path.display()),
            ));
        }
    }
    let paths: Vec<_> = entries
        .into_iter()
        .map(|e| e.path.parent().unwrap_or(Path::new(".")).to_path_buf())
        .collect();
    install(&paths)
}
pub fn manifest() -> io::Result<Manifest> {
    load_manifest()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn atomic_write_survives_and_leaves_no_temp() {
        let root = env::temp_dir().join(format!(
            "bbg-skill-atomic-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let target = root.join("BBG_SKILL.md");
        atomic_write(&target, b"first").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"first");
        atomic_write(&target, b"second").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"second");
        // No leftover unique temp files after writes.
        let leftovers: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp files must be cleaned up");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skill_requires_exact_recovery_before_editing_a_reference() {
        assert!(SKILL.contains("Before editing content last received as `[bbg:file-ref:<ref>]`"));
        assert!(SKILL.contains("run `bbg get <ref>`"));
    }

    #[test]
    fn skill_has_bounded_format_preferences_and_response_shapes() {
        assert_eq!(SKILL_VERSION, "1.2.0");
        assert!(SKILL.contains(
            "Prefer `-` for parallel facts and `-#` for ordered procedures; otherwise use prose."
        ));
        assert!(SKILL.contains("Choose one shape; omit empty lines."));
        for shape in [
            "Task:",
            "status:",
            "decision:",
            "plan:",
            "pair:",
            "teach:",
            "review:",
            "blocker:",
            "handoff:",
            "retro:",
        ] {
            assert!(SKILL.contains(shape), "skill must include {shape}");
        }
        assert!(SKILL.contains("Pair is mid-loop; add its terminal only when returning control."));
        assert!(!SKILL.contains("comparisons on shared attributes"));
    }

    fn temp_root(label: &str) -> PathBuf {
        env::temp_dir().join(format!(
            "bbg-skill-{label}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    /// A single sequential test owns the process-global `BBG_STORE_DIR` so
    /// parallel test threads never race on the manifest path.
    #[test]
    fn skill_lifecycle_refuses_modified_and_uninstalls_by_digest() {
        let pivot = temp_root("pivot");
        // SAFETY: tests are single-threaded w.r.t. this env var because only
        // this test in the crate sets it and it owns its whole body.
        unsafe {
            env::set_var("BBG_STORE_DIR", &pivot);
        }

        let dir_a = temp_root("a");
        let dir_b = temp_root("b");
        fs::create_dir_all(&dir_a).unwrap();
        fs::create_dir_all(&dir_b).unwrap();

        // install writes the skill at each explicit path and records the manifest.
        let written = install(&[dir_a.clone(), dir_b.clone()]).unwrap();
        assert_eq!(written.len(), 2);
        assert_eq!(
            fs::read(dir_a.join(SKILL_FILENAME)).unwrap(),
            SKILL.as_bytes()
        );
        assert_eq!(load_manifest().unwrap().entries.len(), 2);

        // upgrade refuses when a managed skill has been modified by the user.
        fs::write(dir_a.join(SKILL_FILENAME), b"user edit").unwrap();
        let err = upgrade().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(err.to_string().contains("refusing modified managed skill"));
        // Restore so the rest of the lifecycle is deterministic.
        fs::write(dir_a.join(SKILL_FILENAME), SKILL.as_bytes()).unwrap();

        // uninstall removes only files whose digest still matches the manifest;
        // a modified file is preserved and its manifest entry is retained.
        fs::write(dir_b.join(SKILL_FILENAME), b"user edit").unwrap();
        let removed = uninstall().unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0], dir_a.join(SKILL_FILENAME));
        let manifest = load_manifest().unwrap();
        assert_eq!(manifest.entries.len(), 1, "modified entry is kept");
        assert_eq!(manifest.entries[0].path, dir_b.join(SKILL_FILENAME));

        let _ = fs::remove_dir_all(pivot);
        let _ = fs::remove_dir_all(dir_a);
        let _ = fs::remove_dir_all(dir_b);
    }
}
